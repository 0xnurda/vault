use anchor_lang::prelude::*;

use crate::errors::VaultError;
use crate::events::TvlUpdated;
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

    // C-02: Sanity checks — max 20% TVL change per update when shares exist
    if vault.total_shares > 0 && old_tvl > 0 {
        let max_change = old_tvl / 5; // 20%
        require!(
            tvl_usd <= old_tvl.saturating_add(max_change)
                && tvl_usd >= old_tvl.saturating_sub(max_change),
            VaultError::TvlChangeExceeded
        );
    }

    vault.tvl_usd = tvl_usd;
    vault.sol_price_usd = sol_price;
    vault.last_tvl_update = Clock::get()?.unix_timestamp;

    emit!(TvlUpdated {
        old_tvl,
        new_tvl: tvl_usd,
        sol_price,
        share_price: vault.share_price(),
    });

    Ok(())
}
