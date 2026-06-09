use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::pubkey;

/// Raydium CLMM program ID (mainnet and devnet share the same program).
pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
    pubkey!("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK");

/// Wrapped SOL (wSOL) mint address.
pub const WSOL_MINT: Pubkey =
    pubkey!("So11111111111111111111111111111111111111112");

/// Minimum token0 deposit (anti-dust, prevents rounding to 0 shares)
pub const MIN_DEPOSIT_TOKEN0: u64 = 1_000; // raw units

/// Minimum token1 deposit (anti-dust)
pub const MIN_DEPOSIT_TOKEN1: u64 = 1_000; // raw units

/// Phantom "dead shares" added to total_shares on the first deposit (audit #7).
/// These shares are never redeemable — they prevent first-depositor price manipulation
/// by ensuring price_per_share = deposit_value / (deposit_value + DEAD_SHARES) ≈ 1
/// rather than allowing an attacker to set an arbitrary price with a 1-unit deposit.
/// Negligible cost to first depositor: 1000 / deposit_value (e.g. 0.0015% for 1 SOL deposit).
pub const DEAD_SHARES: u64 = 1_000;
