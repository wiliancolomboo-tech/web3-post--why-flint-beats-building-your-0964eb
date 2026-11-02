use anchor_lang::prelude::*;

#[account(zero_copy)]
#[derive(Default)]
#[repr(C)]
pub struct HookConfig {
    /// Authority responsible for updating hook configurations (e.g., Governance/Admin)
    pub authority: Pubkey,
    /// Associated Flint Core Pool ID
    pub pool_id: Pubkey,
    /// Pyth / Switchboard Oracle Account Feed for real-time reference price
    pub oracle_feed: Pubkey,
    /// Base swap fee in basis points (1 bps = 0.01%)
    pub base_fee_bps: u16,
    /// Max allowable dynamic fee in basis points (e.g., 500 = 5.00%)
    pub max_fee_bps: u16,
    /// Exponentially Weighted Moving Average (EWMA) of volatility (scaled by 1e6)
    pub ewma_volatility: u64,
    /// Timestamp of the last oracle synchronisation
    pub last_update_ts: i64,
    /// Last recorded oracle price (scaled 1e8)
    pub last_oracle_price: u64,
    /// Invariant amplification parameter (used for concentrated curve shaping)
    pub amp_factor: u32,
    /// Nonce padding for alignment
    pub padding: [u8; 36],
}

impl HookConfig {
    pub const LEN: usize = 8 + std::mem::size_of::<HookConfig>();
}