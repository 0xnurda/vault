use anchor_lang::prelude::*;

#[event]
pub struct VaultInitialized {
    pub admin: Pubkey,
    pub protocol_wallet: Pubkey,
    pub share_mint: Pubkey,
    pub token0_treasury: Pubkey,
    pub token1_treasury: Pubkey,
    pub pool_id: Pubkey,
}

#[event]
pub struct DepositToken0Event {
    pub user: Pubkey,
    pub amount: u64,
    pub deposit_value: u64,
    pub shares_minted: u64,
    pub total_shares: u64,
    pub tvl: u64,
    pub token0_price: u64,
}

#[event]
pub struct DepositToken1Event {
    pub user: Pubkey,
    pub amount: u64,
    pub shares_minted: u64,
    pub total_shares: u64,
    pub tvl: u64,
}

#[event]
pub struct WithdrawEvent {
    pub user: Pubkey,
    pub shares_burned: u64,
    pub token0_withdrawn: u64,
    pub token1_withdrawn: u64,
    pub withdrawal_value: u64,
}

#[event]
pub struct SwapEvent {
    pub amount_in: u64,
    pub direction: String,
    pub treasury_token0: u64,
    pub treasury_token1: u64,
}

#[event]
pub struct PositionOpened {
    pub position_mint: Pubkey,
    pub pool_id: Pubkey,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: u128,
    pub token0_used: u64,
    pub token1_used: u64,
}

#[event]
pub struct PositionClosed {
    pub treasury_token0: u64,
    pub treasury_token1: u64,
}

#[event]
pub struct LiquidityIncreased {
    pub token0_added: u64,
    pub token1_added: u64,
    pub new_liquidity: u128,
}

#[event]
pub struct LiquidityDecreased {
    pub token0_received: u64,
    pub token1_received: u64,
    pub remaining_liquidity: u128,
}

#[event]
pub struct FeesCollected {
    pub total_token0_fees: u64,
    pub total_token1_fees: u64,
    pub protocol_token0_fees: u64,
    pub protocol_token1_fees: u64,
}

#[event]
pub struct ProtocolFeeExtracted {
    pub token0_amount: u64,
    pub token1_amount: u64,
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
