use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};
use pyth_sdk_solana::load_price_feed_from_account_info;

declare_id!("FLiNT111111111111111111111111111111111111111");

pub const MAX_PRICE_AGE_SECONDS: u64 = 30;
pub const MAX_CONF_WIDTH_BPS: u64 = 100; // Max 1% confidence width to reject manipulated prices
pub const BPS_DENOMINATOR: u64 = 10_000;

#[program]
pub mod flint_adapter {
    use super::*;

    /// Initializes a protocol registry instance to interact with Flint's shared liquidity
    pub fn initialize_registry(
        ctx: Context<InitializeRegistry>,
        fee_tier_bps: u16,
    ) -> Result<()> {
        require!(fee_tier_bps <= 1000, FlintError::InvalidFeeTier);
        let registry = &mut ctx.accounts.registry;
        registry.authority = ctx.accounts.authority.key();
        registry.fee_tier_bps = fee_tier_bps;
        registry.is_paused = false;
        registry.total_swapped_volume = 0;
        registry.bump = ctx.bumps.registry;
        Ok(())
    }

    /// Executes an atomic swap using Flint's dynamic curve with oracle confidence protection
    pub fn execute_flint_swap(
        ctx: Context<ExecuteFlintSwap>,
        amount_in: u64,
        minimum_amount_out: u64,
        max_staleness: i64,
    ) -> Result<()> {
        require!(!ctx.accounts.registry.is_paused, FlintError::ProtocolPaused);
        require!(amount_in > 0, FlintError::ZeroAmount);

        // 1. Oracle Validation (Mitigating LVR & Stale Arbitrage)
        let clock = Clock::get()?;
        let price_feed = load_price_feed_from_account_info(&ctx.accounts.pyth_oracle)
            .map_err(|_| FlintError::InvalidOracleFeed)?;
        
        let current_price = price_feed
            .get_price_no_older_than(clock.unix_timestamp, max_staleness as u64)
            .ok_or(FlintError::StalePriceOracle)?;

        // Assert oracle confidence width <= MAX_CONF_WIDTH_BPS
        let conf_width = ((current_price.conf as u128)
            .checked_mul(BPS_DENOMINATOR as u128)
            .unwrap())
            / (current_price.price.abs() as u128);
        require!(
            conf_width <= MAX_CONF_WIDTH_BPS as u128,
            FlintError::OracleConfidenceTooWide
        );

        // 2. Derive dynamic pricing with Flint dynamic spread formula
        // Spreads scale with market volatility: Effective_Fee = Base_Fee + f(Conf)
        let effective_fee_bps = ctx.accounts.registry.fee_tier_bps as u64
            + ((conf_width as u64) / 10);
        
        let protocol_cut = (amount_in as u128)
            .checked_mul(effective_fee_bps as u128)
            .unwrap()
            .checked_div(BPS_DENOMINATOR as u128)
            .unwrap() as u64;

        let net_amount_in = amount_in
            .checked_sub(protocol_cut)
            .ok_or(FlintError::MathUnderflow)?;

        // 3. Compute dynamic expected out via constant product invariant maintained by Flint
        let vault_in_balance = ctx.accounts.vault_in.amount;
        let vault_out_balance = ctx.accounts.vault_out.amount;

        require!(vault_in_balance > 0 && vault_out_balance > 0, FlintError::InsufficientVaultLiquidity);

        // (x + dx) * (y - dy) = x * y
        // dy = (y * dx) / (x + dx)
        let amount_out = (vault_out_balance as u128)
            .checked_mul(net_amount_in as u128)
            .unwrap()
            .checked_div((vault_in_balance as u128).checked_add(net_amount_in as u128).unwrap())
            .unwrap() as u64;

        // 4. Slippage enforcement
        require!(amount_out >= minimum_amount_out, FlintError::SlippageExceeded);

        // 5. Atomic transfers via Token Program CPI
        // Transfer User -> Flint Vault In
        let transfer_in_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_token_in.to_account_info(),
                to: ctx.accounts.vault_in.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        );
        token::transfer(transfer_in_ctx, amount_in)?;

        // Transfer Flint Vault Out -> User via PDA signer seeds
        let registry_key = ctx.accounts.registry.key();
        let seeds: &[&[&[u8]]] = &[&[
            b"flint_vault",
            registry_key.as_ref(),
            ctx.accounts.mint_out.to_account_info().key.as_ref(),
            &[ctx.bumps.vault_out],
        ]];

