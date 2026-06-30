use anchor_lang::prelude::*;
use crate::constants::{
    DEAD_SHARES, MAX_POSITION_WIDTH_SPACINGS, MIN_POSITION_SIDE_PCT, MIN_POSITION_WIDTH_SPACINGS,
};
use crate::errors::VaultError;

// Fixed-width integers for bit-exact CLMM math. Isolated in their own module so
// the construct_uint! macro doesn't collide with anchor_lang's prelude Result/`?`.
mod bignum {
    uint::construct_uint! {
        pub struct U256(4);
    }
    uint::construct_uint! {
        pub struct U128(2);
    }
}
use bignum::{U128, U256};

/// Validate that an open_position range is sane (audit M3). Rejects ranges that
/// are too narrow, too wide, not straddling the current price, or skewed so the
/// current tick sits at an edge (near-one-sided). Pool-agnostic: thresholds are
/// relative to tick_spacing and the range width.
pub fn validate_position_range(
    tick_lower: i32,
    tick_upper: i32,
    tick_current: i32,
    tick_spacing: i32,
) -> Result<()> {
    // A5: bound ticks to the CLMM's valid range first. Without this, garbage
    // ticks (e.g. i32::MIN/MAX) would overflow `tick_upper - tick_lower` and
    // panic/revert (overflow-checks are on).
    require!(
        tick_lower >= MIN_TICK && tick_upper <= MAX_TICK,
        VaultError::InvalidTickRange
    );
    require!(tick_lower < tick_upper, VaultError::InvalidTickRange);
    let width = tick_upper - tick_lower;

    // (a) width within [MIN, MAX] × tick_spacing
    require!(
        width >= MIN_POSITION_WIDTH_SPACINGS.saturating_mul(tick_spacing)
            && width <= MAX_POSITION_WIDTH_SPACINGS.saturating_mul(tick_spacing),
        VaultError::InvalidPositionRange
    );

    // (b) current price strictly inside the range (two-sided / active)
    require!(
        tick_lower < tick_current && tick_current < tick_upper,
        VaultError::InvalidPositionRange
    );

    // (c) centering: each side ≥ MIN_POSITION_SIDE_PCT% of width
    let left = (tick_current - tick_lower) as i64;
    let right = (tick_upper - tick_current) as i64;
    let min_side = (width as i64).saturating_mul(MIN_POSITION_SIDE_PCT) / 100;
    require!(left >= min_side && right >= min_side, VaultError::InvalidPositionRange);

    Ok(())
}

/// Main Vault account - stores global state.
/// One vault per Raydium CLMM pool; PDA seeds = [b"vault", pool_id].
#[account]
#[derive(Default)]
pub struct Vault {
    /// Admin who can manage positions and vault settings
    pub admin: Pubkey,
    /// SPL Token mint for vault shares
    pub share_mint: Pubkey,
    /// Raydium CLMM pool used for this vault (price source + position pool)
    /// Also part of the vault PDA seeds → immutable after initialization
    pub pool_id: Pubkey,
    /// Mint of token0 in this vault (e.g. wSOL for SOL/USDC)
    pub token0_mint: Pubkey,
    /// Mint of token1 in this vault (e.g. USDC for SOL/USDC)
    pub token1_mint: Pubkey,
    /// PDA that holds token0 (e.g. wSOL)
    pub token0_treasury: Pubkey,
    /// PDA that holds token1 (e.g. USDC)
    pub token1_treasury: Pubkey,
    /// Wallet that receives protocol fees (10% of position trading fees)
    pub protocol_wallet: Pubkey,
    /// Total shares minted (6 decimals)
    pub total_shares: u64,
    /// Total token0 in treasury — includes accumulated_protocol_fees_token0
    pub treasury_token0: u64,
    /// Total token1 in treasury — includes accumulated_protocol_fees_token1
    pub treasury_token1: u64,
    /// Decimals of token0 (e.g. 9 for SOL)
    pub token0_decimals: u8,
    /// Decimals of token1 (e.g. 6 for USDC)
    pub token1_decimals: u8,
    /// Vault PDA bump
    pub bump: u8,
    /// token0 treasury PDA bump
    pub token0_treasury_bump: u8,
    /// token1 treasury PDA bump
    pub token1_treasury_bump: u8,
    /// Share mint authority bump
    pub share_mint_bump: u8,
    /// Active position NFT mint (Pubkey::default() if no position)
    pub position_mint: Pubkey,
    /// Whether there's an active CLMM position
    pub has_active_position: bool,
    /// token0 deposited into active position (approximation, for analytics)
    pub position_token0: u64,
    /// token1 deposited into active position (approximation, for analytics)
    pub position_token1: u64,
    /// Liquidity in position (from Raydium PersonalPositionState)
    pub position_liquidity: u128,
    /// Lower tick of position
    pub position_tick_lower: i32,
    /// Upper tick of position
    pub position_tick_upper: i32,
    /// Whether the vault is paused (user deposits/withdrawals disabled)
    pub is_paused: bool,
    /// True during rebalance (between close_position and open_position).
    pub is_rebalancing: bool,
    /// Pending admin for two-step admin transfer
    pub pending_admin: Pubkey,
    /// Accumulated token0 protocol fees not yet extracted (excluded from TVL)
    pub accumulated_protocol_fees_token0: u64,
    /// Accumulated token1 protocol fees not yet extracted (excluded from TVL)
    pub accumulated_protocol_fees_token1: u64,
    /// Unix timestamp when the current rebalance started (set by close_position,
    /// cleared by open_position and cancel_rebalance). Kept for observability; it is
    /// no longer a withdrawal gate — since A1, `withdraw` is always available when
    /// there is no active position (all funds in treasury).
    pub rebalance_started_at: i64,
    /// Hot operator key for automated operations (rebalance, collect_fees, swaps).
    /// Separate from admin: admin (cold/multisig) sets config; operator (hot bot)
    /// runs day-to-day ops within on-chain guardrails and CANNOT move funds out.
    pub operator: Pubkey,
    /// Unix timestamp when the current swap rate-limit window started (audit H1).
    pub swap_window_start: i64,
    /// token1-denominated swap volume accumulated in the current window (audit H1).
    pub swap_volume_in_window: u64,
    /// Slot of the last collect_fees call (audit M-1). decrease_liquidity and
    /// close_position require this to equal the current slot, forcing the keeper
    /// to harvest fees in the same transaction so Raydium's CPI-computed accrued
    /// fees don't get swept into principal untaxed.
    pub last_fee_collection_slot: u64,
    /// Unix timestamp of the last treasury swap (audit M-3). Enforces a cooldown
    /// between swaps so a compromised operator cannot drain the whole per-window
    /// volume cap in a single block — the limit is spread over time, giving the
    /// admin a window to detect and rotate the operator key.
    pub last_swap_at: i64,
}

