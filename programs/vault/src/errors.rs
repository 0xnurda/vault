use anchor_lang::prelude::*;

#[error_code]
pub enum VaultError {
    #[msg("Unauthorized: only admin can perform this action")]
    Unauthorized,

    #[msg("Invalid amount: must be greater than zero")]
    InvalidAmount,

    #[msg("Insufficient shares for withdrawal")]
    InsufficientShares,

    #[msg("Insufficient treasury balance")]
    InsufficientTreasuryBalance,

    #[msg("Math overflow")]
    MathOverflow,

    #[msg("Invalid mint address")]
    InvalidMint,

    #[msg("Vault is paused")]
    VaultPaused,

    #[msg("Vault is currently rebalancing, try again shortly")]
    RebalancingInProgress,

    #[msg("Withdrawal exceeds available treasury — use withdraw_from_position when a position is active")]
    WithdrawalExceedsTreasury,

    #[msg("Position already exists")]
    PositionAlreadyExists,

    #[msg("No active position")]
    NoActivePosition,

    #[msg("Invalid position")]
    InvalidPosition,

    #[msg("No pending admin transfer")]
    NoPendingAdmin,

    #[msg("No protocol fees accumulated to extract")]
    NoFeesToExtract,

    #[msg("Invalid tick range: tick_lower must be less than tick_upper")]
    InvalidTickRange,

    #[msg("Deposit amount too small (below minimum)")]
    DepositTooSmall,

    #[msg("Vault is not currently rebalancing")]
    NotRebalancing,

    #[msg("Invalid price feed: must be the Raydium CLMM pool set by admin")]
    InvalidPriceFeed,

    #[msg("Slippage exceeds maximum allowed (500 bps / 5%)")]
    SlippageTooHigh,

    #[msg("Output below minimum: slippage tolerance exceeded")]
    SlippageExceeded,

    #[msg("Price manipulation detected: spot price deviates from TWAP by more than 1.5%")]
    PriceManipulationDetected,

    #[msg("Swap volume exceeds the per-window rate limit")]
    SwapVolumeExceeded,

    #[msg("Oracle price unavailable: pool has no usable observation history")]
    OracleUnavailable,

    #[msg("Position range invalid: too narrow/wide or not centered on current price")]
    InvalidPositionRange,

    #[msg("Fees must be collected in the same transaction before decreasing or closing the position")]
    FeesNotCollected,

    #[msg("Swap cooldown active: too soon since the last treasury swap")]
    SwapCooldownActive,

    #[msg("No reward tokens available to extract")]
    NoRewardsToExtract,
}
