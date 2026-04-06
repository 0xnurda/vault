use anchor_lang::prelude::*;
use anchor_spl::token::spl_token::native_mint;
use raydium_clmm_cpi::states::{PersonalPositionState, PoolState};

use crate::errors::VaultError;
use crate::state::{seeds, Vault};

/// Sync vault.position_sol / position_usdc with real on-chain amounts.
///
/// position_sol/usdc are set at open_position / increase_liquidity time and
/// become stale as the CLMM pool price moves (CLMM auto-rebalances token mix).
/// This instruction reads the actual amounts from the Raydium position and
/// updates the vault state so TVL calculations and share pricing are accurate.
///
/// Should be called by the admin bot periodically (e.g. before each deposit or
/// on a cron every few minutes).
#[derive(Accounts)]
pub struct SyncPositionValue<'info> {
    /// Admin signs to prevent griefing (anyone could spam this)
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
        constraint = vault.has_active_position @ VaultError::NoActivePosition,
        constraint = vault.position_pool_id == pool_state.key() @ VaultError::InvalidPriceFeed,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// Raydium CLMM pool — provides current sqrt_price_x64 and tick_current.
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Raydium personal position PDA: ["position", position_mint].
    /// CHECK: key validated against vault.position_mint below.
    pub personal_position: UncheckedAccount<'info>,
}

pub fn sync_handler(ctx: Context<SyncPositionValue>) -> Result<()> {
    let vault = &mut ctx.accounts.vault;

    // ── Validate personal_position PDA ───────────────────────────────────────
    let (expected_pda, _) = Pubkey::find_program_address(
        &[b"position", vault.position_mint.as_ref()],
        &raydium_clmm_cpi::id(),
    );
    require!(
        ctx.accounts.personal_position.key() == expected_pda,
        VaultError::InvalidPosition
    );

    // ── Read position state ───────────────────────────────────────────────────
    let position_data = ctx.accounts.personal_position.try_borrow_data()?;
    let pos = PersonalPositionState::try_deserialize(&mut &position_data[..])?;
    let liquidity = pos.liquidity;
    let tick_lower = pos.tick_lower_index;
    let tick_upper = pos.tick_upper_index;
    drop(position_data);

    if liquidity == 0 {
        vault.position_sol  = 0;
        vault.position_usdc = 0;
        vault.position_liquidity = 0;
        return Ok(());
    }

    // ── Read pool state ───────────────────────────────────────────────────────
    let pool = ctx.accounts.pool_state.load()?;
    let sqrt_price_x64 = pool.sqrt_price_x64;
    let tick_current   = pool.tick_current;
    let sol_is_token0  = pool.token_mint_0 == native_mint::id();
    drop(pool);

    // ── Compute sqrt prices at tick bounds ────────────────────────────────────
    let sqrt_lower = get_sqrt_price_at_tick(tick_lower);
    let sqrt_upper = get_sqrt_price_at_tick(tick_upper);

    // ── Calculate real amounts ────────────────────────────────────────────────
    //
    //  In-range:     amount0 = L*(sqrt_upper-sqrt_current)/(sqrt_upper*sqrt_current/2^64)
    //                amount1 = L*(sqrt_current-sqrt_lower)/2^64
    //  Below range:  all token0, no token1
    //  Above range:  no token0, all token1
    let (amount_token0, amount_token1) = if tick_current < tick_lower {
        (get_amount_0_delta(sqrt_lower, sqrt_upper, liquidity), 0u64)
    } else if tick_current >= tick_upper {
        (0u64, get_amount_1_delta(sqrt_lower, sqrt_upper, liquidity))
    } else {
        (
            get_amount_0_delta(sqrt_price_x64, sqrt_upper, liquidity),
            get_amount_1_delta(sqrt_lower, sqrt_price_x64, liquidity),
        )
    };

    // ── Update vault ──────────────────────────────────────────────────────────
    if sol_is_token0 {
        vault.position_sol  = amount_token0;
        vault.position_usdc = amount_token1;
    } else {
        vault.position_sol  = amount_token1;
        vault.position_usdc = amount_token0;
    }
    vault.position_liquidity = liquidity;

    msg!(
        "sync_position_value: sol={} usdc={} liquidity={} tick_current={}",
        vault.position_sol,
        vault.position_usdc,
        liquidity,
        tick_current,
    );

    Ok(())
}

// ── Tick math ─────────────────────────────────────────────────────────────────
//
// Standard Uniswap v3 / Raydium tick math.
// Intermediate multiplications are Q128.128 (need the high 128 bits of a
// 256-bit product), implemented here with 4×u64 limb arithmetic.

/// Multiply two Q128.128 ratios and return the high 128 bits (i.e. result >> 128).
fn mul_shift_128(a: u128, b: u128) -> u128 {
    let a_lo = a & u64::MAX as u128;
    let a_hi = a >> 64;
    let b_lo = b & u64::MAX as u128;
    let b_hi = b >> 64;

    let lo_lo = a_lo * b_lo;
    let lo_hi = a_lo * b_hi;
    let hi_lo = a_hi * b_lo;
    let hi_hi = a_hi * b_hi;

    let mid_carry = (lo_lo >> 64)
        .wrapping_add(lo_hi & u64::MAX as u128)
        .wrapping_add(hi_lo & u64::MAX as u128);

    hi_hi
        .wrapping_add(lo_hi >> 64)
        .wrapping_add(hi_lo >> 64)
        .wrapping_add(mid_carry >> 64)
}

