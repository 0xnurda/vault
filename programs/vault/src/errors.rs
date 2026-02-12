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

    #[msg("TVL is stale: update required before operation")]
    StaleTvl,

    #[msg("Math overflow")]
    MathOverflow,

    #[msg("Invalid mint address")]
    InvalidMint,

    #[msg("Vault is paused")]
    VaultPaused,

    #[msg("Withdrawal amount exceeds available treasury")]
    WithdrawalExceedsTreasury,

    #[msg("Minimum deposit not met")]
    MinimumDepositNotMet,

    #[msg("Invalid SOL price")]
    InvalidSolPrice,

    #[msg("Position already exists")]
    PositionAlreadyExists,

    #[msg("No active position")]
    NoActivePosition,

    #[msg("Invalid position")]
    InvalidPosition,
}
