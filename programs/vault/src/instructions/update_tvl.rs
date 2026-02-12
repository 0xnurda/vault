use anchor_lang::prelude::*;

use crate::errors::VaultError;
use crate::state::{seeds, Vault};

#[derive(Accounts)]
pub struct UpdateTvl<'info> {
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

/// Update TVL (called by backend periodically)
/// tvl_usd: Total Value Locked in USD (6 decimals)
/// sol_price: Current SOL price in USD (6 decimals)
pub fn handler(ctx: Context<UpdateTvl>, tvl_usd: u64, sol_price: u64) -> Result<()> {
    let vault = &mut ctx.accounts.vault;

    require!(sol_price > 0, VaultError::InvalidSolPrice);

    let old_tvl = vault.tvl_usd;
    let old_price = vault.sol_price_usd;

    vault.tvl_usd = tvl_usd;
    vault.sol_price_usd = sol_price;
    vault.last_tvl_update = Clock::get()?.unix_timestamp;

    msg!(
        "TVL updated: {} -> {} USD (6 decimals)",
        old_tvl,
        tvl_usd
    );
    msg!(
        "SOL price updated: {} -> {} USD (6 decimals)",
        old_price,
        sol_price
    );
    msg!("Share price: {} (6 decimals)", vault.share_price());

    Ok(())
}
