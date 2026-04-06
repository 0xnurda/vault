use anchor_lang::prelude::*;

use crate::errors::VaultError;
use crate::state::{seeds, Vault};

/// Emergency instruction: cancel a stuck rebalance.
///
/// If `open_position` fails after `close_position` sets `is_rebalancing = true`,
/// users are blocked from deposits and withdrawals until this is called.
///
/// Only callable by admin. Resets `is_rebalancing = false` so users can withdraw.
/// Admin must ensure treasury funds are safe before calling this.
///
/// NOTE: Do NOT call this mid-rebalance (between close_position and open_position)
/// unless the rebalance is truly stuck. Calling it prematurely will unblock users
/// while no active position exists, which is acceptable (funds sit in treasury).
#[derive(Accounts)]
pub struct CancelRebalance<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
        constraint = vault.is_rebalancing @ VaultError::NotRebalancing,
    )]
    pub vault: Box<Account<'info, Vault>>,
}

pub fn handler(ctx: Context<CancelRebalance>) -> Result<()> {
    let vault = &mut ctx.accounts.vault;
    vault.is_rebalancing = false;

    msg!(
        "Rebalance cancelled by admin {}. is_rebalancing = false.",
        ctx.accounts.admin.key()
    );

    Ok(())
}
