use anchor_lang::prelude::*;

/// Per-user smart wallet — holds treasury accounts and position state
#[account]
#[derive(Default)]
pub struct SmartWallet {
    /// User who owns this wallet
    pub owner: Pubkey,

    /// Backend operator who can manage positions (Pubkey::default() = no delegate)
    pub delegate: Pubkey,

    /// PDA that holds wSOL
    pub sol_treasury: Pubkey,

    /// PDA that holds USDC
    pub usdc_treasury: Pubkey,

    /// USDC mint address
    pub usdc_mint: Pubkey,

    // ============ Position state ============

    /// Active position NFT mint
    pub position_mint: Pubkey,

    /// Raydium CLMM pool ID
    pub position_pool_id: Pubkey,

    /// Whether there's an active position
    pub has_active_position: bool,

    /// Liquidity in position
    pub position_liquidity: u128,

    /// Lower tick of position
    pub position_tick_lower: i32,

    /// Upper tick of position
    pub position_tick_upper: i32,

    /// SOL amount in active position (lamports)
    pub position_sol: u64,

    /// USDC amount in active position (6 decimals)
    pub position_usdc: u64,

    // ============ Bumps ============

    /// SmartWallet PDA bump
    pub bump: u8,

    /// SOL treasury PDA bump
    pub sol_treasury_bump: u8,

    /// USDC treasury PDA bump
    pub usdc_treasury_bump: u8,

    // ============ Flags ============

    /// Whether the wallet is paused (deposits/position management disabled)
    pub is_paused: bool,

    // ============ Timestamps ============

    /// Wallet creation timestamp
    pub created_at: i64,

    /// Last activity timestamp
    pub updated_at: i64,
}

impl SmartWallet {
    pub const LEN: usize = 8 +  // discriminator
        32 + // owner
        32 + // delegate
        32 + // sol_treasury
        32 + // usdc_treasury
        32 + // usdc_mint
        32 + // position_mint
        32 + // position_pool_id
        1 +  // has_active_position
        16 + // position_liquidity (u128)
        4 +  // position_tick_lower (i32)
        4 +  // position_tick_upper (i32)
        8 +  // position_sol
        8 +  // position_usdc
        1 +  // bump
        1 +  // sol_treasury_bump
        1 +  // usdc_treasury_bump
        1 +  // is_paused
        8 +  // created_at
        8 +  // updated_at
        63;  // padding for future fields
}

/// Seeds for PDAs
pub mod seeds {
    pub const SMART_WALLET: &[u8] = b"smart_wallet";
    pub const WALLET_SOL_TREASURY: &[u8] = b"wallet_sol";
    pub const WALLET_USDC_TREASURY: &[u8] = b"wallet_usdc";
}