        let transfer_out_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault_out.to_account_info(),
                to: ctx.accounts.user_token_out.to_account_info(),
                authority: ctx.accounts.vault_out.to_account_info(),
            },
            seeds,
        );
        token::transfer(transfer_out_ctx, amount_out)?;

        // 6. State updates
        let registry = &mut ctx.accounts.registry;
        registry.total_swapped_volume = registry
            .total_swapped_volume
            .checked_add(amount_in)
            .unwrap();

        emit!(FlintSwapExecuted {
            trader: ctx.accounts.user.key(),
            token_in: ctx.accounts.mint_in.key(),
            token_out: ctx.accounts.mint_out.key(),
            amount_in,
            amount_out,
            fee_assessed: protocol_cut,
            oracle_price: current_price.price,
        });

        Ok(())
    }
}

// -----------------------------------------------------------------------------
// ACCOUNT VALIDATION STRUCTS
// -----------------------------------------------------------------------------

#[derive(Accounts)]
pub struct InitializeRegistry<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + Registry::MAX_SIZE,
        seeds = [b"flint_registry", authority.key().as_ref()],
        bump
    )]
    pub registry: Account<'info, Registry>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecuteFlintSwap<'info> {
    #[account(
        mut,
        seeds = [b"flint_registry", registry.authority.as_ref()],
        bump = registry.bump
    )]
    pub registry: Account<'info, Registry>,

    pub mint_in: Account<'info, Mint>,
    pub mint_out: Account<'info, Mint>,

    #[account(
        mut,
        seeds = [b"flint_vault", registry.key().as_ref(), mint_in.key().as_ref()],
        bump,
        token::mint = mint_in,
        token::authority = vault_in,
    )]
    pub vault_in: Account<'info, TokenAccount>,

    #[account(
        mut,
        seeds = [b"flint_vault", registry.key().as_ref(), mint_out.key().as_ref()],
        bump,
        token::mint = mint_out,
        token::authority = vault_out,
    )]
    pub vault_out: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_token_in.owner == user.key(),
        constraint = user_token_in.mint == mint_in.key()
    )]
    pub user_token_in: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = user_token_out.owner == user.key(),
        constraint = user_token_out.mint == mint_out.key()
    )]
    pub user_token_out: Account<'info, TokenAccount>,

    pub user: Signer<'info>,

    /// CHECK: Validated inside handler using pyth_sdk_solana deserializer
    pub pyth_oracle: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,
}

// -----------------------------------------------------------------------------
// STATE STRUCTS
// -----------------------------------------------------------------------------

#[account]
pub struct Registry {
    pub authority: Pubkey,         // 32
    pub fee_tier_bps: u16,         // 2
    pub is_paused: bool,           // 1
    pub total_swapped_volume: u64, // 8
    pub bump: u8,                  // 1
}

impl Registry {
    pub const MAX_SIZE: usize = 32 + 2 + 1 + 8 + 1 + 64; // +64 buffer
}

// -----------------------------------------------------------------------------
// EVENTS & ERRORS
// -----------------------------------------------------------------------------

#[event]
pub struct FlintSwapExecuted {
    pub trader: Pubkey,
    pub token_in: Pubkey,
    pub token_out: Pubkey,
    pub amount_in: u64,
    pub amount_out: u64,
    pub fee_assessed: u64,
    pub oracle_price: i64,
}

#[error_code]
pub enum FlintError {
    #[msg("Provided fee tier exceeds maximum permissible cap of 1000 bps (10%).")]
    InvalidFeeTier,
    #[msg("Protocol execution is currently paused by authority.")]
    ProtocolPaused,
    #[msg("Swap input amount must be greater than zero.")]
    ZeroAmount,
    #[msg("Pyth oracle account could not be loaded or parsed.")]
    InvalidOracleFeed,
    #[msg("Oracle price data has exceeded the maximum staleness threshold.")]
    StalePriceOracle,
    #[msg("Oracle confidence interval is too wide; high volatility detected.")]
    OracleConfidenceTooWide,
    #[msg("Arithmetic underflow or overflow detected in math core.")]
    MathUnderflow,
    #[msg("Flint vault reserves are insufficient for trading pair.")]
    InsufficientVaultLiquidity,
    #[msg("Output amount violates the strict user slippage boundary.")]
    SlippageExceeded,
}