impl Vault {
    pub const LEN: usize = 8   + // discriminator
        32 + // admin
        32 + // share_mint
        32 + // pool_id
        32 + // token0_mint
        32 + // token1_mint
        32 + // token0_treasury
        32 + // token1_treasury
        32 + // protocol_wallet
        8  + // total_shares
        8  + // treasury_token0
        8  + // treasury_token1
        1  + // token0_decimals
        1  + // token1_decimals
        1  + // bump
        1  + // token0_treasury_bump
        1  + // token1_treasury_bump
        1  + // share_mint_bump
        32 + // position_mint
        1  + // has_active_position
        8  + // position_token0
        8  + // position_token1
        16 + // position_liquidity (u128)
        4  + // position_tick_lower (i32)
        4  + // position_tick_upper (i32)
        1  + // is_paused
        1  + // is_rebalancing
        32 + // pending_admin
        8  + // accumulated_protocol_fees_token0
        8  + // accumulated_protocol_fees_token1
        8  + // rebalance_started_at
        32 + // operator (Pubkey)
        8  + // swap_window_start (i64)
        8  + // swap_volume_in_window (u64)
        8  + // last_fee_collection_slot (u64) — audit M-1
        8;   // last_swap_at (i64) — audit M-3 (consumed remaining padding; LEN unchanged)

    /// TVL using real-time position amounts (computed from pool + position accounts).
    /// Used by deposit_token0 / deposit_token1 for accurate share pricing.
    pub fn calculate_tvl_with_position(
        &self,
        token0_price_in_token1: u64,
        position_token0: u64,
        position_token1: u64,
    ) -> u64 {
        let user_token0 = self.treasury_token0
            .saturating_add(position_token0)
            .saturating_sub(self.accumulated_protocol_fees_token0);
        let user_token1 = self.treasury_token1
            .saturating_add(position_token1)
            .saturating_sub(self.accumulated_protocol_fees_token1);

        let token0_value = (user_token0 as u128)
            .checked_mul(token0_price_in_token1 as u128)
            .and_then(|v| v.checked_div(10u128.pow(self.token0_decimals as u32)))
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0);

        token0_value.saturating_add(user_token1)
    }

    /// Shares to mint for a deposit. First depositor gets 1 share per 1 token1 unit.
    pub fn calculate_shares_to_mint(
        &self,
        deposit_value: u64,
        current_tvl: u64,
    ) -> Result<u64> {
        // Fresh start: empty vault, zero TVL, or only phantom dead-shares remain
        // (everyone withdrew → total_shares stuck at DEAD_SHARES, TVL ≈ dust).
        // Without this, deposit_value × DEAD_SHARES / dust overflows u64 and the
        // vault becomes permanently un-depositable (audit M2).
        if self.total_shares <= DEAD_SHARES || current_tvl == 0 {
            return Ok(deposit_value);
        }
        let proportional = (deposit_value as u128)
            .checked_mul(self.total_shares as u128)
            .and_then(|v| v.checked_div(current_tvl as u128));
        match proportional {
            Some(v) if v <= u64::MAX as u128 => Ok(v as u64),
            // Edge (audit L-2): the vault collapsed to dust TVL while shares
            // remain (e.g. DEAD_SHARES + a tiny stuck holder), so the
            // proportional mint exceeds u64 and would revert every deposit.
            // The dust holders are economically negligible — fall back to
            // fresh-start pricing instead of locking the vault out of deposits.
            _ => Ok(deposit_value),
        }
    }

    /// Convert token0 amount to token1 units using pool price.
    pub fn token0_to_token1(&self, amount: u64, token0_price_in_token1: u64) -> u64 {
        (amount as u128)
            .checked_mul(token0_price_in_token1 as u128)
            .and_then(|v| v.checked_div(10u128.pow(self.token0_decimals as u32)))
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0)
    }

    /// True if `key` may run operational actions (rebalance, collect_fees, swap).
    /// Both the hot operator and the admin are authorized.
    pub fn is_operator(&self, key: &Pubkey) -> bool {
        *key == self.operator || *key == self.admin
    }
}

// ─── Raydium CLMM pool price helpers ───────────────────────────────────────
//
// These offsets (and the AccountLoader<PoolState> reads elsewhere) were verified
// against LIVE pool accounts on 2026-06-15: the on-chain PoolState is 1544 bytes
// on BOTH mainnet (CAMMCzo… pool 3ucNos…) and devnet (DRay… pool 9Pkg…), which
// equals raydium-clmm-cpi's PoolState::LEN exactly — byte-for-byte, with these
// fields at the offsets below. The `dynamic_fee_info` added in raydium-clmm
// `main` is NOT in the deployed programs, so there is no layout drift and no
// change is needed here. (Contrast ObservationState, which Raydium shrank to a
// 100-slot cumulative-tick layout — that one we had to re-parse from raw bytes.)
// The crate rev is pinned in Cargo.toml so this layout can't shift unnoticed.

/// Convert Raydium CLMM `sqrt_price_x64` (Q64.64) to token0 price in token1 units.
///
/// Returns the price of 1 token0 (at token0_decimals) expressed in token1 units
/// (with token1_decimals decimal places).
///
/// `token0_is_pool_token0`: true if vault.token0_mint == pool.token_mint_0.
/// When false (inverted pool), token0 and token1 are swapped relative to pool ordering.
///
/// Uses >>32 intermediate to avoid u128 overflow when squaring.
pub fn sqrt_price_to_price(
    sqrt_price_x64: u128,
    token0_is_pool_token0: bool,
    token0_decimals: u8,
    _token1_decimals: u8,
) -> Option<u64> {
    let a = sqrt_price_x64 >> 32;
    if a == 0 {
        return None;
    }
    let price_q64 = a.checked_mul(a)?; // ≈ (raw_token1/raw_token0) * 2^64

    if token0_is_pool_token0 {
        // token0_price_in_token1 = price_q64 * 10^token0_decimals >> 64
        let scale = 10u128.pow(token0_decimals as u32);
        let raw = price_q64.checked_mul(scale)?;
        u64::try_from(raw >> 64).ok()
    } else {
        // pool token0 = vault token1, pool token1 = vault token0 (inverted)
        // token0_price_in_token1 = 2^64 * 10^token0_decimals / price_q64
        let scale = 10u128.pow(token0_decimals as u32);
        let numerator = scale << 64;
        u64::try_from(numerator.checked_div(price_q64)?).ok()
    }
}

// ─── CLMM position amount math ─────────────────────────────────────────────

