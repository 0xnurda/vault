use anchor_lang::prelude::*;

#[event]
pub struct VaultInitialized {
    pub admin: Pubkey,
    pub protocol_wallet: Pubkey,
    pub share_mint: Pubkey,
    pub sol_treasury: Pubkey,
    pub usdc_treasury: Pubkey,
    pub sol_price_feed: Pubkey,
}

#[event]
pub struct DepositSolEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub deposit_value_usd: u64,
    pub shares_minted: u64,
    pub total_shares: u64,
    pub tvl_usd: u64,
    pub sol_price_usd: u64,
}

#[event]
pub struct DepositUsdcEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub shares_minted: u64,
    pub total_shares: u64,
    pub tvl_usd: u64,
}

#[event]
pub struct WithdrawEvent {
    pub user: Pubkey,
    pub shares_burned: u64,
    pub sol_withdrawn: u64,
    pub usdc_withdrawn: u64,
    pub withdrawal_value_usd: u64,
}

#[event]
pub struct SwapEvent {
    pub amount_in: u64,
    pub direction: String,
    pub treasury_sol: u64,
    pub treasury_usdc: u64,
}

#[event]
pub struct PositionOpened {
    pub position_mint: Pubkey,
    pub pool_id: Pubkey,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: u128,
    pub sol_used: u64,
    pub usdc_used: u64,
}

#[event]
pub struct PositionClosed {
    pub treasury_sol: u64,
    pub treasury_usdc: u64,
}

#[event]
pub struct LiquidityIncreased {
    pub sol_added: u64,
    pub usdc_added: u64,
    pub new_liquidity: u128,
}

#[event]
pub struct LiquidityDecreased {
    pub sol_received: u64,
    pub usdc_received: u64,
    pub remaining_liquidity: u128,
}

#[event]
pub struct FeesCollected {
    /// Total SOL fees collected from Raydium
    pub total_sol_fees: u64,
    /// Total USDC fees collected from Raydium
    pub total_usdc_fees: u64,
    /// SOL fees allocated to protocol (10%)
    pub protocol_sol_fees: u64,
    /// USDC fees allocated to protocol (10%)
    pub protocol_usdc_fees: u64,
}

#[event]
pub struct ProtocolFeeExtracted {
    pub sol_amount: u64,
    pub usdc_amount: u64,
    pub protocol_wallet: Pubkey,
}

#[event]
pub struct VaultPausedEvent {
    pub paused: bool,
}

#[event]
pub struct AdminTransferProposed {
    pub current_admin: Pubkey,
    pub proposed_admin: Pubkey,
}

#[event]
pub struct AdminTransferAccepted {
    pub old_admin: Pubkey,
    pub new_admin: Pubkey,
}

#[event]
pub struct PriceFeedUpdated {
    pub raydium_pool: Pubkey,
}
