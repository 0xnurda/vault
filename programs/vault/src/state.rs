use anchor_lang::prelude::*;
use crate::errors::VaultError;

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
    /// Unix timestamp when the current rebalance started (set by close_position).
    /// Emergency withdrawal timeout: users can withdraw after 3600s even if rebalancing.
    /// Cleared by open_position and cancel_rebalance.
    pub rebalance_started_at: i64,
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
        16;  // padding for future fields

    /// Calculate TVL in token1 units using on-chain pool price.
    ///
    /// For SOL/USDC pools: token1 = USDC → TVL is in USDC micro-units ≈ USD.
    /// accumulated_protocol_fees are excluded (they belong to the protocol).
    ///
    /// token0_price_in_token1: price of 1 token0 in token1 units
    ///   (with token1_decimals decimal places)
    pub fn calculate_tvl(&self, token0_price_in_token1: u64) -> u64 {
        let user_token0 = self.treasury_token0
            .saturating_add(self.position_token0)
            .saturating_sub(self.accumulated_protocol_fees_token0);
        let user_token1 = self.treasury_token1
            .saturating_add(self.position_token1)
            .saturating_sub(self.accumulated_protocol_fees_token1);

        let token0_value = (user_token0 as u128)
            .checked_mul(token0_price_in_token1 as u128)
            .and_then(|v| v.checked_div(10u128.pow(self.token0_decimals as u32)))
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0);

        token0_value.saturating_add(user_token1)
    }

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
        if self.total_shares == 0 || current_tvl == 0 {
            return Ok(deposit_value);
        }
        (deposit_value as u128)
            .checked_mul(self.total_shares as u128)
            .and_then(|v| v.checked_div(current_tvl as u128))
            .and_then(|v| u64::try_from(v).ok())
            .ok_or(error!(VaultError::MathOverflow))
    }

    /// token1 value of given shares given current TVL.
    pub fn calculate_withdrawal_value(
        &self,
        shares: u64,
        current_tvl: u64,
    ) -> Result<u64> {
        if self.total_shares == 0 {
            return Ok(0);
        }
        (shares as u128)
            .checked_mul(current_tvl as u128)
            .and_then(|v| v.checked_div(self.total_shares as u128))
            .and_then(|v| u64::try_from(v).ok())
            .ok_or(error!(VaultError::MathOverflow))
    }

    /// Convert token0 amount to token1 units using pool price.
    pub fn token0_to_token1(&self, amount: u64, token0_price_in_token1: u64) -> u64 {
        (amount as u128)
            .checked_mul(token0_price_in_token1 as u128)
            .and_then(|v| v.checked_div(10u128.pow(self.token0_decimals as u32)))
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0)
    }

    /// User-accessible treasury token0 (excluding protocol fees).
    pub fn user_treasury_token0(&self) -> u64 {
        self.treasury_token0.saturating_sub(self.accumulated_protocol_fees_token0)
    }

    /// User-accessible treasury token1 (excluding protocol fees).
    pub fn user_treasury_token1(&self) -> u64 {
        self.treasury_token1.saturating_sub(self.accumulated_protocol_fees_token1)
    }
}

// ─── Raydium CLMM pool price helpers ───────────────────────────────────────

/// Byte offset of `token_mint_0` in a Raydium CLMM PoolState account.
const POOL_TOKEN_MINT_0_OFFSET: usize = 73;

/// Byte offset of `sqrt_price_x64` in a Raydium CLMM PoolState account.
const POOL_SQRT_PRICE_OFFSET: usize = 253;

/// Byte offset of `tick_current` (i32) in a Raydium CLMM PoolState account.
const POOL_TICK_CURRENT_OFFSET: usize = 269;

/// Read `token_mint_0` from raw Raydium CLMM pool account bytes.
pub fn read_pool_token_mint_0(data: &[u8]) -> Option<Pubkey> {
    let end = POOL_TOKEN_MINT_0_OFFSET + 32;
    let bytes: [u8; 32] = data.get(POOL_TOKEN_MINT_0_OFFSET..end)?.try_into().ok()?;
    Some(Pubkey::from(bytes))
}

/// Read `sqrt_price_x64` (u128, little-endian) from raw Raydium CLMM pool bytes.
pub fn read_pool_sqrt_price_x64(data: &[u8]) -> Option<u128> {
    let end = POOL_SQRT_PRICE_OFFSET + 16;
    let bytes: [u8; 16] = data.get(POOL_SQRT_PRICE_OFFSET..end)?.try_into().ok()?;
    Some(u128::from_le_bytes(bytes))
}

/// Read `tick_current` (i32, little-endian) from raw Raydium CLMM pool bytes.
pub fn read_pool_tick_current(data: &[u8]) -> Option<i32> {
    let end = POOL_TICK_CURRENT_OFFSET + 4;
    let bytes: [u8; 4] = data.get(POOL_TICK_CURRENT_OFFSET..end)?.try_into().ok()?;
    Some(i32::from_le_bytes(bytes))
}

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

/// Multiply two Q128.128 fixed-point values and return the high 128 bits.
/// Implements (a × b) >> 128 using four u64-limb arithmetic to avoid overflow.
pub fn mul_shift_128(a: u128, b: u128) -> u128 {
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

/// Convert tick index to sqrt_price_x64 (Q64.64).
/// Uses the Uniswap v3 / Raydium CLMM precomputed ratio table.
pub fn get_sqrt_price_at_tick(tick: i32) -> u128 {
    let abs_tick = tick.unsigned_abs() as u128;

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

    let frac = ratio & ((1u128 << 64) - 1);
    (ratio >> 64) + if frac != 0 { 1 } else { 0 }
}

/// token0 amount from a liquidity range.
pub fn get_amount_0_delta(sqrt_lo: u128, sqrt_hi: u128, liquidity: u128) -> u64 {
    if sqrt_lo >= sqrt_hi || liquidity == 0 {
        return 0;
    }
    let diff = sqrt_hi - sqrt_lo;
    let step1 = (liquidity as u128)
        .saturating_mul(diff)
        .checked_div(sqrt_hi)
        .unwrap_or(0);
    let result = step1
        .checked_shl(32)
        .unwrap_or(u128::MAX)
        .checked_div(sqrt_lo >> 32)
        .unwrap_or(0);
    result.min(u64::MAX as u128) as u64
}

/// token1 amount from a liquidity range.
pub fn get_amount_1_delta(sqrt_lo: u128, sqrt_hi: u128, liquidity: u128) -> u64 {
    if sqrt_lo >= sqrt_hi || liquidity == 0 {
        return 0;
    }
    let diff = sqrt_hi - sqrt_lo;
    let result = (liquidity >> 32).saturating_mul(diff >> 32);
    result.min(u64::MAX as u128) as u64
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
    pub const POSITION_NFT: &[u8] = b"position_nft";
}
