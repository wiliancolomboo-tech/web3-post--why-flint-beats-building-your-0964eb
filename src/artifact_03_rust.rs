use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("FLiNTqX1P4o2k9R8v7N2jZcE8VfBqH9m1bY4sA3gM9e");

pub const PRECISION_SCALE: u128 = 1_000_000_000_000; // 1e12 fixed-point scale
pub const MAX_BPS: u64 = 10_000;
pub const MAX_ORACLE_STALENESS_SLOTS: u64 = 25;

#[program]
pub mod flint_prop_amm {
    use super::*;

    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        base_fee_bps: u16,
        skew_sensitivity_bps: u16,
        target_base_reserve: u64,
        target_quote_reserve: u64,
    ) -> Result<()> {
        require!(base_fee_bps <= 2_000, FlintError::FeeTooHigh); // Max 20%
        require!(skew_sensitivity_bps <= 5_000, FlintError::SensitivityTooHigh);

        let pool = &mut ctx.accounts.pool_config;
        pool.base_mint = ctx.accounts.base_mint.key();
        pool.quote_mint = ctx.accounts.quote_mint.key();
        pool.base_vault = ctx.accounts.base_vault.key();
        pool.quote_vault = ctx.accounts.quote_vault.key();
        pool.authority = ctx.accounts.authority.key();
        pool.authority_bump = ctx.bumps.pool_authority;
        pool.pool_bump = ctx.bumps.pool_config;
        pool.is_locked = false;

        pool.base_fee_bps = base_fee_bps;
        pool.skew_sensitivity_bps = skew_sensitivity_bps;
        pool.target_base_reserve = target_base_reserve;
        pool.target_quote_reserve = target_quote_reserve;
        pool.last_oracle_slot = Clock::get()?.slot;

        emit!(PoolInitialized {
            pool: pool.key(),
            base_mint: pool.base_mint,
            quote_mint: pool.quote_mint,
        });

        Ok(())
    }

    pub fn swap(
        ctx: Context<Swap>,
        amount_in: u64,
        min_amount_out: u64,
        oracle_price_feed: u64, // Scaled by 1e6
        feed_slot: u64,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.pool_config;
        require!(!pool.is_locked, FlintError::ReentrancyDetected);
        pool.is_locked = true; // Transient reentrancy lock

        let current_slot = Clock::get()?.slot;
        require!(
            current_slot.saturating_sub(feed_slot) <= MAX_ORACLE_STALENESS_SLOTS,
            FlintError::StaleOracleFeed
        );

        let base_reserve = ctx.accounts.base_vault.amount;
        let quote_reserve = ctx.accounts.quote_vault.amount;

        let a_to_b = ctx.accounts.user_token_source.mint == pool.base_mint;

        // Dynamic fee calculation based on post-trade skew
        let effective_fee_bps = calculate_dynamic_fee(
            a_to_b,
            amount_in,
            base_reserve,
            quote_reserve,
            pool.target_base_reserve,
            pool.target_quote_reserve,
            pool.base_fee_bps,
            pool.skew_sensitivity_bps,
        )?;

        // Output calculation using Oracle-anchored invariant
        let net_amount_in = (amount_in as u128)
            .checked_mul((MAX_BPS - effective_fee_bps as u64) as u128)
            .ok_or(FlintError::MathOverflow)?
            .checked_div(MAX_BPS as u128)
            .ok_or(FlintError::MathOverflow)? as u64;

        let amount_out = calculate_output_amount(
            a_to_b,
            net_amount_in,
            oracle_price_feed,
            ctx.accounts.base_mint.decimals,
            ctx.accounts.quote_mint.decimals,
        )?;

        require!(amount_out >= min_amount_out, FlintError::SlippageExceeded);

        // Execute Inbound Transfer (User -> Pool Vault)
        let base_mint_key = pool.base_mint;
        let quote_mint_key = pool.quote_mint;
        let (in_vault, out_vault, in_mint, out_mint) = if a_to_b {
            (
                ctx.accounts.base_vault.to_account_info(),
                ctx.accounts.quote_vault.to_account_info(),
                ctx.accounts.base_mint.to_account_info(),
                ctx.accounts.quote_mint.to_account_info(),
            )
        } else {
            (
                ctx.accounts.quote_vault.to_account_info(),
                ctx.accounts.base_vault.to_account_info(),
                ctx.accounts.quote_mint.to_account_info(),
                ctx.accounts.base_mint.to_account_info(),
            )
        };

        // Transfer tokens from user to vault
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.user_token_source.to_account_info(),
                    to: in_vault,
                    authority: ctx.accounts.user.to_account_info(),
                    mint: in_mint,
                },
            ),
            amount_in,
            if a_to_b { ctx.accounts.base_mint.decimals } else { ctx.accounts.quote_mint.decimals },
        )?;

        // Execute Outbound Transfer (Pool Vault -> User)
        let pool_key = pool.key();
        let auth_seeds: &[&[&[u8]]] = &[&[
            b"vault_auth",
            pool_key.as_ref(),
            &[pool.authority_bump],
        ]];

        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: out_vault,
                    to: ctx.accounts.user_token_destination.to_account_info(),
                    authority: ctx.accounts.pool_authority.to_account_info(),
                    mint: out_mint,
                },
                auth_seeds,
            ),
            amount_out,
            if a_to_b { ctx.accounts.quote_mint.decimals } else { ctx.accounts.base_mint.decimals },
        )?;

        // Release transient lock
        let pool = &mut ctx.accounts.pool_config;
        pool.is_locked = false;

        emit!(SwapExecuted {
            pool: pool.key(),
            user: ctx.accounts.user.key(),
            a_to_b,
            amount_in,
            amount_out,
            fee_bps: effective_fee_bps,
        });

        Ok(())
    }

    pub fn keeper_update_targets(
        ctx: Context<KeeperUpdateTargets>,
        new_target_base: u64,
        new_target_quote: u64,
        new_base_fee: u16,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.pool_config;
        require!(new_base_fee <= 2_000, FlintError::FeeTooHigh);

        pool.target_base_reserve = new_target_base;
        pool.target_quote_reserve = new_target_quote;
        pool.base_fee_bps = new_base_fee;

        emit!(TargetsUpdated {
            pool: pool.key(),
            new_target_base,
            new_target_quote,
            new_base_fee,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// MATH & COMPUTATION HELPERS
// -----------------------------------------------------------------------------

#[inline(always)]
pub fn calculate_dynamic_fee(
    a_to_b: bool,
    amount_in: u64,
    base_reserve: u64,
    quote_reserve: u64,
    target_base: u64,
    target_quote: u64,
    base_fee_bps: u16,
    skew_sensitivity_bps: u16,
) -> Result<u16> {
    // If trade pushes inventory further away from target, fee increases.
    // If trade restores inventory toward target, fee decreases.
    let (current_res, target_res) = if a_to_b {
        (base_reserve, target_base)
    } else {
        (quote_reserve, target_quote)
    };

    let is_depleting = current_res < target_res;
    let delta = if is_depleting {
        target_res.saturating_sub(current_res)
    } else {
        current_res.saturating_sub(target_res)
    };

    let skew_ratio = ((delta as u128) * (PRECISION_SCALE)) / (target_res as u128).max(1);
    let penalty = ((skew_ratio * (skew_sensitivity_bps as u128)) / PRECISION_SCALE) as u16;

    let fee = if is_depleting && !a_to_b {
        // Taking out already scarce base token -> high fee
        base_fee_bps.saturating_add(penalty)
    } else if !is_depleting && a_to_b {
        // Adding more base token when already saturated -> high fee
        base_fee_bps.saturating_add(penalty)
    } else {
        // Trade is balancing the pool -> discount fee down to 1 bp minimum
        base_fee_bps.saturating_sub(penalty).max(1)
    };

    Ok(fee.min(MAX_BPS as u16))
}

#[inline(always)]
pub fn calculate_output_amount(
    a_to_b: bool,
    net_amount_in: u64,
    oracle_price: u64, // Base price in terms of Quote, scaled 1e6
    base_decimals: u8,
    quote_decimals: u8,
) -> Result<u64> {
    let price_scale = 1_000_000u128;
    let amount_in_u128 = net_amount_in as u128;

    let amount_out = if a_to_b {
        // Base In -> Quote Out
        // AmountOut = AmountIn * Price * 10^(QuoteDecimals - BaseDecimals) / PriceScale
        let decimal_adjustment = 10u128.pow(quote_decimals as u32);
        let base_adjustment = 10u128.pow(base_decimals as u32);

        amount_in_u128
            .checked_mul(oracle_price as u128)
            .ok_or(FlintError::MathOverflow)?
            .checked_mul(decimal_adjustment)
            .ok_or(FlintError::MathOverflow)?
            .checked_div(price_scale)
            .ok_or(FlintError::MathOverflow)?
            .checked_div(base_adjustment)
            .ok_or(FlintError::MathOverflow)?
    } else {
        // Quote In -> Base Out
        // AmountOut = (AmountIn * PriceScale * 10^BaseDecimals) / (Price * 10^QuoteDecimals)
        let decimal_adjustment = 10u128.pow(base_decimals as u32);
        let quote_adjustment = 10u128.pow(quote_decimals as u32);

        amount_in_u128
            .checked_mul(price_scale)
            .ok_or(FlintError::MathOverflow)?
            .checked_mul(decimal_adjustment)
            .ok_or(FlintError::MathOverflow)?
            .checked_div(oracle_price as u128)
            .ok_or(FlintError::MathOverflow)?
            .checked_div(quote_adjustment)
            .ok_or(FlintError::MathOverflow)?
    };

    Ok(amount_out as u64)
}

// -----------------------------------------------------------------------------
// ACCOUNT CONTEXTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializePool<'info> {
    #[account(
        init,
        payer = payer,
        space = 8 + PoolConfig::INIT_SPACE,
        seeds = [b"flint_pool", base_mint.key().as_ref(), quote_mint.key().as_ref()],
        bump
    )]
    pub pool_config: Account<'info, PoolConfig>,

    /// CHECK: PDA signer for vault balances
    #[account(
        seeds = [b"vault_auth", pool_config.key().as_ref()],
        bump
    )]
    pub pool_authority: UncheckedAccount<'info>,

    pub base_mint: InterfaceAccount<'info, Mint>,
    pub quote_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = payer,
        token::mint = base_mint,
        token::authority = pool_authority,
        seeds = [b"base_vault", pool_config.key().as_ref()],
        bump
    )]
    pub base_vault: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init,
        payer = payer,
        token::mint = quote_mint,
        token::authority = pool_authority,
        seeds = [b"quote_vault", pool_config.key().as_ref()],
        bump
    )]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub payer: Signer<'info>,
    pub authority: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct Swap<'info> {
    #[account(
        mut,
        seeds = [b"flint_pool", base_mint.key().as_ref(), quote_mint.key().as_ref()],
        bump = pool_config.pool_bump
    )]
    pub pool_config: Account<'info, PoolConfig>,

    /// CHECK: Verified via seeds check
    #[account(
        seeds = [b"vault_auth", pool_config.key().as_ref()],
        bump = pool_config.authority_bump
    )]
    pub pool_authority: UncheckedAccount<'info>,

    pub base_mint: InterfaceAccount<'info, Mint>,
    pub quote_mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        seeds = [b"base_vault", pool_config.key().as_ref()],
        bump
    )]
    pub base_vault: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"quote_vault", pool_config.key().as_ref()],
        bump
    )]
    pub quote_vault: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub user_token_source: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub user_token_destination: InterfaceAccount<'info, TokenAccount>,

    pub user: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct KeeperUpdateTargets<'info> {
    #[account(
        mut,
        has_one = authority @ FlintError::UnauthorizedKeeper
    )]
    pub pool_config: Account<'info, PoolConfig>,
    pub authority: Signer<'info>,
}

