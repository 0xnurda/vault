use anchor_lang::prelude::*;

/// Main Vault account - stores global state
#[account]
#[derive(Default)]
pub struct Vault {
    /// Admin who can manage funds (backend wallet)
    pub admin: Pubkey,

    /// SPL Token mint for vault shares
    pub share_mint: Pubkey,

    /// PDA that holds SOL (wrapped as wSOL)
    pub sol_treasury: Pubkey,

    /// PDA that holds USDC
    pub usdc_treasury: Pubkey,

    /// USDC mint address (mainnet: EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v)
    pub usdc_mint: Pubkey,

    /// Total shares minted
    pub total_shares: u64,

    /// Total SOL in treasury (lamports)
    pub treasury_sol: u64,

    /// Total USDC in treasury (6 decimals)
    pub treasury_usdc: u64,

    /// Total Value Locked in USD (6 decimals, e.g., 1000000 = $1)
    pub tvl_usd: u64,

    /// Current SOL price in USD (6 decimals)
    pub sol_price_usd: u64,

    /// Last TVL update timestamp
    pub last_tvl_update: i64,

    /// Vault PDA bump
    pub bump: u8,

    /// Sol treasury PDA bump
    pub sol_treasury_bump: u8,

    /// USDC treasury PDA bump
    pub usdc_treasury_bump: u8,

    /// Share mint authority bump
    pub share_mint_bump: u8,

    /// Active position NFT mint (None if no position)
    pub position_mint: Pubkey,

    /// Whether there's an active position
    pub has_active_position: bool,

    /// SOL amount in active position (lamports)
    pub position_sol: u64,

    /// USDC amount in active position (6 decimals)
    pub position_usdc: u64,

    /// Liquidity in position
    pub position_liquidity: u128,

    /// Lower tick of position
    pub position_tick_lower: i32,

    /// Upper tick of position
    pub position_tick_upper: i32,

    /// Pool ID for the position
    pub position_pool_id: Pubkey,
}

impl Vault {
    pub const LEN: usize = 8 + // discriminator
        32 + // admin
        32 + // share_mint
        32 + // sol_treasury
        32 + // usdc_treasury
        32 + // usdc_mint
        8 +  // total_shares
        8 +  // treasury_sol
        8 +  // treasury_usdc
        8 +  // tvl_usd
        8 +  // sol_price_usd
        8 +  // last_tvl_update
        1 +  // bump
        1 +  // sol_treasury_bump
        1 +  // usdc_treasury_bump
        1 +  // share_mint_bump
        32 + // position_mint
        1 +  // has_active_position
        8 +  // position_sol
        8 +  // position_usdc
        16 + // position_liquidity (u128)
        4 +  // position_tick_lower (i32)
        4 +  // position_tick_upper (i32)
        32 + // position_pool_id
        64;  // padding for future fields

    /// Calculate share price: TVL / total_shares (with 6 decimal precision)
    pub fn share_price(&self) -> u64 {
        if self.total_shares == 0 {
            return 1_000_000; // 1 USD = 1 share initially
        }
        // share_price = tvl_usd / total_shares
        self.tvl_usd
            .checked_mul(1_000_000)
            .unwrap()
            .checked_div(self.total_shares)
            .unwrap_or(1_000_000)
    }

    /// Calculate shares to mint for a deposit value in USD
    pub fn calculate_shares_to_mint(&self, deposit_value_usd: u64) -> u64 {
        if self.total_shares == 0 {
            // First deposit: 1 USD = 1 share
            return deposit_value_usd;
        }
        // shares = deposit_value * total_shares / tvl
        deposit_value_usd
            .checked_mul(self.total_shares)
            .unwrap()
            .checked_div(self.tvl_usd)
            .unwrap_or(0)
    }

    /// Calculate withdrawal value in USD for given shares
    pub fn calculate_withdrawal_value(&self, shares: u64) -> u64 {
        if self.total_shares == 0 {
            return 0;
        }
        // value = shares * tvl / total_shares
        shares
            .checked_mul(self.tvl_usd)
            .unwrap()
            .checked_div(self.total_shares)
            .unwrap_or(0)
    }

    /// Convert SOL amount (lamports) to USD value (6 decimals)
    pub fn sol_to_usd(&self, lamports: u64) -> u64 {
        // lamports * sol_price / 10^9 (SOL has 9 decimals)
        lamports
            .checked_mul(self.sol_price_usd)
            .unwrap()
            .checked_div(1_000_000_000)
            .unwrap_or(0)
    }

    /// Convert USD value to SOL amount (lamports)
    pub fn usd_to_sol(&self, usd_value: u64) -> u64 {
        // usd_value * 10^9 / sol_price
        usd_value
            .checked_mul(1_000_000_000)
            .unwrap()
            .checked_div(self.sol_price_usd)
            .unwrap_or(0)
    }
}

/// User deposit record
#[account]
#[derive(Default)]
pub struct UserDeposit {
    /// User's wallet address
    pub user: Pubkey,

    /// Vault this deposit belongs to
    pub vault: Pubkey,

    /// Number of shares owned
    pub shares: u64,

    /// Total SOL deposited (for tracking, lamports)
    pub total_deposited_sol: u64,

    /// Total USDC deposited (for tracking, 6 decimals)
    pub total_deposited_usdc: u64,

    /// Total USD value withdrawn
    pub total_withdrawn_usd: u64,

    /// First deposit timestamp
    pub created_at: i64,

    /// Last activity timestamp
    pub updated_at: i64,

    /// PDA bump
    pub bump: u8,
}

impl UserDeposit {
    pub const LEN: usize = 8 + // discriminator
        32 + // user
        32 + // vault
        8 +  // shares
        8 +  // total_deposited_sol
        8 +  // total_deposited_usdc
        8 +  // total_withdrawn_usd
        8 +  // created_at
        8 +  // updated_at
        1 +  // bump
        32;  // padding
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
