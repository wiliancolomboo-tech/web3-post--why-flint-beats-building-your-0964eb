use anchor_lang::prelude::*;

#[error_code]
pub enum HookError {
    #[msg("Calculation overflow occurred during invariant evaluation.")]
    MathOverflow,
    #[msg("Dynamic fee exceeded configured maximum basis points.")]
    FeeExceedsMaximum,
    #[msg("Supplied slippage tolerance exceeded acceptable boundary.")]
    SlippageExceeded,
    #[msg("Invalid authority signature for hook parameter mutation.")]
    UnauthorizedAuthority,
    #[msg("Oracle price feed is stale or invalid.")]
    StaleOracleFeed,
    #[msg("Zero liquidity invariant violation.")]
    ZeroLiquidity,
}