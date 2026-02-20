use anchor_lang::prelude::*;
use anchor_spl::token::{self, CloseAccount, Token, TokenAccount};

use crate::errors::WalletError;
use crate::events::WalletClosed;
use crate::state::{seeds, SmartWallet};

#[derive(Accounts)]
pub struct CloseWallet<'info> {
    /// Owner closing their wallet
    #[account(mut)]
    pub user: Signer<'info>,

    /// Smart wallet state (will be closed, rent returned to user)
    #[account(
        mut,
        seeds = [seeds::SMART_WALLET, user.key().as_ref()],
        bump = wallet.bump,
        constraint = wallet.owner == user.key() @ WalletError::Unauthorized,
        constraint = !wallet.has_active_position @ WalletError::NoActivePosition,
        close = user,
    )]
    pub wallet: Box<Account<'info, SmartWallet>>,

    /// SOL treasury (will be closed)
    #[account(
        mut,
        seeds = [seeds::WALLET_SOL_TREASURY, wallet.key().as_ref()],
        bump = wallet.sol_treasury_bump,
        constraint = sol_treasury.amount == 0 @ WalletError::InsufficientBalance,
    )]
    pub sol_treasury: Box<Account<'info, TokenAccount>>,

    /// USDC treasury (will be closed)
    #[account(
        mut,
        seeds = [seeds::WALLET_USDC_TREASURY, wallet.key().as_ref()],
        bump = wallet.usdc_treasury_bump,
        constraint = usdc_treasury.amount == 0 @ WalletError::InsufficientBalance,
    )]
    pub usdc_treasury: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<CloseWallet>) -> Result<()> {
    let wallet = &ctx.accounts.wallet;
    let wallet_key = wallet.key();

    // Close SOL treasury (self-authority PDA signs)
    let sol_treasury_seeds: &[&[u8]] = &[
        seeds::WALLET_SOL_TREASURY,
        wallet_key.as_ref(),
        &[wallet.sol_treasury_bump],
    ];
    token::close_account(CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        CloseAccount {
            account: ctx.accounts.sol_treasury.to_account_info(),
            destination: ctx.accounts.user.to_account_info(),
            authority: ctx.accounts.sol_treasury.to_account_info(),
        },
        &[sol_treasury_seeds],
    ))?;

    // Close USDC treasury (self-authority PDA signs)
    let usdc_treasury_seeds: &[&[u8]] = &[
        seeds::WALLET_USDC_TREASURY,
        wallet_key.as_ref(),
        &[wallet.usdc_treasury_bump],
    ];
    token::close_account(CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        CloseAccount {
            account: ctx.accounts.usdc_treasury.to_account_info(),
            destination: ctx.accounts.user.to_account_info(),
            authority: ctx.accounts.usdc_treasury.to_account_info(),
        },
        &[usdc_treasury_seeds],
    ))?;

    // Wallet PDA is closed via `close = user` constraint above

    emit!(WalletClosed {
        owner: ctx.accounts.user.key(),
    });

    Ok(())
}