/// Convert tick index to sqrt_price_x64 (Q64.64) — BIT-EXACT port of Raydium's
/// on-chain `tick_math::get_sqrt_price_at_tick` (audit H-3).
///
/// Verified to match `SqrtPriceMath.getSqrtPriceX64FromTick` (Raydium SDK = the
/// on-chain program) to the ULP across the entire tick range. Differs from the
/// Uniswap Q96 table: Raydium uses 64-bit-truncated constants, starts the even
/// case at 2^64, shifts by 64 (not 128), and inverts positive ticks via
/// U128::MAX / ratio with NO final rounding.
pub fn get_sqrt_price_at_tick(tick: i32) -> u128 {
    let abs_tick = tick.unsigned_abs();

    let mut ratio = if abs_tick & 0x1 != 0 {
        U128::from(0xfffcb933bd6fb800u128)
    } else {
        U128::from(1u128) << 64
    };
    if abs_tick & 0x2     != 0 { ratio = (ratio * U128::from(0xfff97272373d4000u128)) >> 64 }
    if abs_tick & 0x4     != 0 { ratio = (ratio * U128::from(0xfff2e50f5f657000u128)) >> 64 }
    if abs_tick & 0x8     != 0 { ratio = (ratio * U128::from(0xffe5caca7e10f000u128)) >> 64 }
    if abs_tick & 0x10    != 0 { ratio = (ratio * U128::from(0xffcb9843d60f7000u128)) >> 64 }
    if abs_tick & 0x20    != 0 { ratio = (ratio * U128::from(0xff973b41fa98e800u128)) >> 64 }
    if abs_tick & 0x40    != 0 { ratio = (ratio * U128::from(0xff2ea16466c9b000u128)) >> 64 }
    if abs_tick & 0x80    != 0 { ratio = (ratio * U128::from(0xfe5dee046a9a3800u128)) >> 64 }
    if abs_tick & 0x100   != 0 { ratio = (ratio * U128::from(0xfcbe86c7900bb000u128)) >> 64 }
    if abs_tick & 0x200   != 0 { ratio = (ratio * U128::from(0xf987a7253ac65800u128)) >> 64 }
    if abs_tick & 0x400   != 0 { ratio = (ratio * U128::from(0xf3392b0822bb6000u128)) >> 64 }
    if abs_tick & 0x800   != 0 { ratio = (ratio * U128::from(0xe7159475a2caf000u128)) >> 64 }
    if abs_tick & 0x1000  != 0 { ratio = (ratio * U128::from(0xd097f3bdfd2f2000u128)) >> 64 }
    if abs_tick & 0x2000  != 0 { ratio = (ratio * U128::from(0xa9f746462d9f8000u128)) >> 64 }
    if abs_tick & 0x4000  != 0 { ratio = (ratio * U128::from(0x70d869a156f31c00u128)) >> 64 }
    if abs_tick & 0x8000  != 0 { ratio = (ratio * U128::from(0x31be135f97ed3200u128)) >> 64 }
    if abs_tick & 0x10000 != 0 { ratio = (ratio * U128::from(0x9aa508b5b85a500u128)) >> 64 }
    if abs_tick & 0x20000 != 0 { ratio = (ratio * U128::from(0x5d6af8dedc582cu128)) >> 64 }
    if abs_tick & 0x40000 != 0 { ratio = (ratio * U128::from(0x2216e584f5fau128)) >> 64 }

    if tick > 0 {
        ratio = U128::MAX / ratio;
    }

    ratio.as_u128()
}

/// token0 amount from a liquidity range — BIT-EXACT vs Raydium (audit H-3).
///
/// Formula (Uniswap-v3 / Raydium CLMM, round-down):
///   amount0 = (L << 64) · (√P_hi − √P_lo) / √P_hi / √P_lo   (two nested floors)
///
/// Computed in U256 so the (L<<64)·diff intermediate (≤ 2¹⁹²) never truncates.
/// This matches `LiquidityMath.getAmountsFromLiquidity(..., roundUp=false)` to the
/// raw unit across the full tick range (verified by the SDK diff-test).
pub fn get_amount_0_delta(sqrt_lo: u128, sqrt_hi: u128, liquidity: u128) -> u64 {
    if sqrt_lo == 0 || sqrt_hi <= sqrt_lo || liquidity == 0 {
        return 0;
    }
    let diff = sqrt_hi - sqrt_lo;
    // num = (L << 64) · diff   (fits u256: result is bounded by u64 for valid inputs)
    let num = (U256::from(liquidity) << 64u32) * U256::from(diff);
    // two nested floor divisions, exactly as Raydium's mulDiv(num, .., sqrtB) / sqrtA
    let amount = num / U256::from(sqrt_hi) / U256::from(sqrt_lo);
    if amount > U256::from(u64::MAX) {
        u64::MAX
    } else {
        amount.as_u64()
    }
}

/// token1 amount from a liquidity range — BIT-EXACT vs Raydium (audit H-3).
///
/// Formula: amount1 = L · (√P_hi − √P_lo) / 2⁶⁴   (round-down)
/// Computed in U256 to avoid the u128-overflow fallback entirely.
pub fn get_amount_1_delta(sqrt_lo: u128, sqrt_hi: u128, liquidity: u128) -> u64 {
    if sqrt_hi <= sqrt_lo || liquidity == 0 {
        return 0;
    }
    let diff = sqrt_hi - sqrt_lo;
    let amount = (U256::from(liquidity) * U256::from(diff)) >> 64u32;
    if amount > U256::from(u64::MAX) {
        u64::MAX
    } else {
        amount.as_u64()
    }
}

/// Compute the real token0 and token1 amounts held in a CLMM position at current price.
///
/// Returns `(token0_amount, token1_amount)` in raw units (lamports / micro-units).
///
/// `token0_is_pool_token0`: true when vault.token0_mint == pool.token_mint_0.
pub fn calculate_position_amounts(
    sqrt_price_x64: u128,
    tick_current: i32,
    tick_lower: i32,
    tick_upper: i32,
    liquidity: u128,
    token0_is_pool_token0: bool,
) -> (u64, u64) {
    if liquidity == 0 {
        return (0, 0);
    }

    let sqrt_lower = get_sqrt_price_at_tick(tick_lower);
    let sqrt_upper = get_sqrt_price_at_tick(tick_upper);

    let (amount_pool_token0, amount_pool_token1) = if tick_current < tick_lower {
        (get_amount_0_delta(sqrt_lower, sqrt_upper, liquidity), 0u64)
    } else if tick_current >= tick_upper {
        (0u64, get_amount_1_delta(sqrt_lower, sqrt_upper, liquidity))
    } else {
        (
            get_amount_0_delta(sqrt_price_x64, sqrt_upper, liquidity),
            get_amount_1_delta(sqrt_lower, sqrt_price_x64, liquidity),
        )
    };

    // Map pool token0/token1 → vault token0/token1
    if token0_is_pool_token0 {
        (amount_pool_token0, amount_pool_token1)
    } else {
        (amount_pool_token1, amount_pool_token0)
    }
}

/// User deposit record
#[account]
#[derive(Default)]
pub struct UserDeposit {
    pub user: Pubkey,
    pub vault: Pubkey,
    pub shares: u64,
    pub total_deposited_token0: u64,
    pub total_deposited_token1: u64,
    /// DEPRECATED / unused. Kept to preserve the on-chain UserDeposit byte layout
    /// (removing it would shift every existing depositor account → requires migration).
    pub total_withdrawn_value: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub bump: u8,
}

impl UserDeposit {
    // 8 disc + user(32) + vault(32) + shares(8) + deposited_token0(8)
    // + deposited_token1(8) + withdrawn_value(8) + created_at(8) + updated_at(8) + bump(1) = 121
    pub const LEN: usize = 8 + 32 + 32 + 8 + 8 + 8 + 8 + 8 + 8 + 1;
}

/// Seeds for PDAs
pub mod seeds {
    pub const VAULT: &[u8] = b"vault";
    pub const TOKEN0_TREASURY: &[u8] = b"token0_treasury";
    pub const TOKEN1_TREASURY: &[u8] = b"token1_treasury";
    pub const SHARE_MINT: &[u8] = b"share_mint";
    pub const USER_DEPOSIT: &[u8] = b"user_deposit";
}