/// Returns sqrt(1.0001^tick) in Q64.64 format.
/// Identical to Uniswap v3 / Raydium TickMath.getSqrtRatioAtTick().
fn get_sqrt_price_at_tick(tick: i32) -> u128 {
    let abs_tick = tick.unsigned_abs() as u128;

    // 0x100000000000000000000000000000000 = 2^128 (Q128.128 representation of 1.0)
    // overflows u128, so use u128::MAX (error = 1 ULP ≈ 3e-39, negligible)
    let mut ratio: u128 = if abs_tick & 0x1 != 0 {
        0xfffcb933bd6fad37aa2d162d1a594001
    } else {
        u128::MAX
    };

    if abs_tick & 0x2      != 0 { ratio = mul_shift_128(ratio, 0xfff97272373d413259a46990580e213a); }
    if abs_tick & 0x4      != 0 { ratio = mul_shift_128(ratio, 0xfff2e50f5f656932ef12357cf3c7fdcc); }
    if abs_tick & 0x8      != 0 { ratio = mul_shift_128(ratio, 0xffe5caca7e10e4e61c3624eaa0941cd0); }
    if abs_tick & 0x10     != 0 { ratio = mul_shift_128(ratio, 0xffcb9843d60f6159c9db58835c926644); }
    if abs_tick & 0x20     != 0 { ratio = mul_shift_128(ratio, 0xff973b41fa98c081472e6896dfb254c0); }
    if abs_tick & 0x40     != 0 { ratio = mul_shift_128(ratio, 0xff2ea16466c96a3843ec78b326b52861); }
    if abs_tick & 0x80     != 0 { ratio = mul_shift_128(ratio, 0xfe5dee046a99a2a811c461f1969c3053); }
    if abs_tick & 0x100    != 0 { ratio = mul_shift_128(ratio, 0xfcbe86c7900a88aedcffc83b479aa3a4); }
    if abs_tick & 0x200    != 0 { ratio = mul_shift_128(ratio, 0xf987a7253ac413176f2b074cf7815e54); }
    if abs_tick & 0x400    != 0 { ratio = mul_shift_128(ratio, 0xf3392b0822b70005940c7a398e4b70f3); }
    if abs_tick & 0x800    != 0 { ratio = mul_shift_128(ratio, 0xe7159475a2c29b7443b29c7fa6e889d9); }
    if abs_tick & 0x1000   != 0 { ratio = mul_shift_128(ratio, 0xd097f3bdfd2022b8845ad8f792aa5825); }
    if abs_tick & 0x2000   != 0 { ratio = mul_shift_128(ratio, 0xa9f746462d870fdf8a65dc1f90e061e5); }
    if abs_tick & 0x4000   != 0 { ratio = mul_shift_128(ratio, 0x70d869a156d2a1b890bb3df62baf32f7); }
    if abs_tick & 0x8000   != 0 { ratio = mul_shift_128(ratio, 0x31be135f97d08fd981231505542fcfa6); }
    if abs_tick & 0x10000  != 0 { ratio = mul_shift_128(ratio, 0x9aa508b5b7a84e1c677de54f3e99bc9);  }
    if abs_tick & 0x20000  != 0 { ratio = mul_shift_128(ratio, 0x5d6af8dedb81196699c329225ee604);   }
    if abs_tick & 0x40000  != 0 { ratio = mul_shift_128(ratio, 0x2216e584f5fa1ea926041bedfe98);     }
    if abs_tick & 0x80000  != 0 { ratio = mul_shift_128(ratio, 0x48a170391f7dc42444e8fa2);          }

    if tick > 0 {
        ratio = u128::MAX / ratio;
    }

    // Convert Q128.128 → Q64.64 (round up)
    let frac = ratio & ((1u128 << 64) - 1);
    (ratio >> 64) + if frac != 0 { 1 } else { 0 }
}

// ── Amount delta helpers ──────────────────────────────────────────────────────
//
// Both functions use "divide-first" ordering to stay within u128.
// For typical vault sizes (< 10 000 SOL) and SOL/USDC price range these
// will not overflow; in the unlikely case of overflow we saturate to u64::MAX.

/// token0 (SOL if sol_is_token0) amount from liquidity + sqrt price range.
/// amount0 = L × (sqrt_hi − sqrt_lo) × 2^64 / (sqrt_hi × sqrt_lo)
fn get_amount_0_delta(sqrt_lo: u128, sqrt_hi: u128, liquidity: u128) -> u64 {
    if sqrt_lo >= sqrt_hi || liquidity == 0 {
        return 0;
    }
    let diff = sqrt_hi - sqrt_lo;

    // Rearrange to: (L * diff / sqrt_hi) * 2^64 / sqrt_lo
    // Step 1: L * diff / sqrt_hi  — both fit in ~2^64 for typical values
    let step1 = (liquidity as u128)
        .saturating_mul(diff)
        .checked_div(sqrt_hi)
        .unwrap_or(0);

    // Step 2: step1 * 2^64 / sqrt_lo
    // step1 can be up to ~2^64; multiply by 2^64 would overflow.
    // Split: (step1 << 32) / (sqrt_lo >> 32)
    let result = step1
        .checked_shl(32)
        .unwrap_or(u128::MAX)
        .checked_div(sqrt_lo >> 32)
        .unwrap_or(0);

    result.min(u64::MAX as u128) as u64
}

/// token1 (USDC if sol_is_token0) amount from liquidity + sqrt price range.
/// amount1 = L × (sqrt_hi − sqrt_lo) / 2^64
fn get_amount_1_delta(sqrt_lo: u128, sqrt_hi: u128, liquidity: u128) -> u64 {
    if sqrt_lo >= sqrt_hi || liquidity == 0 {
        return 0;
    }
    let diff = sqrt_hi - sqrt_lo;

    // L * diff / 2^64 — split into (L >> 32) * (diff >> 32)
    let result = (liquidity >> 32).saturating_mul(diff >> 32);
    result.min(u64::MAX as u128) as u64
}
