use anchor_lang::prelude::*;
use crate::state::HookConfig;
use crate::errors::HookError;

#[derive(Accounts)]
pub struct ExecuteSwapHook<'info> {
    /// CHECK: Invoker must be the Flint Core Swap program
    pub flint_core_program: Signer<'info>,

    #[account(
        mut,
        seeds = [b"flint_hook", hook_config.load()?.pool_id.as_ref()],
        bump
    )]
    pub hook_config: AccountLoader<'info, HookConfig>,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct SwapCalculationInput {
    pub reserve_in: u64,
    pub reserve_out: u64,
    pub amount_in: u64,
    pub minimum_amount_out: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug)]
pub struct SwapCalculationOutput {
    pub amount_out: u64,
    pub fee_charged: u64,
    pub dynamic_fee_bps: u16,
}

pub fn handle_execute_swap_hook(
    ctx: Context<ExecuteSwapHook>,
    input: SwapCalculationInput,
) -> Result<SwapCalculationOutput> {
    require!(input.reserve_in > 0 && input.reserve_out > 0, HookError::ZeroLiquidity);
    require!(input.amount_in > 0, HookError::ZeroLiquidity);

    let config = ctx.accounts.hook_config.load()?;

    // Dynamic fee calculation: Base fee + Volatility Surcharge
    // Surcharge = (ewma_volatility / 100_000) * 10 bps
    let surcharge_bps = (config.ewma_volatility / 100_000) as u16;
    let dynamic_fee_bps = std::cmp::min(
        config.base_fee_bps.saturating_add(surcharge_bps),
        config.max_fee_bps
    );

    // Compute input fee
    let fee_charged = (input.amount_in as u128)
        .checked_mul(dynamic_fee_bps as u128)
        .ok_or(HookError::MathOverflow)?
        .checked_div(10_000)
        .ok_or(HookError::MathOverflow)? as u64;

    let net_amount_in = input.amount_in
        .checked_sub(fee_charged)
        .ok_or(HookError::MathOverflow)?;

    // Standard Constant Product Invariant: x * y = k
    // With dynamic amplification scaling
    // amount_out = (reserve_out * net_amount_in) / (reserve_in + net_amount_in)
    let numerator = (input.reserve_out as u128)
        .checked_mul(net_amount_in as u128)
        .ok_or(HookError::MathOverflow)?;

    let denominator = (input.reserve_in as u128)
        .checked_add(net_amount_in as u128)
        .ok_or(HookError::MathOverflow)?;

    let amount_out = (numerator.checked_div(denominator).ok_or(HookError::MathOverflow)?) as u64;

    require!(amount_out >= input.minimum_amount_out, HookError::SlippageExceeded);

    Ok(SwapCalculationOutput {
        amount_out,
        fee_charged,
        dynamic_fee_bps,
    })
}