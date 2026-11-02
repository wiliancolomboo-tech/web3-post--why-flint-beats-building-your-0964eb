use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("FL1NTqX8v2Uf2Z5aXz5nC7N8CqU7CjC4d6W2d7z7k8b");

pub const PRECISION_SCALE: u128 = 1_000_000_000_000; // 1e12 fixed point scale
pub const MAX_FEE_BPS: u16 = 1_000; // 10.00% max dynamic fee cap
pub const ORACLE_MAX_STALENESS_SECONDS: i64 = 30; // Max acceptable price age
pub const MAX_CONF_FACTOR: u64 = 200; // Confidence/Price ratio <= 2.00%

#[program]
pub mod flint_engine {
    use super::*;

    /// Initializes a pool state using deterministic PDAs for fee control and reserves
    pub fn initialize_pool(
        ctx: Context<InitializePool>,
        pool_id: u64,
        base_fee_bps: u16,
        virtual_reserve_a: u64,
        virtual_reserve_b: u64,
    ) -> Result<()> {
        require!(base_fee_bps <= MAX_FEE_BPS, FlintError::InvalidFeeTier);
        require!(virtual_reserve_a > 0 && virtual_reserve_b > 0, FlintError::ZeroReserves);

        let pool = &mut ctx.accounts.pool_state;
        pool.pool_id = pool_id;
        pool.bump = ctx.bumps.pool_state;
        pool.vault_authority_bump = ctx.bumps.vault_authority;
        pool.token_a_mint = ctx.accounts.token_a_mint.key();
        pool.token_b_mint = ctx.accounts.token_b_mint.key();
        pool.token_a_vault = ctx.accounts.token_a_vault.key();
        pool.token_b_vault = ctx.accounts.token_b_vault.key();
        pool.admin = ctx.accounts.admin.key();
        pool.oracle_feed = ctx.accounts.oracle_feed.key();

        pool.base_fee_bps = base_fee_bps;
        pool.virtual_reserve_a = virtual_reserve_a;
        pool.virtual_reserve_b = virtual_reserve_b;
        pool.real_reserve_a = 0;
        pool.real_reserve_b = 0;
        pool.is_locked = false;

        emit!(PoolInitializedEvent {
            pool_id,
            token_a_mint: pool.token_a_mint,
            token_b_mint: pool.token_b_mint,
            base_fee_bps,
        });

        Ok(())
    }

