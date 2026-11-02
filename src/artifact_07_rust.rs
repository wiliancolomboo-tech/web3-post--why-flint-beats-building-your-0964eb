use anchor_lang::prelude::*;
use crate::state::HookConfig;
use crate::errors::HookError;

#[derive(Accounts)]
pub struct InitializeHook<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    /// CHECK: Pool ID is validated against Flint Core engine upon registration
    pub pool_id: UncheckedAccount<'info>,

    /// CHECK: External verified price feed account
    pub oracle_feed: UncheckedAccount<'info>,

    #[account(
        init,
        payer = authority,
        space = HookConfig::LEN,
        seeds = [b"flint_hook", pool_id.key().as_ref()],
        bump
    )]
    pub hook_config: AccountLoader<'info, HookConfig>,

    pub system_program: Program<'info, System>,
}

pub fn handle_initialize_hook(
    ctx: Context<InitializeHook>,
    base_fee_bps: u16,
    max_fee_bps: u16,
    amp_factor: u32,
) -> Result<()> {
    require!(base_fee_bps <= max_fee_bps, HookError::FeeExceedsMaximum);
    require!(max_fee_bps <= 1000, HookError::FeeExceedsMaximum); // Maximum 10% safety cap

    let mut config = ctx.accounts.hook_config.load_init()?;
    config.authority = ctx.accounts.authority.key();
    config.pool_id = ctx.accounts.pool_id.key();
    config.oracle_feed = ctx.accounts.oracle_feed.key();
    config.base_fee_bps = base_fee_bps;
    config.max_fee_bps = max_fee_bps;
    config.ewma_volatility = 0;
    config.last_update_ts = Clock::get()?.unix_timestamp;
    config.last_oracle_price = 0;
    config.amp_factor = amp_factor;

    Ok(())
}