// -----------------------------------------------------------------------------
// DATA STRUCTURES & ERRORS
// -----------------------------------------------------------------------------

#[account]
#[derive(InitSpace)]
pub struct PoolConfig {
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_vault: Pubkey,
    pub quote_vault: Pubkey,
    pub authority: Pubkey,
    pub target_base_reserve: u64,
    pub target_quote_reserve: u64,
    pub base_fee_bps: u16,
    pub skew_sensitivity_bps: u16,
    pub last_oracle_slot: u64,
    pub authority_bump: u8,
    pub pool_bump: u8,
    pub is_locked: bool,
}

#[event]
pub struct PoolInitialized {
    pub pool: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
}

#[event]
pub struct SwapExecuted {
    pub pool: Pubkey,
    pub user: Pubkey,
    pub a_to_b: bool,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_bps: u16,
}

#[event]
pub struct TargetsUpdated {
    pub pool: Pubkey,
    pub new_target_base: u64,
    pub new_target_quote: u64,
    pub new_base_fee: u16,
}

#[error_code]
pub enum FlintError {
    #[msg("Fee basis points exceeds safety limit (20%)")]
    FeeTooHigh,
    #[msg("Sensitivity basis points exceeds upper limit (50%)")]
    SensitivityTooHigh,
    #[msg("Arithmetic overflow or underflow occurred")]
    MathOverflow,
    #[msg("Oracle price feed is older than max allowed slot delay")]
    StaleOracleFeed,
    #[msg("Execution slippage exceeded user specified limit")]
    SlippageExceeded,
    #[msg("Reentrancy detected: Pool state currently locked")]
    ReentrancyDetected,
    #[msg("Unauthorized invocation of keeper targets")]
    UnauthorizedKeeper,
}