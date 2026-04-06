use anchor_lang::prelude::Pubkey;
use anchor_lang::solana_program::pubkey;

/// Raydium CLMM program ID (mainnet and devnet share the same program).
pub const RAYDIUM_CLMM_PROGRAM_ID: Pubkey =
    pubkey!("CAMMCzo5YL8w4VFF8KVHrK22GGUsp5VTaW7grrKgrWqK");

/// Wrapped SOL (wSOL) mint address.
pub const WSOL_MINT: Pubkey =
    pubkey!("So11111111111111111111111111111111111111112");

/// Minimum SOL deposit: 0.001 SOL (anti-dust, prevents rounding to 0 shares)
pub const MIN_DEPOSIT_SOL: u64 = 1_000_000; // lamports

/// Minimum USDC deposit: 1 USDC (anti-dust)
pub const MIN_DEPOSIT_USDC: u64 = 1_000_000; // 6 decimals