// ─── Flash-loan price manipulation protection ─────────────────────────────────

/// Maximum allowed deviation between spot sqrt_price and a ≥30-second-old
/// reference observation, for DEPOSITS. 250 bps on sqrt ≈ 5% price deviation
/// (audit M-2, widened from 75/1.5%). The 1.5% band caused false reverts during
/// normal SOL volatility over the 30-s window; 5% still makes a flash-loan
/// sandwich uneconomical on a deep pool (moving price 5% costs far more in
/// slippage than any share-mint gain) while letting legitimate deposits through.
pub const MAX_SQRT_DEVIATION_BPS: u128 = 250;

/// Softer deviation band for WITHDRAWALS (audit M-2). 500 bps on sqrt ≈ 10%
/// price. Withdrawals get a wider band so volatility can never lock a user's
/// funds in — being unable to exit is worse UX than the marginal extra
/// manipulation room, and a withdrawer who moves the pool to extract more
/// burns most of the gain in slippage + the protocol fee.
pub const MAX_SQRT_DEVIATION_WITHDRAW_BPS: u128 = 500;

/// Minimum age of the reference observation to be meaningful.
const TWAP_MIN_AGE_SECS: u32 = 30;

// ─── Raydium CLMM ObservationState (CURRENT on-chain layout) ──────────────────
//
// Raydium replaced the old 1000-slot oracle (which stored per-slot
// sqrt_price_x64) with a compact 100-slot cumulative-tick oracle. Both mainnet
// (CAMMCzo…) and devnet (DRay…) pools now use THIS layout (account = 4483 bytes),
// while the raydium-clmm-cpi crate still ships the old struct — so we parse the
// account bytes directly instead of zero-copy-loading the crate type.
//
// Account data (8-byte anchor discriminator first), all little-endian:
//   8   discriminator
//   +0  initialized: bool (1)
//   +1  recent_epoch: u64 (8)
//   +9  observation_index: u16 (2)
//   +11 pool_id: Pubkey (32)
//   +43 observations: [Observation; 100]
//   Observation = block_timestamp: u32 (4) | tick_cumulative: i64 (8) | padding (32) = 44
const OBS_DISCRIMINATOR: usize = 8;
const OBS_INITIALIZED_OFFSET: usize = OBS_DISCRIMINATOR;            // 8
const OBS_POOL_ID_OFFSET: usize = OBS_DISCRIMINATOR + 1 + 8 + 2;    // 19
const OBS_OBSERVATIONS_OFFSET: usize = OBS_POOL_ID_OFFSET + 32;     // 51
const OBS_ENTRY_LEN: usize = 44;
const OBS_NUM: usize = 100;

/// Valid tick bounds for Raydium CLMM (= Uniswap v3 bounds).
pub const MIN_TICK: i32 = -443636;
pub const MAX_TICK: i32 = 443636;

/// Read `pool_id` from a raw ObservationState account.
pub fn observation_pool_id(obs_data: &[u8]) -> Option<Pubkey> {
    let bytes: [u8; 32] = obs_data
        .get(OBS_POOL_ID_OFFSET..OBS_POOL_ID_OFFSET + 32)?
        .try_into()
        .ok()?;
    Some(Pubkey::new_from_array(bytes))
}

/// Verify the current pool sqrt_price_x64 (spot) has not been manipulated by a
/// flash-loan sandwich, using Raydium's cumulative-tick TWAP oracle.
///
/// A ≥30 s time-weighted-average tick is derived from two observations'
/// `tick_cumulative` checkpoints, converted to a sqrt price via the bit-exact
/// tick table, and compared with spot. A same-tx manipulation moves spot a lot
/// but barely moves the long-window TWAP, so it is detected.
///
/// When `require_reference` is true (vault holds funds), a missing ≥30 s window
/// is FAIL-SAFE: revert (audit H3). When false (first deposit into an empty
/// vault) a missing reference is allowed — nothing to manipulate yet.
pub fn check_price_not_manipulated(
    sqrt_price_x64: u128,
    obs_data: &[u8],
    require_reference: bool,
    max_deviation_bps: u128,
) -> Result<()> {
    let ref_sqrt = match reference_sqrt_price(obs_data) {
        Some(s) => s,
        None => {
            require!(!require_reference, VaultError::OracleUnavailable);
            return Ok(());
        }
    };

    // Both values are Q64.64 — same format, direct comparison is valid.
    require!(
        sqrt_within_deviation(sqrt_price_x64, ref_sqrt, max_deviation_bps),
        VaultError::PriceManipulationDetected
    );

    Ok(())
}

/// True iff `spot_sqrt` is within `max_deviation_bps` (on the sqrt price) of
/// `ref_sqrt`: |spot - ref| * 10_000 <= ref * bps. Saturating, overflow-safe.
pub fn sqrt_within_deviation(spot_sqrt: u128, ref_sqrt: u128, max_deviation_bps: u128) -> bool {
    let deviation = if spot_sqrt > ref_sqrt {
        spot_sqrt - ref_sqrt
    } else {
        ref_sqrt - spot_sqrt
    };
    deviation.saturating_mul(10_000) <= ref_sqrt.saturating_mul(max_deviation_bps)
}

// ─── Admin-swap drain protection (audit #4) ───────────────────────────────────

/// Max acceptable slippage for a treasury swap, in bps. 100 = 1% (audit H1).
/// Tightened from 200. Covers normal 30-s drift + pool fee on a deep pool, but
/// halves the per-swap value an attacker can bleed via self-sandwich.
pub const MAX_SWAP_SLIPPAGE_BPS: u128 = 100;

/// Rate-limit window for treasury swaps (audit H1). 1 hour.
pub const SWAP_WINDOW_SECS: i64 = 3_600;

/// Max swap volume per window as a fraction of treasury value, in bps.
/// 10000 = 100% of treasury per hour (audit M-3, tightened from 15000/150%).
/// Still covers two full one-sided 50/50 rebalances per hour, but cuts the
/// worst-case self-sandwich drain to ≈1%/hr (cap × MAX_SWAP_SLIPPAGE_BPS).
pub const MAX_SWAP_VOLUME_BPS: u128 = 10_000;

/// Minimum seconds between two treasury swaps (audit M-3). A legitimate
/// rebalance performs a single swap, so this never blocks normal operation;
/// it only stops a compromised operator from front-loading the entire window
/// cap in one block, spreading any drain over time so the admin can react.
pub const SWAP_COOLDOWN_SECS: i64 = 60;

