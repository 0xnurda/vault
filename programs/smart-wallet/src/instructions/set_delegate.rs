use anchor_lang::prelude::*;

use crate::errors::WalletError;
use crate::events::DelegateSet;
use crate::state::{seeds, SmartWallet};

#[derive(Accounts)]
pub struct SetDelegate<'info> {
    /// User setting the delegate (must be owner)
    #[account(mut)]
    pub user: Signer<'info>,

    /// Smart wallet state
    #[account(
        mut,
        seeds = [seeds::SMART_WALLET, user.key().as_ref()],
        bump = wallet.bump,
        constraint = wallet.owner == user.key() @ WalletError::Unauthorized,
    )]
    pub wallet: Account<'info, SmartWallet>,
}

/// Set or remove delegate. Pass Pubkey::default() to remove.
pub fn handler(ctx: Context<SetDelegate>, new_delegate: Pubkey) -> Result<()> {
    let wallet = &mut ctx.accounts.wallet;

    let old_delegate = wallet.delegate;
    wallet.delegate = new_delegate;
    wallet.updated_at = Clock::get()?.unix_timestamp;

    emit!(DelegateSet {
        wallet: wallet.key(),
        old_delegate,
        new_delegate,
    });

    Ok(())
}