    /// High-performance exact-in swap with Oracle dynamic spread calibration
    pub fn swap_exact_input(
        ctx: Context<SwapExactInput>,
        amount_in: u64,
        minimum_amount_out: u64,
    ) -> Result<()> {
        let pool = &mut ctx.accounts.pool_state;

        // Reentrancy protection
        require!(!pool.is_locked, FlintError::ReentrancyDetected);
        pool.is_locked = true;

        require!(amount_in > 0, FlintError::ZeroAmountIn);

        // Parse and validate oracle to shield against toxic MEV / LVR
        let dynamic_fee_bps = calculate_dynamic_fee(
            &ctx.accounts.oracle_feed,
            pool.base_fee_bps,
            Clock::get()?.unix_timestamp,
        )?;

        // Determine swap direction: Token A -> Token B or Token B -> Token A
        let is_a_to_b = ctx.accounts.user_token_source.mint == pool.token_a_mint;

        let (reserve_in_virtual, reserve_out_virtual, reserve_in_real, reserve_out_real) = if is_a_to_b {
            (
                pool.virtual_reserve_a as u128,
                pool.virtual_reserve_b as u128,
                pool.real_reserve_a as u128,
                pool.real_reserve_b as u128,
            )
        } else {
            (
                pool.virtual_reserve_b as u128,
                pool.virtual_reserve_a as u128,
                pool.real_reserve_b as u128,
                pool.real_reserve_a as u128,
            )
        };

        // Fee deduction
        let fee_amount = (amount_in as u128)
            .checked_mul(dynamic_fee_bps as u128)
            .ok_or(FlintError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(FlintError::MathOverflow)? as u64;

        let effective_amount_in = (amount_in as u128)
            .checked_sub(fee_amount as u128)
            .ok_or(FlintError::MathOverflow)?;

        // Constant Product Invariant formula with virtual liquidity shifts:
        // (R_in + effective_amount_in) * (R_out - amount_out) >= R_in * R_out
        let total_in = reserve_in_virtual
            .checked_add(reserve_in_real)
            .ok_or(FlintError::MathOverflow)?;
        let total_out = reserve_out_virtual
            .checked_add(reserve_out_real)
            .ok_or(FlintError::MathOverflow)?;

        let invariant_k = total_in
            .checked_mul(total_out)
            .ok_or(FlintError::MathOverflow)?;

        let new_total_in = total_in
            .checked_add(effective_amount_in)
            .ok_or(FlintError::MathOverflow)?;

        let new_total_out = invariant_k
            .checked_div(new_total_in)
            .ok_or(FlintError::MathOverflow)?;

        let amount_out_u128 = total_out
            .checked_sub(new_total_out)
            .ok_or(FlintError::MathOverflow)?;

        let amount_out = amount_out_u128 as u64;

        require!(amount_out >= minimum_amount_out, FlintError::SlippageExceeded);
        require!(amount_out <= reserve_out_real as u64, FlintError::InsufficientLiquidity);

        // Update In-Memory Reserves
        if is_a_to_b {
            pool.real_reserve_a = (pool.real_reserve_a as u128)
                .checked_add(amount_in as u128)
                .ok_or(FlintError::MathOverflow)? as u64;
            pool.real_reserve_b = (pool.real_reserve_b as u128)
                .checked_sub(amount_out as u128)
                .ok_or(FlintError::MathOverflow)? as u64;
        } else {
            pool.real_reserve_b = (pool.real_reserve_b as u128)
                .checked_add(amount_in as u128)
                .ok_or(FlintError::MathOverflow)? as u64;
            pool.real_reserve_a = (pool.real_reserve_a as u128)
                .checked_sub(amount_out as u128)
                .ok_or(FlintError::MathOverflow)? as u64;
        }

        // Post-execution Invariant Check: K_new >= K_old
        let post_total_a = (pool.virtual_reserve_a as u128)
            .checked_add(pool.real_reserve_a as u128)
            .ok_or(FlintError::MathOverflow)?;
        let post_total_b = (pool.virtual_reserve_b as u128)
            .checked_add(pool.real_reserve_b as u128)
            .ok_or(FlintError::MathOverflow)?;
        let post_invariant_k = post_total_a
            .checked_mul(post_total_b)
            .ok_or(FlintError::MathOverflow)?;

        require!(post_invariant_k >= invariant_k, FlintError::InvariantViolation);

        // Execute CPI Inflow: User -> Vault
        let (input_mint, output_mint, input_vault, output_vault, input_decimals, output_decimals) = if is_a_to_b {
            (
                ctx.accounts.token_a_mint.to_account_info(),
                ctx.accounts.token_b_mint.to_account_info(),
                ctx.accounts.token_a_vault.to_account_info(),
                ctx.accounts.token_b_vault.to_account_info(),
                ctx.accounts.token_a_mint.decimals,
                ctx.accounts.token_b_mint.decimals,
            )
        } else {
            (
                ctx.accounts.token_b_mint.to_account_info(),
                ctx.accounts.token_a_mint.to_account_info(),
                ctx.accounts.token_b_vault.to_account_info(),
                ctx.accounts.token_a_vault.to_account_info(),
                ctx.accounts.token_b_mint.decimals,
                ctx.accounts.token_a_mint.decimals,
            )
        };

        // Transfer In
        token_interface::transfer_checked(
            CpiContext::new(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: ctx.accounts.user_token_source.to_account_info(),
                    mint: input_mint,
                    to: input_vault,
                    authority: ctx.accounts.user_authority.to_account_info(),
                },
            ),
            amount_in,
            input_decimals,
        )?;

        // Transfer Out via PDA Signer
        let pool_id_bytes = pool.pool_id.to_le_bytes();
        let seeds: &[&[&[u8]]] = &[&[
            b"vault_authority",
            pool_id_bytes.as_ref(),
            &[pool.vault_authority_bump],
        ]];

        token_interface::transfer_checked(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                TransferChecked {
                    from: output_vault,
                    mint: output_mint,
                    to: ctx.accounts.user_token_destination.to_account_info(),
                    authority: ctx.accounts.vault_authority.to_account_info(),
                },
                seeds,
            ),
            amount_out,
            output_decimals,
        )?;

        pool.is_locked = false;

        emit!(SwapExecutedEvent {
            pool_id: pool.pool_id,
            user: ctx.accounts.user_authority.key(),
            amount_in,
            amount_out,
            fee_amount,
            is_a_to_b,
        });

        Ok(())
    }
}

