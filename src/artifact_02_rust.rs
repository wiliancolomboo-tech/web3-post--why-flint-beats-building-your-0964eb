use anchor_lang::prelude::*;
use anchor_spl::token_interface::{self, Mint, TokenAccount, TokenInterface, TransferChecked};

declare_id!("FLiNT111111111111111111111111111111111111111");

pub const PRECISION: u128 = 1_000_000_000_000; // 1e12 Fixed Point
pub const MAX_FEE_BPS: u16 = 2_000; // 20% cap under extreme volatility
pub const MIN_FEE_BPS: u16 = 5;     // 0.05% base fee

#[program]
pub mod flint_vs_prop_amm {
    use super::*;

    /// -------------------------------------------------------------------
    /// FLINT PATTERN: Ultra-optimized, dynamic LVR-protected AMM pool setup
    /// -------------------------------------------------------------------
    pub fn initialize_flint_pool(
        ctx: Context<InitializeFlintPool>,
        base_fee_bps: u16,
        volatility_multiplier: u16,
    ) -> Result<()> {
        require!(base_fee_bps >= MIN_FEE_BPS, AmmError::FeeTooLow);
        require!(base_fee_bps <= 500, AmmError::FeeTooHigh);

        let pool = &mut ctx.accounts.pool;
        pool.authority = ctx.accounts.authority.key();
        pool.mint_a = ctx.accounts.mint_a.key();
        pool.mint_b = ctx.accounts.mint_b.key();
        pool.vault_a = ctx.accounts.vault_a.key();
        pool.vault_b = ctx.accounts.vault_b.key();
        pool.base_fee_bps = base_fee_bps;
        pool.volatility_multiplier = volatility_multiplier;
        pool.reserve_a = 0;
        pool.reserve_b = 0;
        pool.last_swap_timestamp = Clock::get()?.unix_timestamp;
        pool.last_slot = Clock::get()?.slot;
        pool.transient_reentrancy_lock = 0;
        pool.bump = ctx.bumps.pool;

        msg!("Flint AMM initialized successfully.");
        Ok(())
    }

    /// Deposits initial liquidity to Flint pool
    pub fn deposit_flint(
        ctx: Context<DepositFlint>,
        amount_a: u64,
        amount_b: u64,
    ) -> Result<()> {
        require!(amount_a > 0 && amount_b > 0, AmmError::ZeroAmount);

        // Transfer Token A
        token_interface::transfer_checked(
            ctx.accounts.into_transfer_to_vault_a_context(),
            amount_a,
            ctx.accounts.mint_a.decimals,
        )?;

        // Transfer Token B
        token_interface::transfer_checked(
            ctx.accounts.into_transfer_to_vault_b_context(),
            amount_b,
            ctx.accounts.mint_b.decimals,
        )?;

        let pool = &mut ctx.accounts.pool;
        pool.reserve_a = pool.reserve_a.checked_add(amount_a).ok_or(AmmError::MathOverflow)?;
        pool.reserve_b = pool.reserve_b.checked_add(amount_b).ok_or(AmmError::MathOverflow)?;

        msg!("Flint Deposit Complete: ResA={}, ResB={}", pool.reserve_a, pool.reserve_b);
        Ok(())
    }

