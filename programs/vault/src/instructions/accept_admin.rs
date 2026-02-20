use anchor_lang::prelude::*;

use crate::errors::VaultError;
use crate::events::AdminTransferAccepted;
use crate::state::{seeds, Vault};

/// Step 2: New admin accepts the transfer
#[derive(Accounts)]
pub struct AcceptAdmin<'info> {
    /// New admin (must be the pending_admin)
    #[account(mut)]
    pub new_admin: Signer<'info>,

    /// Vault state
    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.pending_admin == new_admin.key() @ VaultError::NoPendingAdmin,
    )]
    pub vault: Account<'info, Vault>,
}

pub fn handler(ctx: Context<AcceptAdmin>) -> Result<()> {
    let vault = &mut ctx.accounts.vault;
    let old_admin = vault.admin;

    vault.admin = vault.pending_admin;
    vault.pending_admin = Pubkey::default();

    emit!(AdminTransferAccepted {
        old_admin,
        new_admin: vault.admin,
    });

    Ok(())
}
