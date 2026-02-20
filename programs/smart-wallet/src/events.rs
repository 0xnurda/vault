use anchor_lang::prelude::*;

#[event]
pub struct WalletCreated {
    pub owner: Pubkey,
    pub sol_treasury: Pubkey,
    pub usdc_treasury: Pubkey,
}

#[event]
pub struct WalletClosed {
    pub owner: Pubkey,
}

#[event]
pub struct DelegateSet {
    pub wallet: Pubkey,
    pub old_delegate: Pubkey,
    pub new_delegate: Pubkey,
}

#[event]
pub struct FundTreasuryEvent {
    pub wallet: Pubkey,
    pub operator: Pubkey,
    pub amount: u64,
    pub is_sol: bool,
}

#[event]
pub struct WalletWithdrawEvent {
    pub wallet: Pubkey,
    pub user: Pubkey,
    pub amount: u64,
    pub is_sol: bool,
}

#[event]
pub struct WalletPositionOpened {
    pub wallet: Pubkey,
    pub position_mint: Pubkey,
    pub pool_id: Pubkey,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: u128,
    pub sol_used: u64,
    pub usdc_used: u64,
}

#[event]
pub struct WalletPositionClosed {
    pub wallet: Pubkey,
    pub sol_treasury: u64,
    pub usdc_treasury: u64,
}

#[event]
pub struct WalletLiquidityIncreased {
    pub wallet: Pubkey,
    pub sol_added: u64,
    pub usdc_added: u64,
    pub new_liquidity: u128,
}

#[event]
pub struct WalletLiquidityDecreased {
    pub wallet: Pubkey,
    pub sol_received: u64,
    pub usdc_received: u64,
    pub remaining_liquidity: u128,
}

#[event]
pub struct WalletFeesCollected {
    pub wallet: Pubkey,
    pub sol_fees: u64,
    pub usdc_fees: u64,
}

#[event]
pub struct WalletSwapEvent {
    pub wallet: Pubkey,
    pub amount_in: u64,
    pub direction: String,
}

#[event]
pub struct WalletPausedEvent {
    pub wallet: Pubkey,
    pub paused: bool,
}
