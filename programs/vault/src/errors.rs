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

    #[msg("Withdrawal exceeds available treasury — admin must call decrease_liquidity or close_position first")]
    WithdrawalExceedsTreasury,

    #[msg("Position already exists")]
    PositionAlreadyExists,

    #[msg("No active position")]
    NoActivePosition,

    #[msg("Invalid position")]
    InvalidPosition,

    #[msg("No pending admin transfer")]
    NoPendingAdmin,

    #[msg("Oracle price is stale (older than 60 seconds)")]
    StaleOraclePrice,

    #[msg("Oracle price is invalid (zero or negative)")]
    InvalidOraclePrice,

    #[msg("No protocol fees accumulated to extract")]
    NoFeesToExtract,

    #[msg("Invalid tick range: tick_lower must be less than tick_upper")]
    InvalidTickRange,

    #[msg("Deposit amount too small (below minimum)")]
    DepositTooSmall,

    #[msg("Vault is not currently rebalancing")]
    NotRebalancing,

    #[msg("SOL price not set — admin must call update_price first")]
    SolPriceNotSet,

    #[msg("Invalid SOL price: must be greater than zero")]
    InvalidSolPrice,

    #[msg("Invalid price feed: must be the Raydium CLMM pool set by admin")]
    InvalidPriceFeed,
}