/// Manipulation-resistant reference sqrt_price (Q64.64) from Raydium's
/// cumulative-tick oracle: the time-weighted average tick over a ≥30 s window
/// ending at the newest observation, converted to sqrt price.
///
/// A flash manipulation in the current block updates only the newest checkpoint
/// for a tiny dt, so its weight in a ≥30 s window is negligible — the TWAP stays
/// close to the honest price while spot moves. Returns None when the buffer has
/// no two observations spanning ≥30 s (caller decides fail-safe vs allow).
///
/// Reads one `Observation` per slot from the raw account (block_timestamp: u32,
/// tick_cumulative: i64). Two passes over 100 slots — cheap.
pub fn reference_sqrt_price(obs_data: &[u8]) -> Option<u128> {
    if *obs_data.get(OBS_INITIALIZED_OFFSET)? == 0 {
        return None;
    }
    let read = |i: usize| -> Option<(i64, i64)> {
        let base = OBS_OBSERVATIONS_OFFSET + i * OBS_ENTRY_LEN;
        let ts = u32::from_le_bytes(obs_data.get(base..base + 4)?.try_into().ok()?) as i64;
        if ts == 0 {
            return None; // empty slot
        }
        let cum = i64::from_le_bytes(obs_data.get(base + 4..base + 12)?.try_into().ok()?);
        Some((ts, cum))
    };

    // Pass 1: newest checkpoint (max block_timestamp).
    let mut newest_ts = 0i64;
    let mut newest_cum = 0i64;
    for i in 0..OBS_NUM {
        if let Some((ts, cum)) = read(i) {
            if ts > newest_ts {
                newest_ts = ts;
                newest_cum = cum;
            }
        }
    }
    if newest_ts == 0 {
        return None;
    }

    // Pass 2: the most recent checkpoint that is ≥30 s BEFORE the newest one.
    // (Relative to newest, not wall-clock, so a quiet pool still yields a window.)
    let mut old_ts = 0i64;
    let mut old_cum = 0i64;
    for i in 0..OBS_NUM {
        if let Some((ts, cum)) = read(i) {
            if newest_ts - ts >= TWAP_MIN_AGE_SECS as i64 && ts > old_ts {
                old_ts = ts;
                old_cum = cum;
            }
        }
    }
    if old_ts == 0 || newest_ts <= old_ts {
        return None; // no ≥30 s window available
    }

    // Time-weighted average tick over [old, newest], then → sqrt price.
    let dt = newest_ts - old_ts;
    let avg_tick = (newest_cum - old_cum) / dt;
    let tick = avg_tick.clamp(MIN_TICK as i64, MAX_TICK as i64) as i32;
    Some(get_sqrt_price_at_tick(tick))
}

/// Compute the minimum acceptable swap output (raw units) from a reference
/// sqrt_price (TWAP), enforcing MAX_SWAP_SLIPPAGE_BPS.
///
/// Pool price P = raw_token1 / raw_token0 = (sqrt_x64 / 2^64)^2.
/// - input_is_pool_token0 → selling token0 for token1: out = amount_in × P
/// - else                 → selling token1 for token0: out = amount_in / P
///
/// Overflow-safe: uses (sqrt >> 32)^2 = P × 2^64 then shifts.
pub fn swap_min_out_floor(
    ref_sqrt_x64: u128,
    amount_in: u64,
    input_is_pool_token0: bool,
) -> Option<u64> {
    let half = ref_sqrt_x64 >> 32;          // sqrt(P) × 2^32
    let price_x64 = half.checked_mul(half)?; // P × 2^64

    let expected: u128 = if input_is_pool_token0 {
        // out_token1 = amount_in × P = amount_in × price_x64 / 2^64
        (amount_in as u128).checked_mul(price_x64)? >> 64
    } else {
        // out_token0 = amount_in / P = (amount_in << 64) / price_x64
        ((amount_in as u128).checked_shl(64)?).checked_div(price_x64)?
    };

    let floor = expected
        .checked_mul(10_000u128.checked_sub(MAX_SWAP_SLIPPAGE_BPS)?)?
        .checked_div(10_000)?;
    u64::try_from(floor).ok()
}

/// Value `amount` (raw) of a pool token, in pool-token1 units, using the
/// reference sqrt price. If the amount is pool-token0, multiply by price P;
/// if it is already pool-token1, return as-is. Used for the swap rate-limit
/// so volume and treasury are compared in one consistent unit (audit H1).
pub fn value_in_token1(ref_sqrt_x64: u128, amount: u64, amount_is_pool_token0: bool) -> Option<u128> {
    if !amount_is_pool_token0 {
        return Some(amount as u128);
    }
    // token1 = amount_token0 × P, where P = (sqrt/2^64)^2 = price_x64 / 2^64.
    let half = ref_sqrt_x64 >> 32;          // sqrt(P) × 2^32
    let price_x64 = half.checked_mul(half)?; // P × 2^64
    (amount as u128).checked_mul(price_x64).map(|v| v >> 64)
}

// ─── NEW-3: reward-recipient validation for decrease_liquidity_v2 CPI ──────────
//
// Raydium's `decrease_liquidity_v2` collects LM rewards into a CALLER-SUPPLIED
// `recipient_token_account`, only checking `recipient.mint == reward_vault.mint`
// and `reward_vault == pool.reward_infos[i].token_vault` — it does NOT verify the
// recipient's OWNER. The reward accounts ride in `remaining_accounts`, grouped per
// initialized reward in the order:
//   [ reward_token_vault, recipient_token_account, (reward_vault_mint — Token-2022 only) ]
// `recipient_token_account` is always at group index [1]. Group size is per-call,
// pool-wide (2 for SPL rewards, 3 if ANY reward mint is Token-2022 — Raydium's
// `reward_group_account_num`). Without validation a caller of the permissionless
// `withdraw_from_position` (or any operator path) could pass their own ATA at
// index [1] and siphon the whole position's accrued rewards, which must instead
// land in the vault-owned reward ATA later swept by the admin-only `extract_rewards`.
//
// `reward_group_size` derives the group size robustly from the actual lengths
// rather than hardcoding: given the number of INITIALIZED pool rewards
// (`num_pool_rewards`, counted from `pool_state.reward_infos`) it requires that
// `remaining.len()` divides evenly by it. This stays aligned with Raydium's rule:
// every group has the same width, so `len / num_rewards` is the per-reward width
// (2 or 3) for THIS call. Empty `remaining` → no rewards collected → no groups.
// Fail-closed: any inconsistency (no rewards but accounts present, non-divisible
// length, zero-width group) returns None so the caller reverts.
pub fn reward_group_size(remaining_len: usize, num_pool_rewards: usize) -> Option<usize> {
    if remaining_len == 0 {
        return Some(0); // nothing to collect — no-op
    }
    if num_pool_rewards == 0 {
        return None; // accounts supplied but pool has no initialized rewards
    }
    if remaining_len % num_pool_rewards != 0 {
        return None; // ragged grouping — cannot trust the layout
    }
    let n = remaining_len / num_pool_rewards;
    // Raydium uses 2 (SPL) or 3 (Token-2022 present). Reject anything else as
    // an unexpected layout we won't blindly index into.
    if n == 2 || n == 3 {
        Some(n)
    } else {
        None
    }
}

