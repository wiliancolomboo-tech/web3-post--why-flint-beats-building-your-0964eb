use anchor_lang::prelude::*;

pub mod errors;
pub mod instructions;
pub mod state;

use instructions::initialize_hook::*;
use instructions::update_oracle::*;
use instructions::execute_swap_hook::*;

declare_id!("FLNT8sU98sLdZg21vY33n71sE3x6QvP6d19N4V5887rL");

#[program]
pub mod flint_hook_amm {
    use super::*;

    pub fn initialize_hook(
        ctx: Context<InitializeHook>,
        base_fee_bps: u16,
        max_fee_bps: u16,
        amp_factor: u32,
    ) -> Result<()> {
        handle_initialize_hook(ctx, base_fee_bps, max_fee_bps, amp_factor)
    }

    pub fn update_oracle(
        ctx: Context<UpdateOracle>,
        simulated_pyth_price: u64,
    ) -> Result<()> {
        handle_update_oracle(ctx, simulated_pyth_price)
    }

    pub fn execute_swap_hook(
        ctx: Context<ExecuteSwapHook>,
        input: SwapCalculationInput,
    ) -> Result<SwapCalculationOutput> {
        handle_execute_swap_hook(ctx, input)
    }
}