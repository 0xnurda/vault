use anchor_lang::prelude::*;

use crate::errors::VaultError;
use crate::events::PriceFeedUpdated;
use crate::state::{seeds, Vault};

#[derive(Accounts)]
pub struct UpdatePrice<'info> {
    /// Admin only
    #[account(mut)]
    pub admin: Signer<'info>,

    /// Vault state
    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
    )]
    pub vault: Account<'info, Vault>,
}

/// Set the Raydium CLMM pool used as price source for deposits (admin-only).
/// Call once after upgrade to point vault.sol_price_feed at the Raydium pool.
/// raydium_pool: the Raydium CLMM pool account (e.g. SOL/USDC pool)
pub fn handler(ctx: Context<UpdatePrice>, raydium_pool: Pubkey) -> Result<()> {
    require!(raydium_pool != Pubkey::default(), VaultError::InvalidPriceFeed);

    let vault = &mut ctx.accounts.vault;
    vault.sol_price_feed = raydium_pool;

    emit!(PriceFeedUpdated { raydium_pool });

    Ok(())
}