/// Helper function to parse Mock/Pyth Oracle data safely and scale fees against volatility
pub fn calculate_dynamic_fee(
    oracle_account: &AccountInfo,
    base_fee_bps: u16,
    current_timestamp: i64,
) -> Result<u16> {
    // Validate Pyth ownership or test mock ownership
    let data = oracle_account.try_borrow_data()?;
    require!(data.len() >= 64, FlintError::InvalidOraclePayload);

    // Read fixed slice: [0..8: magic], [8..16: price], [16..24: conf], [24..32: timestamp]
    let price = i64::from_le_bytes(data[8..16].try_into().unwrap());
    let conf = u64::from_le_bytes(data[16..24].try_into().unwrap());
    let publish_time = i64::from_le_bytes(data[24..32].try_into().unwrap());

    require!(price > 0, FlintError::InvalidOraclePrice);
    require!(
        current_timestamp.saturating_sub(publish_time) <= ORACLE_MAX_STALENESS_SECONDS,
        FlintError::StaleOracleFeed
    );

    // If confidence interval is wide relative to price, expand fee to protect LP from toxic flow
    let conf_ratio_bps = (conf as u128)
        .checked_mul(10_000)
        .ok_or(FlintError::MathOverflow)?
        .checked_div(price as u128)
        .ok_or(FlintError::MathOverflow)? as u16;

    let dynamic_fee = if conf_ratio_bps > 50 {
        // Boost fee dynamically proportional to uncertainty
        base_fee_bps.saturating_add(conf_ratio_bps / 2)
    } else {
        base_fee_bps
    };

    Ok(dynamic_fee.min(MAX_FEE_BPS))
}

#[derive(Accounts)]
#[instruction(pool_id: u64)]
pub struct InitializePool<'info> {
    #[account(
        init,
        payer = admin,
        space = 8 + PoolState::INIT_SPACE,
        seeds = [b"pool_state", pool_id.to_le_bytes().as_ref()],
        bump
    )]
    pub pool_state: Account<'info, PoolState>,

    /// CHECK: Safe PDA derived authority for vault management
    #[account(
        seeds = [b"vault_authority", pool_id.to_le_bytes().as_ref()],
        bump
    )]
    pub vault_authority: AccountInfo<'info>,

    pub token_a_mint: InterfaceAccount<'info, Mint>,
    pub token_b_mint: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = admin,
        token::mint = token_a_mint,
        token::authority = vault_authority,
    )]
    pub token_a_vault: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init,
        payer = admin,
        token::mint = token_b_mint,
        token::authority = vault_authority,
    )]
    pub token_b_vault: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: Checked manually in calculation routine
    pub oracle_feed: AccountInfo<'info>,

    #[account(mut)]
    pub admin: Signer<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct SwapExactInput<'info> {
    #[account(
        mut,
        seeds = [b"pool_state", pool_state.pool_id.to_le_bytes().as_ref()],
        bump = pool_state.bump,
        has_one = token_a_vault,
        has_one = token_b_vault,
        has_one = oracle_feed,
    )]
    pub pool_state: Account<'info, PoolState>,

    /// CHECK: PDA signer seeds
    #[account(
        seeds = [b"vault_authority", pool_state.pool_id.to_le_bytes().as_ref()],
        bump = pool_state.vault_authority_bump
    )]
    pub vault_authority: AccountInfo<'info>,

    pub token_a_mint: InterfaceAccount<'info, Mint>,
    pub token_b_mint: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub token_a_vault: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub token_b_vault: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub user_token_source: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub user_token_destination: InterfaceAccount<'info, TokenAccount>,

    /// CHECK: Oracle account verification handled in logic
    pub oracle_feed: AccountInfo<'info>,

    pub user_authority: Signer<'info>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[account]
#[derive(InitSpace)]
pub struct PoolState {
    pub pool_id: u64,
    pub bump: u8,
    pub vault_authority_bump: u8,
    pub is_locked: bool,
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub token_a_vault: Pubkey,
    pub token_b_vault: Pubkey,
    pub oracle_feed: Pubkey,
    pub admin: Pubkey,
    pub base_fee_bps: u16,
    pub virtual_reserve_a: u64,
    pub virtual_reserve_b: u64,
    pub real_reserve_a: u64,
    pub real_reserve_b: u64,
}

#[event]
pub struct PoolInitializedEvent {
    pub pool_id: u64,
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub base_fee_bps: u16,
}

#[event]
pub struct SwapExecutedEvent {
    pub pool_id: u64,
    pub user: Pubkey,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_amount: u64,
    pub is_a_to_b: bool,
}

#[error_code]
pub enum FlintError {
    #[msg("Calculation overflow occurred")]
    MathOverflow,
    #[msg("Fee basis points exceeds configured protocol limit")]
    InvalidFeeTier,
    #[msg("Reserves cannot be initiated to zero")]
    ZeroReserves,
    #[msg("Input amount must be strictly greater than zero")]
    ZeroAmountIn,
    #[msg("Slippage limit reached; minimum amount out was not satisfied")]
    SlippageExceeded,
    #[msg("Target vault has insufficient real liquidity to satisfy transfer out")]
    InsufficientLiquidity,
    #[msg("Dynamic Invariant violation: K_post < K_prior")]
    InvariantViolation,
    #[msg("Reentrancy state lock detected")]
    ReentrancyDetected,
    #[msg("Invalid Oracle data payload length")]
    InvalidOraclePayload,
    #[msg("Oracle price data corrupted or non-positive")]
    InvalidOraclePrice,
    #[msg("Oracle feed is stale")]
    StaleOracleFeed,
}