    /// FLINT SWAP: Optimized math, dynamic LVR fee adjustment, transient guard
    pub fn swap_flint(
        ctx: Context<SwapFlint>,
        amount_in: u64,
        min_amount_out: u64,
        direction_a_to_b: bool,
    ) -> Result<u64> {
        require!(amount_in > 0, AmmError::ZeroAmount);

        let pool = &mut ctx.accounts.pool;

        // 1. Transient Reentrancy Guard Check
        require!(pool.transient_reentrancy_lock == 0, AmmError::ReentrancyDetected);
        pool.transient_reentrancy_lock = 1; // Acquire Lock

        let clock = Clock::get()?;
        let current_slot = clock.slot;
        let current_ts = clock.unix_timestamp;

        // 2. Dynamic LVR / Toxic Flow Mitigation Hook
        // Exponentially penalize swaps landing in the exact same slot / millisecond delta
        let slot_delta = current_slot.saturating_sub(pool.last_slot);
        let dynamic_fee_bps = if slot_delta == 0 {
            // Consecutive swap within the exact same slot: Apply maximum anti-sandwich fee
            std::cmp::min(
                pool.base_fee_bps.saturating_add(pool.volatility_multiplier.saturating_mul(10)),
                MAX_FEE_BPS,
            )
        } else if slot_delta < 3 {
            // Elevated fee window
            std::cmp::min(
                pool.base_fee_bps.saturating_add(pool.volatility_multiplier),
                MAX_FEE_BPS,
            )
        } else {
            pool.base_fee_bps
        };

        // 3. Constant Product Invariant Math with Checked Math
        let (reserve_in, reserve_out) = if direction_a_to_b {
            (pool.reserve_a as u128, pool.reserve_b as u128)
        } else {
            (pool.reserve_b as u128, pool.reserve_a as u128)
        };

        require!(reserve_in > 0 && reserve_out > 0, AmmError::InsufficientLiquidity);

        // Effective amount in after fee deduction
        let fee_amount = (amount_in as u128)
            .checked_mul(dynamic_fee_bps as u128)
            .ok_or(AmmError::MathOverflow)?
            .checked_div(10_000)
            .ok_or(AmmError::MathOverflow)?;

        let effective_amount_in = (amount_in as u128)
            .checked_sub(fee_amount)
            .ok_or(AmmError::MathOverflow)?;

        // dy = (dx * y) / (x + dx)
        let numerator = effective_amount_in
            .checked_mul(reserve_out)
            .ok_or(AmmError::MathOverflow)?;
        let denominator = reserve_in
            .checked_mul(PRECISION)
            .ok_or(AmmError::MathOverflow)?
            .checked_add(effective_amount_in.checked_mul(PRECISION).ok_or(AmmError::MathOverflow)?)
            .ok_or(AmmError::MathOverflow)?
            .checked_div(PRECISION)
            .ok_or(AmmError::MathOverflow)?;

        let amount_out_u128 = numerator.checked_div(denominator).ok_or(AmmError::MathOverflow)?;
        let amount_out = u64::try_from(amount_out_u128).map_err(|_| AmmError::MathOverflow)?;

        require!(amount_out >= min_amount_out, AmmError::ExcessiveSlippage);

        // 4. Update internal reserves
        if direction_a_to_b {
            pool.reserve_a = pool.reserve_a.checked_add(amount_in).ok_or(AmmError::MathOverflow)?;
            pool.reserve_b = pool.reserve_b.checked_sub(amount_out).ok_or(AmmError::MathOverflow)?;
        } else {
            pool.reserve_b = pool.reserve_b.checked_add(amount_in).ok_or(AmmError::MathOverflow)?;
            pool.reserve_a = pool.reserve_a.checked_sub(amount_out).ok_or(AmmError::MathOverflow)?;
        }

        pool.last_slot = current_slot;
        pool.last_swap_timestamp = current_ts;

        // 5. SPL Token Transfers with PDA Signer
        let mint_a_key = pool.mint_a;
        let mint_b_key = pool.mint_b;
        let pool_seeds: &[&[&[u8]]] = &[&[
            b"flint_pool",
            mint_a_key.as_ref(),
            mint_b_key.as_ref(),
            &[pool.bump],
        ]];

        if direction_a_to_b {
            // User transfers A to vault
            token_interface::transfer_checked(
                ctx.accounts.into_user_to_vault_a_context(),
                amount_in,
                ctx.accounts.mint_a.decimals,
            )?;
            // Vault transfers B to user
            token_interface::transfer_checked(
                ctx.accounts.into_vault_b_to_user_context().with_signer(pool_seeds),
                amount_out,
                ctx.accounts.mint_b.decimals,
            )?;
        } else {
            // User transfers B to vault
            token_interface::transfer_checked(
                ctx.accounts.into_user_to_vault_b_context(),
                amount_in,
                ctx.accounts.mint_b.decimals,
            )?;
            // Vault transfers A to user
            token_interface::transfer_checked(
                ctx.accounts.into_vault_a_to_user_context().with_signer(pool_seeds),
                amount_out,
                ctx.accounts.mint_a.decimals,
            )?;
        }

        // Release Transient Reentrancy Lock
        pool.transient_reentrancy_lock = 0;

        msg!(
            "Flint Swap: In={}, Out={}, DynamicFeeBps={}, SlotDelta={}",
            amount_in,
            amount_out,
            dynamic_fee_bps,
            slot_delta
        );
        Ok(amount_out)
    }

