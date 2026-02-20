use anchor_lang::prelude::*;

use crate::errors::VaultError;
use crate::events::VaultPausedEvent;
use crate::state::{seeds, Vault};

#[derive(Accounts)]
pub struct SetPaused<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
    )]
    pub vault: Account<'info, Vault>,
}

pub fn handler(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
    ctx.accounts.vault.is_paused = paused;
    emit!(VaultPausedEvent { paused });
    Ok(())
}
