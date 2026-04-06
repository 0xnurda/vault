use anchor_lang::prelude::*;

use crate::errors::VaultError;
use crate::events::AdminTransferProposed;
use crate::state::{seeds, Vault};

/// Step 1: Current admin proposes a new admin
#[derive(Accounts)]
pub struct TransferAdmin<'info> {
    /// Current admin
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

pub fn handler(ctx: Context<TransferAdmin>, new_admin: Pubkey) -> Result<()> {
    require!(new_admin != Pubkey::default(), VaultError::Unauthorized);

    let vault = &mut ctx.accounts.vault;
    vault.pending_admin = new_admin;

    emit!(AdminTransferProposed {
        current_admin: vault.admin,
        proposed_admin: new_admin,
    });

    Ok(())
}
