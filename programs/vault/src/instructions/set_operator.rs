use anchor_lang::prelude::*;

use crate::errors::VaultError;
use crate::state::{seeds, Vault};

/// Set the hot operator key (admin only).
///
/// The operator runs automated operations (rebalance, collect_fees, swaps)
/// from a hot wallet without needing multisig approval each time. Its powers
/// are bounded on-chain — it can never move funds to an arbitrary address.
/// Admin (cold/multisig) can rotate the operator instantly if it's compromised.
#[derive(Accounts)]
pub struct SetOperator<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
    )]
    pub vault: Box<Account<'info, Vault>>,
}

pub fn handler(ctx: Context<SetOperator>, new_operator: Pubkey) -> Result<()> {
    require!(new_operator != Pubkey::default(), VaultError::Unauthorized);
    let vault = &mut ctx.accounts.vault;
    vault.operator = new_operator;
    msg!("Operator set to {}", new_operator);
    Ok(())
}
