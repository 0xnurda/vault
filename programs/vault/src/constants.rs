use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::pubkey;

/// Wrapped SOL (wSOL) mint address.
pub const WSOL_MINT: Pubkey =
    pubkey!("So11111111111111111111111111111111111111112");

/// Protocol fee = 1/PROTOCOL_FEE_DENOMINATOR of collected trading fees (10%).
/// The remaining 90% stays in treasury for depositors. Single source of truth —
/// used by collect_fees / close_position / decrease_liquidity / withdraw_from_position.
pub const PROTOCOL_FEE_DENOMINATOR: u64 = 10;

/// Minimum token1-denominated value (≈ USDC) required for the FIRST deposit (A3).
/// Prevents a tiny first deposit from eating a large share of its value to the
/// DEAD_SHARES anti-inflation phantom shares: a depositor of value D loses
/// D·DEAD_SHARES/(D+DEAD_SHARES). At D = MIN_FIRST_DEPOSIT_VALUE the loss is
/// negligible (≈0.001). Per-token anti-dust minimums (derived from decimals in
/// the deposit handlers) still apply on top of this.
pub const MIN_FIRST_DEPOSIT_VALUE: u64 = 100_000; // ~0.1 USDC (token1 units)

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
