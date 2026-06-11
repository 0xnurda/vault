use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::pubkey;

/// Raydium CLMM program ID (mainnet and devnet share the same program).
pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
    pubkey!("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK");

/// Wrapped SOL (wSOL) mint address.
pub const WSOL_MINT: Pubkey =
    pubkey!("So11111111111111111111111111111111111111112");

/// Minimum token0 deposit (anti-dust, prevents rounding to 0 shares).
/// token0 = wSOL (9 decimals) → 1_000_000 lamports = 0.001 SOL (audit L4).
pub const MIN_DEPOSIT_TOKEN0: u64 = 1_000_000; // 0.001 SOL

/// Minimum token1 deposit (anti-dust).
/// token1 = USDC (6 decimals) → 1_000 micro = 0.001 USDC.
pub const MIN_DEPOSIT_TOKEN1: u64 = 1_000; // 0.001 USDC

// ─── Position range guardrails (audit M3) ────────────────────────────────────
// Bound the operator's open_position params so a compromised/malicious hot key
// cannot open an absurd one-sided or ultra-narrow position. Expressed relative
// to the pool (tick_spacing + percentages) so they work for any pool.

/// Minimum position width, in multiples of the pool's tick_spacing.
/// A position narrower than this is rejected (prevents instant out-of-range + IL).
pub const MIN_POSITION_WIDTH_SPACINGS: i32 = 8;

/// Maximum position width, in multiples of tick_spacing (capital-efficiency cap).
pub const MAX_POSITION_WIDTH_SPACINGS: i32 = 20_000;

/// Each side of the range (current→lower, current→upper) must be at least this
/// percent of the total width. 20 → current tick sits in the middle 60% of the
/// range, never at an edge (prevents near-one-sided positions).
pub const MIN_POSITION_SIDE_PCT: i64 = 20;

/// Phantom "dead shares" added to total_shares on the first deposit (audit #7).
/// These shares are never redeemable — they prevent first-depositor price manipulation
/// by ensuring price_per_share = deposit_value / (deposit_value + DEAD_SHARES) ≈ 1
/// rather than allowing an attacker to set an arbitrary price with a 1-unit deposit.
/// Negligible cost to first depositor: 1000 / deposit_value (e.g. 0.0015% for 1 SOL deposit).
pub const DEAD_SHARES: u64 = 1_000;
