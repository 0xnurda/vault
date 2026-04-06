use anchor_lang::prelude::*;
use crate::errors::VaultError;

/// Main Vault account - stores global state
#[account]
#[derive(Default)]
pub struct Vault {
    /// Admin who can manage positions and vault settings
    pub admin: Pubkey,
    /// SPL Token mint for vault shares
    pub share_mint: Pubkey,
    /// PDA that holds SOL (wrapped as wSOL)
    pub sol_treasury: Pubkey,
    /// PDA that holds USDC
    pub usdc_treasury: Pubkey,
    /// USDC mint address (mainnet: EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v)
    pub usdc_mint: Pubkey,
    /// Wallet that receives protocol fees (10% of position trading fees)
    pub protocol_wallet: Pubkey,
    /// Pyth SOL/USD price feed account address (set at initialize)
    pub sol_price_feed: Pubkey,
    /// Total shares minted (6 decimals)
    pub total_shares: u64,
    /// Total SOL in treasury (lamports) — includes accumulated_protocol_fees_sol
    pub treasury_sol: u64,
    /// Total USDC in treasury (6 decimals) — includes accumulated_protocol_fees_usdc
    pub treasury_usdc: u64,
    /// Vault PDA bump
    pub bump: u8,
    /// Sol treasury PDA bump
    pub sol_treasury_bump: u8,
    /// USDC treasury PDA bump
    pub usdc_treasury_bump: u8,
    /// Share mint authority bump
    pub share_mint_bump: u8,
    /// Active position NFT mint (Pubkey::default() if no position)
    pub position_mint: Pubkey,
    /// Whether there's an active CLMM position
    pub has_active_position: bool,
    /// SOL deposited into active position at open time (lamports, approximation)
    pub position_sol: u64,
    /// USDC deposited into active position at open time (6 decimals, approximation)
    pub position_usdc: u64,
    /// Liquidity in position (from Raydium PersonalPositionState)
    pub position_liquidity: u128,
    /// Lower tick of position
    pub position_tick_lower: i32,
    /// Upper tick of position
    pub position_tick_upper: i32,
    /// Pool ID for the active position
    pub position_pool_id: Pubkey,
    /// Whether the vault is paused (user deposits/withdrawals disabled)
    pub is_paused: bool,
    /// True during rebalance (between close_position and open_position).
    /// Blocks user deposits and withdrawals.
    pub is_rebalancing: bool,
    /// Pending admin for two-step admin transfer
    pub pending_admin: Pubkey,
    /// Accumulated SOL protocol fees not yet extracted (lamports).
    /// EXCLUDED from TVL — belongs to protocol, not to users.
    pub accumulated_protocol_fees_sol: u64,
    /// Accumulated USDC protocol fees not yet extracted (6 decimals).
    /// EXCLUDED from TVL — belongs to protocol, not to users.
    pub accumulated_protocol_fees_usdc: u64,
    /// Cached SOL price in USD (6 decimals, e.g. 150_000_000 = $150.00).
    /// Set by admin via update_price. Used for share minting on deposit.
    pub sol_price_usd: u64,
    /// Unix timestamp of the last update_price call.
    pub last_price_update: i64,
}

impl Vault {
    pub const LEN: usize = 8  + // discriminator
        32 + // admin
        32 + // share_mint
        32 + // sol_treasury
        32 + // usdc_treasury
        32 + // usdc_mint
        32 + // protocol_wallet
        32 + // sol_price_feed
        8  + // total_shares
        8  + // treasury_sol
        8  + // treasury_usdc
        1  + // bump
        1  + // sol_treasury_bump
        1  + // usdc_treasury_bump
        1  + // share_mint_bump
        32 + // position_mint
        1  + // has_active_position
        8  + // position_sol
        8  + // position_usdc
        16 + // position_liquidity (u128)
        4  + // position_tick_lower (i32)
        4  + // position_tick_upper (i32)
        32 + // position_pool_id
        1  + // is_paused
        1  + // is_rebalancing
        32 + // pending_admin
        8  + // accumulated_protocol_fees_sol
        8  + // accumulated_protocol_fees_usdc
        8  + // sol_price_usd
        8  + // last_price_update
        16; // padding for future fields

    /// Calculate TVL in USD (6 decimals) using on-chain Pyth SOL price.
    ///
    /// accumulated_protocol_fees are excluded — they sit in treasury but belong to protocol.
    ///
    /// TVL = (treasury_sol - fees_sol + position_sol) * price / 1e9
    ///     + (treasury_usdc - fees_usdc + position_usdc)
    pub fn calculate_tvl(&self, sol_price_usd: u64) -> u64 {
        let user_sol = self.treasury_sol
            .saturating_add(self.position_sol)
            .saturating_sub(self.accumulated_protocol_fees_sol);

        let user_usdc = self.treasury_usdc
            .saturating_add(self.position_usdc)
            .saturating_sub(self.accumulated_protocol_fees_usdc);

        let sol_value_usd = (user_sol as u128)
            .checked_mul(sol_price_usd as u128)
            .and_then(|v| v.checked_div(1_000_000_000))
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0);

