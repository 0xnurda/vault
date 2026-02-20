use anchor_lang::prelude::*;

#[error_code]
pub enum WalletError {
    #[msg("Unauthorized: not owner or delegate")]
    Unauthorized,

    #[msg("Invalid amount")]
    InvalidAmount,

    #[msg("Insufficient treasury balance")]
    InsufficientBalance,

    #[msg("Math overflow")]
    MathOverflow,

    #[msg("Position already exists")]
    PositionAlreadyExists,

    #[msg("No active position")]
    NoActivePosition,

    #[msg("Invalid position")]
    InvalidPosition,

    #[msg("No delegate set")]
    NoDelegateSet,

    #[msg("Token account not approved for smart wallet")]
    NotApproved,

    #[msg("Insufficient approved amount")]
    InsufficientApproval,

    #[msg("Token mint mismatch")]
    InvalidMint,

    #[msg("Wallet is paused")]
    WalletPaused,
}