    /// -------------------------------------------------------------------
    /// ANTI-PATTERN: Proprietary In-House AMM (High CU, Monolithic, Vulnerable)
    /// -------------------------------------------------------------------
    pub fn swap_prop_amm_naive(
        ctx: Context<SwapPropAmm>,
        amount_in: u64,
        min_amount_out: u64,
    ) -> Result<u64> {
        let prop = &mut ctx.accounts.prop_pool;

        // Unoptimized computational loop modeling iterative curve solvers
        // frequently found in naive proprietary AMM implementations
        let mut sim_accumulator: u128 = amount_in as u128;
        for i in 1..=45 {
            sim_accumulator = sim_accumulator
                .saturating_mul(1000 + i)
                .checked_div(1000 + i + 1)
                .unwrap_or(1);
        }

        let reserve_in = prop.reserve_a as u128;
        let reserve_out = prop.reserve_b as u128;

        // Static low fee (leads to LVR leakage to sandwichers)
        let static_fee_bps = 25u128;
        let effective_in = (amount_in as u128)
            .checked_mul(10_000 - static_fee_bps)
            .unwrap()
            .checked_div(10_000)
            .unwrap();

        let amount_out = (effective_in * reserve_out / (reserve_in + effective_in)) as u64;
        require!(amount_out >= min_amount_out, AmmError::ExcessiveSlippage);

        prop.reserve_a = prop.reserve_a.checked_add(amount_in).unwrap();
        prop.reserve_b = prop.reserve_b.checked_sub(amount_out).unwrap();

        // SPL Transfers
        let mint_a = prop.mint_a;
        let mint_b = prop.mint_b;
        let seeds: &[&[&[u8]]] = &[&[b"prop_pool", mint_a.as_ref(), mint_b.as_ref(), &[prop.bump]]];

        token_interface::transfer_checked(
            ctx.accounts.into_user_to_vault_a(),
            amount_in,
            ctx.accounts.mint_a.decimals,
        )?;

        token_interface::transfer_checked(
            ctx.accounts.into_vault_b_to_user().with_signer(seeds),
            amount_out,
            ctx.accounts.mint_b.decimals,
        )?;

        msg!("Prop AMM Inefficient Swap Finished: Out={}", amount_out);
        Ok(amount_out)
    }
}

// -------------------------------------------------------------------
// ACCOUNT STRUCTURES & CONTEXT DEFINITIONS
// -------------------------------------------------------------------

#[account]
#[derive(Default)]
pub struct FlintPool {
    pub authority: Pubkey,                // 32
    pub mint_a: Pubkey,                   // 32
    pub mint_b: Pubkey,                   // 32
    pub vault_a: Pubkey,                  // 32
    pub vault_b: Pubkey,                  // 32
    pub reserve_a: u64,                   // 8
    pub reserve_b: u64,                   // 8
    pub last_swap_timestamp: i64,         // 8
    pub last_slot: u64,                   // 8
    pub base_fee_bps: u16,                // 2
    pub volatility_multiplier: u16,       // 2
    pub transient_reentrancy_lock: u8,    // 1
    pub bump: u8,                         // 1
}

impl FlintPool {
    pub const LEN: usize = 8 + 32*5 + 8*4 + 2*2 + 1 + 1 + 32; // + padding
}

#[account]
#[derive(Default)]
pub struct PropPool {
    pub authority: Pubkey,
    pub mint_a: Pubkey,
    pub mint_b: Pubkey,
    pub vault_a: Pubkey,
    pub vault_b: Pubkey,
    pub reserve_a: u64,
    pub reserve_b: u64,
    pub bump: u8,
}

impl PropPool {
    pub const LEN: usize = 8 + 32*5 + 8*2 + 1 + 16;
}

#[derive(Accounts)]
pub struct InitializeFlintPool<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    pub mint_a: InterfaceAccount<'info, Mint>,
    pub mint_b: InterfaceAccount<'info, Mint>,

    #[account(
        init,
        payer = authority,
        space = FlintPool::LEN,
        seeds = [b"flint_pool", mint_a.key().as_ref(), mint_b.key().as_ref()],
        bump
    )]
    pub pool: Account<'info, FlintPool>,

    #[account(
        init,
        payer = authority,
        seeds = [b"vault_a", pool.key().as_ref()],
        bump,
        token::mint = mint_a,
        token::authority = pool,
    )]
    pub vault_a: InterfaceAccount<'info, TokenAccount>,

    #[account(
        init,
        payer = authority,
        seeds = [b"vault_b", pool.key().as_ref()],
        bump,
        token::mint = mint_b,
        token::authority = pool,
    )]
    pub vault_b: InterfaceAccount<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct DepositFlint<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        mut,
        has_one = vault_a,
        has_one = vault_b,
        seeds = [b"flint_pool", mint_a.key().as_ref(), mint_b.key().as_ref()],
        bump = pool.bump
    )]
    pub pool: Account<'info, FlintPool>,

    pub mint_a: InterfaceAccount<'info, Mint>,
    pub mint_b: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub user_token_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub user_token_b: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub vault_b: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

impl<'info> DepositFlint<'info> {
    pub fn into_transfer_to_vault_a_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        let cpi_accounts = TransferChecked {
            from: self.user_token_a.to_account_info(),
            mint: self.mint_a.to_account_info(),
            to: self.vault_a.to_account_info(),
            authority: self.authority.to_account_info(),
        };
        CpiContext::new(self.token_program.to_account_info(), cpi_accounts)
    }

    pub fn into_transfer_to_vault_b_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        let cpi_accounts = TransferChecked {
            from: self.user_token_b.to_account_info(),
            mint: self.mint_b.to_account_info(),
            to: self.vault_b.to_account_info(),
            authority: self.authority.to_account_info(),
        };
        CpiContext::new(self.token_program.to_account_info(), cpi_accounts)
    }
}

