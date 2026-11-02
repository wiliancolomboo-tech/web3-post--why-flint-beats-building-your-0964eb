use anchor_lang::prelude::*;
use crate::state::HookConfig;
use crate::errors::HookError;

#[derive(Accounts)]
pub struct UpdateOracle<'info> {
    pub keeper: Signer<'info>,

    #[account(
        mut,
        seeds = [b"flint_hook", hook_config.load()?.pool_id.as_ref()],
        bump
    )]
    pub hook_config: AccountLoader<'info, HookConfig>,

    /// CHECK: Validated against hook_config.oracle_feed
    pub oracle_feed: UncheckedAccount<'info>,
}

pub fn handle_update_oracle(
    ctx: Context<UpdateOracle>,
    simulated_pyth_price: u64, // In prod, deserialize Pyth Sol-SDK account
) -> Result<()> {
    let mut config = ctx.accounts.hook_config.load_mut()?;
    require_keys_eq!(config.oracle_feed, ctx.accounts.oracle_feed.key(), HookError::UnauthorizedAuthority);

    let current_ts = Clock::get()?.unix_timestamp;
    let price_delta = if config.last_oracle_price == 0 {
        0
    } else {
        (simulated_pyth_price as i128 - config.last_oracle_price as i128).unsigned_abs() as u64
    };

    // EWMA decay calculation: Vol_t = alpha * Delta + (1 - alpha) * Vol_{t-1}
    // Alpha factor = 0.2 (200,000 / 1,000,000)
    let alpha = 200_000u64;
    let scale = 1_000_000u64;
    let instant_vol = price_delta
        .checked_mul(scale)
        .ok_or(HookError::MathOverflow)?
        .checked_div(simulated_pyth_price.max(1))
        .ok_or(HookError::MathOverflow)?;

    let updated_ewma = (alpha.checked_mul(instant_vol).ok_or(HookError::MathOverflow)?)
        .checked_add(
            (scale - alpha)
                .checked_mul(config.ewma_volatility)
                .ok_or(HookError::MathOverflow)?
        )
        .ok_or(HookError::MathOverflow)?
        .checked_div(scale)
        .ok_or(HookError::MathOverflow)?;

    config.ewma_volatility = updated_ewma;
    config.last_oracle_price = simulated_pyth_price;
    config.last_update_ts = current_ts;

    Ok(())
}