/// Validate every reward `recipient_token_account` (group index [1]) forwarded in
/// `remaining` is owned by the vault PDA, BEFORE the `decrease_liquidity_v2` CPI.
/// `num_pool_rewards` is the count of INITIALIZED rewards on the pool (read from
/// the already-loaded `pool_state.reward_infos`). Fail-closed on any parse/layout
/// problem. The legit recipient (the vault-owned reward ATA the backend already
/// passes) still passes.
pub fn validate_reward_recipients(
    remaining: &[AccountInfo],
    vault_key: &Pubkey,
    num_pool_rewards: usize,
) -> Result<()> {
    let group = reward_group_size(remaining.len(), num_pool_rewards)
        .ok_or(error!(VaultError::InvalidRewardRecipient))?;
    if group == 0 {
        return Ok(()); // empty remaining → no rewards to collect
    }
    for chunk in remaining.chunks(group) {
        // chunk[1] is the recipient_token_account. chunks() guarantees full-width
        // chunks here because we already verified len % group == 0.
        let recipient = &chunk[1];
        // Read only the SPL token-account `owner` field (offset 32) without a full
        // deserialize — keeps the per-group loop off the SBF stack. Layout is
        // identical for SPL-Token and Token-2022 base accounts.
        let data = recipient
            .try_borrow_data()
            .map_err(|_| error!(VaultError::InvalidRewardRecipient))?;
        require!(data.len() >= 64, VaultError::InvalidRewardRecipient);
        let owner = Pubkey::try_from(&data[32..64])
            .map_err(|_| error!(VaultError::InvalidRewardRecipient))?;
        require_keys_eq!(owner, *vault_key, VaultError::InvalidRewardRecipient);
    }
    Ok(())
}

/// Count INITIALIZED rewards on a Raydium pool from its `reward_infos`. A slot is
/// considered initialized when its `token_mint` is non-default (mirrors Raydium's
/// own "reward enabled" signal). `token_mint` is read into a local first because
/// `PoolState` is `#[repr(C, packed)]` (unaligned field — taking a reference is UB).
pub fn count_initialized_rewards(
    reward_infos: &[raydium_clmm_cpi::states::RewardInfo],
) -> usize {
    reward_infos
        .iter()
        .filter(|r| {
            let mint = r.token_mint; // copy out of the packed struct
            mint != Pubkey::default()
        })
        .count()
}

// ─── Math regression tests (audit checklist #2) ───────────────────────────────
#[cfg(test)]
mod math_tests {
    use super::*;

    const Q64: u128 = 1u128 << 64;

    #[test]
    fn sqrt_at_tick_zero_is_one() {
        // price(0) = 1.0 → sqrt_price_x64 = 2^64
        assert_eq!(get_sqrt_price_at_tick(0), Q64);
    }

    #[test]
    fn positive_ticks_not_broken() {
        // Regression guard: old code returned ~1 for positive ticks.
        let s_pos = get_sqrt_price_at_tick(28230);
        let s_neg = get_sqrt_price_at_tick(-28230);
        assert!(s_pos > Q64, "positive tick must be > 2^64, got {}", s_pos);
        assert!(s_neg < Q64, "negative tick must be < 2^64, got {}", s_neg);
        assert!(s_neg < get_sqrt_price_at_tick(0));
        assert!(get_sqrt_price_at_tick(0) < s_pos, "must be monotonic");
    }

    #[test]
    fn sqrt_reciprocal_symmetry() {
        // price(t)·price(-t) = 1 → sqrt(t) ≈ 2^128 / sqrt(-t).
        for t in [60i32, 1000, 28230, 100000, 200000] {
            let s_pos = get_sqrt_price_at_tick(t);
            let s_neg = get_sqrt_price_at_tick(-t);
            let expected = u128::MAX / s_pos; // ≈ 2^128 / s_pos
            let diff = if s_neg > expected { s_neg - expected } else { expected - s_neg };
            // tolerance 1e-5 relative (our true error is ~1e-10)
            assert!(
                diff.saturating_mul(100_000) <= expected.max(1),
                "tick {} reciprocal off: s_neg={} expected={}", t, s_neg, expected
            );
        }
    }

    #[test]
    fn amount_1_delta_exact() {
        // amount1 = L·(sqrt_hi − sqrt_lo) >> 64
        assert_eq!(get_amount_1_delta(Q64, Q64 + Q64, 1000), 1000);       // diff = 2^64 → L
        assert_eq!(get_amount_1_delta(Q64, Q64 + (1u128 << 63), 1000), 500); // diff = 2^63 → L/2
        assert_eq!(get_amount_1_delta(Q64, Q64 + (1u128 << 62), 4000), 1000); // diff = 2^62 → L/4
    }

    #[test]
    fn delta_zero_edge_cases() {
        assert_eq!(get_amount_0_delta(Q64, Q64, 1000), 0);     // lo == hi
        assert_eq!(get_amount_1_delta(Q64, Q64, 1000), 0);     // lo == hi
        assert_eq!(get_amount_0_delta(Q64, 2 * Q64, 0), 0);    // L == 0
        assert_eq!(get_amount_1_delta(Q64, 2 * Q64, 0), 0);    // L == 0
    }

    #[test]
    fn amount_0_delta_positive_and_bounded() {
        // token0 = L·(√hi−√lo)/(√lo·√hi) — positive for a valid in-range span.
        let lo = Q64;
        let hi = Q64 + (1u128 << 60);
        let a0 = get_amount_0_delta(lo, hi, 1_000_000_000);
        assert!(a0 > 0, "amount0 must be positive for a valid range");
        // sanity upper bound: amount0 < L for a narrow range above 1.0
        assert!((a0 as u128) < 1_000_000_000);
    }

    #[test]
    fn swap_floor_rejects_lowball() {
        // At price 1.0 (sqrt = 2^64), selling 1_000_000 token0 → expect ~1_000_000 token1.
        // Floor = expected · (1 − 1%) ≈ 990_000. A min_out of 1 must be below the floor.
        let floor = swap_min_out_floor(Q64, 1_000_000, true).unwrap();
        assert!(floor > 900_000 && floor <= 1_000_000, "floor={}", floor);
        assert!(1 < floor, "min_out=1 must be rejected by the floor");
    }

    #[test]
    fn m2_only_dead_shares_does_not_lock_out() {
        // After everyone withdraws, total_shares is stuck at DEAD_SHARES and TVL ≈ dust.
        // A large next deposit must NOT overflow — calculate_shares_to_mint must treat
        // "only dead shares remain" as a fresh start (audit M2).
        let mut v = Vault::default();
        v.total_shares = DEAD_SHARES;            // 1000 phantom shares left
        let big_deposit = 1_000_000_000_000u64;  // 1M USDC-equiv
        let dust_tvl = 3u64;                     // pennies of leftover value

        // Old buggy path would compute big_deposit × 1000 / 3 → > u64::MAX → MathOverflow.
        let shares = v.calculate_shares_to_mint(big_deposit, dust_tvl)
            .expect("must not overflow when only dead shares remain");
        assert_eq!(shares, big_deposit, "fresh-start: 1:1 share price");
    }

    #[test]
    fn shares_mint_normal_ratio() {
        // Normal case still uses the ratio: deposit × total_shares / tvl.
        let mut v = Vault::default();
        v.total_shares = 1_000_000;
        let shares = v.calculate_shares_to_mint(100, 200).unwrap(); // 100 × 1e6 / 200
        assert_eq!(shares, 500_000);
    }

