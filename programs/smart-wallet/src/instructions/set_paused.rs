
use anchor_lang::prelude::*;

use crate::errors::WalletError;
use crate::events::WalletPausedEvent;
use crate::state::{seeds, SmartWallet};

#[derive(Accounts)]
pub struct SetPaused<'info> {
    /// Owner only can pause/unpause
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::SMART_WALLET, user.key().as_ref()],
        bump = wallet.bump,
        constraint = wallet.owner == user.key() @ WalletError::Unauthorized,
    )]
    pub wallet: Account<'info, SmartWallet>,
}

pub fn handler(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
    ctx.accounts.wallet.is_paused = paused;
    ctx.accounts.wallet.updated_at = Clock::get()?.unix_timestamp;
    emit!(WalletPausedEvent {
        wallet: ctx.accounts.wallet.key(),
        paused,
    });
    Ok(())
}