        sol_value_usd.saturating_add(user_usdc)
    }

    /// Shares to mint for a deposit. First depositor gets 1 share per $1.
    ///
    /// Returns MathOverflow if deposit_value_usd * total_shares overflows u64.
    /// Callers should also check that shares > 0 (deposit too small for current TVL).
    pub fn calculate_shares_to_mint(
        &self,
        deposit_value_usd: u64,
        current_tvl: u64,
    ) -> Result<u64> {
        if self.total_shares == 0 || current_tvl == 0 {
            return Ok(deposit_value_usd);
        }
        (deposit_value_usd as u128)
            .checked_mul(self.total_shares as u128)
            .and_then(|v| v.checked_div(current_tvl as u128))
            .and_then(|v| u64::try_from(v).ok())
            .ok_or(error!(VaultError::MathOverflow))
    }

    /// USD value of given shares given current TVL.
    ///
    /// Returns MathOverflow if shares * current_tvl overflows u64.
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

    /// Convert SOL lamports to USD (6 decimals) using provided price.
    pub fn sol_to_usd(&self, lamports: u64, sol_price_usd: u64) -> u64 {
        (lamports as u128)
            .checked_mul(sol_price_usd as u128)
            .and_then(|v| v.checked_div(1_000_000_000))
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0)
    }

    /// User-accessible treasury SOL (excluding protocol fees).
    pub fn user_treasury_sol(&self) -> u64 {
        self.treasury_sol.saturating_sub(self.accumulated_protocol_fees_sol)
    }

    /// User-accessible treasury USDC (excluding protocol fees).
    pub fn user_treasury_usdc(&self) -> u64 {
        self.treasury_usdc.saturating_sub(self.accumulated_protocol_fees_usdc)
    }
}

// ─── Raydium CLMM pool price helpers ───────────────────────────────────────

/// Byte offset of `token_mint_0` in a Raydium CLMM PoolState account.
/// Layout (repr C, packed): 8 disc + 1 bump + 32 amm_config + 32 owner = 73.
const POOL_TOKEN_MINT_0_OFFSET: usize = 73;

/// Byte offset of `sqrt_price_x64` in a Raydium CLMM PoolState account.
/// Continues from token_mint_0+32+32+32+32+32+1+1+2+16 = 253.
const POOL_SQRT_PRICE_OFFSET: usize = 253;

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

/// Convert Raydium CLMM `sqrt_price_x64` (Q64.64) to SOL price in USD with 6 decimals.
///
/// The pool pairs WSOL (9 dec) and USDC (6 dec).  Token ordering determines
/// which direction the price ratio goes.
///
/// `sol_is_token0 = true`  → token0=WSOL, token1=USDC
///   price (raw) = micro_USDC / lamport
///   sol_price_6dec = price_raw * 1e9 = (sqrt^2 / 2^128) * 1e9
///
/// `sol_is_token0 = false` → token0=USDC, token1=WSOL
///   price (raw) = lamports / micro_USDC
///   sol_price_6dec = 1e9 / price_raw = 2^128 * 1e9 / sqrt^2
///
/// Uses a >>32 intermediate to avoid u128 overflow when squaring.
pub fn sqrt_price_to_sol_usd(sqrt_price_x64: u128, sol_is_token0: bool) -> Option<u64> {
    let a = sqrt_price_x64 >> 32; // safe: prevents overflow when squaring
    if a == 0 {
        return None;
    }
    let price_q64 = a.checked_mul(a)?; // ≈ price * 2^64

    if sol_is_token0 {
        // sol_price_6dec = price_q64 * 1e9 >> 64
        let raw = price_q64.checked_mul(1_000_000_000u128)?;
        u64::try_from(raw >> 64).ok()
    } else {
        // sol_price_6dec = (1e9 * 2^64) / price_q64
        // 1e9 << 64 ≈ 1.84e28 — fits comfortably in u128
        let numerator: u128 = 1_000_000_000u128 << 64;
        u64::try_from(numerator.checked_div(price_q64)?).ok()
    }
}

/// User deposit record
#[account]
#[derive(Default)]
pub struct UserDeposit {
    pub user: Pubkey,
    pub vault: Pubkey,
    pub shares: u64,
    pub total_deposited_sol: u64,
    pub total_deposited_usdc: u64,
    pub total_withdrawn_usd: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub bump: u8,
}

impl UserDeposit {
    pub const LEN: usize = 8 + 32 + 32 + 8 + 8 + 8 + 8 + 8 + 8 + 1 + 32;
}

/// Seeds for PDAs
pub mod seeds {
    pub const VAULT: &[u8] = b"vault";
    pub const SOL_TREASURY: &[u8] = b"sol_treasury";
    pub const USDC_TREASURY: &[u8] = b"usdc_treasury";
    pub const SHARE_MINT: &[u8] = b"share_mint";
    pub const USER_DEPOSIT: &[u8] = b"user_deposit";
    pub const POSITION_NFT: &[u8] = b"position_nft";
}