    // ── M3: position range guardrails ─────────────────────────────────────────
    #[test]
    fn range_accepts_legit_symmetric() {
        // Bot opens a symmetric ±range around current price → current ≈ center.
        // spacing=1, current=0, range ±1000 ticks.
        assert!(validate_position_range(-1000, 1000, 0, 1).is_ok());
        // Slightly off-center but within 20% bound: current at 35% of width.
        assert!(validate_position_range(-1000, 1000, -300, 1).is_ok());
    }

    #[test]
    fn range_rejects_out_of_range() {
        // current price entirely below the range → one-sided.
        assert!(validate_position_range(1000, 3000, 0, 1).is_err());
        // current above the range.
        assert!(validate_position_range(-3000, -1000, 0, 1).is_err());
    }

    #[test]
    fn range_rejects_edge_skew() {
        // current at 95% of the range (near upper edge) → near-one-sided.
        // width=2000, current at -1000+1900 = 900 → right side = 100 < 20%(400).
        assert!(validate_position_range(-1000, 1000, 900, 1).is_err());
    }

    #[test]
    fn range_rejects_too_narrow() {
        // width 4 < MIN (8 spacings × spacing 1).
        assert!(validate_position_range(-2, 2, 0, 1).is_err());
    }

    #[test]
    fn range_respects_tick_spacing() {
        // spacing=60: MIN width = 8×60 = 480. A 300-wide range is too narrow.
        assert!(validate_position_range(-150, 150, 0, 60).is_err());
        // 600-wide centered range passes.
        assert!(validate_position_range(-300, 300, 0, 60).is_ok());
    }

    #[test]
    fn deviation_band_deposit_vs_withdraw() {
        // bps are on the sqrt price; price deviation ≈ 2× sqrt deviation.
        let reference = 1_000_000u128;
        let within = |pct_sqrt: i64, bps: u128| {
            let spot = (reference as i64 + reference as i64 * pct_sqrt / 100) as u128;
            sqrt_within_deviation(spot, reference, bps)
        };

        // Deposit band (250 bps sqrt ≈ 5% price): tolerates normal volatility...
        assert!(within(2, MAX_SQRT_DEVIATION_BPS));   // +2% sqrt — passes
        assert!(within(-2, MAX_SQRT_DEVIATION_BPS));  // symmetric
        // ...but still rejects a large flash move.
        assert!(!within(4, MAX_SQRT_DEVIATION_BPS));  // +4% sqrt > 2.5% band

        // Withdraw band (500 bps ≈ 10% price) is strictly softer: a move that
        // would block a deposit still lets the user exit.
        assert!(within(4, MAX_SQRT_DEVIATION_WITHDRAW_BPS));
        assert!(within(-4, MAX_SQRT_DEVIATION_WITHDRAW_BPS));
        // Withdraw band is wider than deposit band.
        assert!(MAX_SQRT_DEVIATION_WITHDRAW_BPS > MAX_SQRT_DEVIATION_BPS);
    }

    #[test]
    fn deviation_band_exact_boundary() {
        // Exactly at the band edge passes; one unit past fails.
        let reference = 10_000_000u128;
        // 250 bps of 10_000_000 = 250_000 absolute sqrt deviation allowed.
        assert!(sqrt_within_deviation(reference + 250_000, reference, MAX_SQRT_DEVIATION_BPS));
        assert!(!sqrt_within_deviation(reference + 250_001, reference, MAX_SQRT_DEVIATION_BPS));
    }

    #[test]
    fn sqrt_price_to_price_both_branches() {
        // Q64.64 of 1.0 → pool price P = 1. With 6 decimals, 1 token0 = 1e6 token1.
        let one = 1u128 << 64;
        assert_eq!(sqrt_price_to_price(one, true, 6, 6), Some(1_000_000));
        // Inverted pool at P=1 is still 1.
        assert_eq!(sqrt_price_to_price(one, false, 6, 6), Some(1_000_000));

        // sqrt_price for pool price P = 4 → sqrt = 2 (Q64.64 = 2 * 2^64).
        let sqrt_p4 = 2u128 << 64;
        // Non-inverted: 1 token0 = 4 token1 → 4e6 at 6 decimals.
        assert_eq!(sqrt_price_to_price(sqrt_p4, true, 6, 6), Some(4_000_000));
        // Inverted: price flips to 1/4 → 0.25 = 250_000 at 6 decimals.
        assert_eq!(sqrt_price_to_price(sqrt_p4, false, 6, 6), Some(250_000));
    }

    #[test]
    fn sqrt_price_to_price_inverted_is_reciprocal() {
        // non_inverted × inverted ≈ scale^2 (10^6 × 10^6 = 10^12) within rounding.
        let sqrt_p4 = 2u128 << 64;
        let direct = sqrt_price_to_price(sqrt_p4, true, 6, 6).unwrap() as u128;
        let inverted = sqrt_price_to_price(sqrt_p4, false, 6, 6).unwrap() as u128;
        assert_eq!(direct * inverted, 1_000_000_000_000);
    }

    #[test]
    fn sqrt_price_to_price_zero_sqrt_is_none() {
        // sqrt so small that the >>32 intermediate is 0 → None, never a divide-by-zero.
        assert_eq!(sqrt_price_to_price(1, true, 6, 6), None);
        assert_eq!(sqrt_price_to_price(1, false, 6, 6), None);
    }

    // ── NEW-2: check_price_not_manipulated guard against a real obs buffer ─────
    //
    // Build a minimal ObservationState account (raw bytes, current layout) with
    // two checkpoints spanning ≥30 s at a constant tick, so reference_sqrt_price
    // resolves to get_sqrt_price_at_tick(tick). Then assert spot within / out of
    // band passes / reverts.
    fn make_obs_buffer(pool_id: Pubkey, newest_ts: u32, older_ts: u32, tick: i64) -> Vec<u8> {
        // total size must cover OBS_OBSERVATIONS_OFFSET + OBS_NUM * OBS_ENTRY_LEN
        let mut buf = vec![0u8; OBS_OBSERVATIONS_OFFSET + OBS_NUM * OBS_ENTRY_LEN];
        buf[OBS_INITIALIZED_OFFSET] = 1; // initialized = true
        buf[OBS_POOL_ID_OFFSET..OBS_POOL_ID_OFFSET + 32].copy_from_slice(pool_id.as_ref());

        // tick_cumulative grows by `tick` per second → constant avg tick.
        let write = |buf: &mut [u8], slot: usize, ts: u32| {
            let base = OBS_OBSERVATIONS_OFFSET + slot * OBS_ENTRY_LEN;
            buf[base..base + 4].copy_from_slice(&ts.to_le_bytes());
            let cum: i64 = tick * ts as i64;
            buf[base + 4..base + 12].copy_from_slice(&cum.to_le_bytes());
        };
        write(&mut buf, 0, older_ts);
        write(&mut buf, 1, newest_ts);
        buf
    }

    #[test]
    fn new2_guard_within_band_passes() {
        let pool = Pubkey::new_unique();
        // 30 s window at tick 0 → reference sqrt = get_sqrt_price_at_tick(0) = 2^64.
        let obs = make_obs_buffer(pool, 100, 60, 0);
        let reference = get_sqrt_price_at_tick(0);
        // spot exactly at reference (0 deviation) must pass under the deposit band.
        assert!(check_price_not_manipulated(reference, &obs, true, MAX_SQRT_DEVIATION_BPS).is_ok());
        // spot 1% above reference in sqrt terms is still inside the 2.5% band.
        let spot = reference + reference / 100;
        assert!(check_price_not_manipulated(spot, &obs, true, MAX_SQRT_DEVIATION_BPS).is_ok());
    }