#[derive(Accounts)]
pub struct SwapFlint<'info> {
    pub user: Signer<'info>,

    #[account(
        mut,
        has_one = vault_a,
        has_one = vault_b,
        seeds = [b"flint_pool", mint_a.key().as_ref(), mint_b.key().as_ref()],
        bump = pool.bump
    )]
    pub pool: Account<'info, FlintPool>,

    pub mint_a: InterfaceAccount<'info, Mint>,
    pub mint_b: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub user_token_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub user_token_b: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub vault_b: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

impl<'info> SwapFlint<'info> {
    pub fn into_user_to_vault_a_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        let cpi_accounts = TransferChecked {
            from: self.user_token_a.to_account_info(),
            mint: self.mint_a.to_account_info(),
            to: self.vault_a.to_account_info(),
            authority: self.user.to_account_info(),
        };
        CpiContext::new(self.token_program.to_account_info(), cpi_accounts)
    }

    pub fn into_vault_b_to_user_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        let cpi_accounts = TransferChecked {
            from: self.vault_b.to_account_info(),
            mint: self.mint_b.to_account_info(),
            to: self.user_token_b.to_account_info(),
            authority: self.pool.to_account_info(),
        };
        CpiContext::new(self.token_program.to_account_info(), cpi_accounts)
    }

    pub fn into_user_to_vault_b_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        let cpi_accounts = TransferChecked {
            from: self.user_token_b.to_account_info(),
            mint: self.mint_b.to_account_info(),
            to: self.vault_b.to_account_info(),
            authority: self.user.to_account_info(),
        };
        CpiContext::new(self.token_program.to_account_info(), cpi_accounts)
    }

    pub fn into_vault_a_to_user_context(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        let cpi_accounts = TransferChecked {
            from: self.vault_a.to_account_info(),
            mint: self.mint_a.to_account_info(),
            to: self.user_token_a.to_account_info(),
            authority: self.pool.to_account_info(),
        };
        CpiContext::new(self.token_program.to_account_info(), cpi_accounts)
    }
}

#[derive(Accounts)]
pub struct SwapPropAmm<'info> {
    pub user: Signer<'info>,

    #[account(
        mut,
        has_one = vault_a,
        has_one = vault_b,
        seeds = [b"prop_pool", mint_a.key().as_ref(), mint_b.key().as_ref()],
        bump = prop_pool.bump
    )]
    pub prop_pool: Account<'info, PropPool>,

    pub mint_a: InterfaceAccount<'info, Mint>,
    pub mint_b: InterfaceAccount<'info, Mint>,

    #[account(mut)]
    pub user_token_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub user_token_b: InterfaceAccount<'info, TokenAccount>,

    #[account(mut)]
    pub vault_a: InterfaceAccount<'info, TokenAccount>,
    #[account(mut)]
    pub vault_b: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}

impl<'info> SwapPropAmm<'info> {
    pub fn into_user_to_vault_a(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        let cpi_accounts = TransferChecked {
            from: self.user_token_a.to_account_info(),
            mint: self.mint_a.to_account_info(),
            to: self.vault_a.to_account_info(),
            authority: self.user.to_account_info(),
        };
        CpiContext::new(self.token_program.to_account_info(), cpi_accounts)
    }

    pub fn into_vault_b_to_user(&self) -> CpiContext<'_, '_, '_, 'info, TransferChecked<'info>> {
        let cpi_accounts = TransferChecked {
            from: self.vault_b.to_account_info(),
            mint: self.mint_b.to_account_info(),
            to: self.user_token_b.to_account_info(),
            authority: self.prop_pool.to_account_info(),
        };
        CpiContext::new(self.token_program.to_account_info(), cpi_accounts)
    }
}

#[error_code]
pub enum AmmError {
    #[msg("Fee parameter below protocol minimum.")]
    FeeTooLow,
    #[msg("Fee parameter exceeds maximum allowed threshold.")]
    FeeTooHigh,
    #[msg("Zero input amount provided.")]
    ZeroAmount,
    #[msg("Mathematical overflow occurred.")]
    MathOverflow,
    #[msg("Insufficient liquidity in pool reserves.")]
    InsufficientLiquidity,
    #[msg("Excessive slippage: Output amount is below requested minimum.")]
    ExcessiveSlippage,
    #[msg("Transient reentrancy guard tripped.")]
    ReentrancyDetected,
}