    #[test]
    fn new2_guard_out_of_band_reverts() {
        let pool = Pubkey::new_unique();
        let obs = make_obs_buffer(pool, 100, 60, 0);
        let reference = get_sqrt_price_at_tick(0);
        // spot 5% above reference in sqrt terms exceeds the 2.5% deposit band → revert.
        let spot = reference + reference / 20;
        assert!(check_price_not_manipulated(spot, &obs, true, MAX_SQRT_DEVIATION_BPS).is_err());
    }

    #[test]
    fn new2_observation_pool_id_roundtrips() {
        let pool = Pubkey::new_unique();
        let obs = make_obs_buffer(pool, 100, 60, 0);
        // The guard's pool_id check reads exactly this; must roundtrip.
        assert_eq!(observation_pool_id(&obs), Some(pool));
        // A different vault pool_id must NOT match.
        assert_ne!(observation_pool_id(&obs), Some(Pubkey::new_unique()));
    }

    #[test]
    fn new2_no_30s_window_is_failsafe_when_required() {
        // Only one checkpoint (no ≥30 s window) → reference_sqrt_price = None.
        // With require_reference = true (vault holds funds) this must FAIL-SAFE.
        let pool = Pubkey::new_unique();
        let mut obs = make_obs_buffer(pool, 100, 80, 0); // window only 20 s < 30 s
        let _ = &mut obs;
        let spot = get_sqrt_price_at_tick(0);
        assert!(check_price_not_manipulated(spot, &obs, true, MAX_SQRT_DEVIATION_BPS).is_err());
        // When NOT required (empty vault), the same missing window is allowed.
        assert!(check_price_not_manipulated(spot, &obs, false, MAX_SQRT_DEVIATION_BPS).is_ok());
    }

    // ── NEW-1: live-liquidity pro-rata math (withdraw_from_position) ───────────
    //
    // user_liquidity = position_liquidity × shares_amount / total_shares, where
    // position_liquidity now comes from the LIVE position. This mirrors the exact
    // checked-math expression in the handler. The key property: if the stale cache
    // were ABOVE the live value, using the cache would over-remove principal;
    // using the live value caps removal at the true pro-rata share.
    fn pro_rata_liquidity(position_liquidity: u128, shares_amount: u128, total_shares: u128) -> u128 {
        position_liquidity
            .checked_mul(shares_amount)
            .and_then(|v| v.checked_div(total_shares))
            .unwrap_or(0)
    }

    #[test]
    fn new1_pro_rata_uses_live_not_cache() {
        // Live position has 1_000 liquidity; a stale cache claims 1_500.
        let live = 1_000u128;
        let stale_cache = 1_500u128;
        let shares = 50u128;
        let total = 100u128;

        // Fair (live) share = 1000 * 50 / 100 = 500.
        assert_eq!(pro_rata_liquidity(live, shares, total), 500);
        // Cache-based would over-remove: 1500 * 50 / 100 = 750 > 500.
        assert!(pro_rata_liquidity(stale_cache, shares, total) > pro_rata_liquidity(live, shares, total));
    }

    #[test]
    fn new1_pro_rata_full_redeem_and_zero() {
        // Redeeming 100% of shares removes 100% of live liquidity.
        assert_eq!(pro_rata_liquidity(1_000, 100, 100), 1_000);
        // Zero live liquidity → zero removal, never panics.
        assert_eq!(pro_rata_liquidity(0, 50, 100), 0);
    }

    #[test]
    fn new1_cache_decrement_saturates() {
        // After NEW-1 the source can exceed the analytics cache; the end-of-handler
        // `vault.position_liquidity.saturating_sub(user_liquidity)` must not underflow.
        let cache: u128 = 800;        // stale, BELOW the live-derived removal
        let user_liquidity: u128 = 1_000;
        assert_eq!(cache.saturating_sub(user_liquidity), 0);
    }

    #[test]
    fn shares_mint_dust_tvl_does_not_overflow() {
        // L-2: DEAD_SHARES + a tiny stuck holder, TVL collapsed to dust. A large
        // deposit must NOT revert — it falls back to fresh-start pricing.
        let mut v = Vault::default();
        v.total_shares = DEAD_SHARES + 5; // just above the dead-shares floor
        let dust_tvl = 1u64;              // ~zero value left in the vault
        let shares = v
            .calculate_shares_to_mint(u64::MAX, dust_tvl)
            .expect("must not revert");
        assert_eq!(shares, u64::MAX); // fresh-start fallback
    }

    // ── NEW-3: reward-recipient group-size math ───────────────────────────────
    // These cover the pure index/divisibility logic of `reward_group_size`. The
    // owner-equality check itself (recipient.owner == vault PDA) is enforced at
    // runtime in `validate_reward_recipients` and exercised by integration tests
    // (constructing real AccountInfos with backing data is impractical here).
    #[test]
    fn reward_group_empty_is_noop() {
        // No reward accounts forwarded → group size 0 (handler treats as no-op).
        assert_eq!(reward_group_size(0, 0), Some(0));
        assert_eq!(reward_group_size(0, 3), Some(0));
    }

    #[test]
    fn reward_group_spl_width_two() {
        // 1 SPL reward → [vault, recipient] = 2 accounts.
        assert_eq!(reward_group_size(2, 1), Some(2));
        // 2 SPL rewards → 4 accounts, width 2.
        assert_eq!(reward_group_size(4, 2), Some(2));
        // 3 SPL rewards → 6 accounts, width 2.
        assert_eq!(reward_group_size(6, 3), Some(2));
    }

    #[test]
    fn reward_group_token2022_width_three() {
        // Token-2022 reward present → [vault, recipient, mint] = 3 per reward.
        assert_eq!(reward_group_size(3, 1), Some(3));
        assert_eq!(reward_group_size(6, 2), Some(3));
        assert_eq!(reward_group_size(9, 3), Some(3));
    }

    #[test]
    fn reward_group_rejects_accounts_without_rewards() {
        // Caller forwarded accounts but the pool has zero initialized rewards →
        // fail-closed (None), so the handler reverts.
        assert_eq!(reward_group_size(2, 0), None);
        assert_eq!(reward_group_size(4, 0), None);
    }

    #[test]
    fn reward_group_rejects_non_divisible() {
        // 5 accounts can't split evenly into 2 groups → None.
        assert_eq!(reward_group_size(5, 2), None);
        // 7 accounts / 2 rewards → ragged → None.
        assert_eq!(reward_group_size(7, 2), None);
    }

    #[test]
    fn reward_group_rejects_unexpected_width() {
        // Width must be 2 or 3. 1 account / 1 reward (width 1) → None.
        assert_eq!(reward_group_size(1, 1), None);
        // width 4 (e.g. 8 accts / 2 rewards) is not a Raydium layout → None.
        assert_eq!(reward_group_size(8, 2), None);
    }
}
