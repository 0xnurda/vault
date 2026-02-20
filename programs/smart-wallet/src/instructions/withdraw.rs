use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::WalletError;
use crate::events::WalletWithdrawEvent;
use crate::state::{seeds, SmartWallet};

#[derive(Accounts)]
pub struct Withdraw<'info> {
    /// User withdrawing (must be owner — delegate CANNOT withdraw)
    #[account(mut)]
    pub user: Signer<'info>,

    /// Smart wallet state
    #[account(
        mut,
        seeds = [seeds::SMART_WALLET, user.key().as_ref()],
        bump = wallet.bump,
        constraint = wallet.owner == user.key() @ WalletError::Unauthorized,
    )]
    pub wallet: Box<Account<'info, SmartWallet>>,

    /// Wallet's source treasury
    #[account(mut)]
    pub treasury: Box<Account<'info, TokenAccount>>,

    /// User's destination token account
    #[account(
        mut,
        constraint = user_token_account.owner == user.key(),
        constraint = user_token_account.mint == treasury.mint @ WalletError::InvalidMint,
    )]
    pub user_token_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<Withdraw>, amount: u64, is_sol: bool) -> Result<()> {
    require!(amount > 0, WalletError::InvalidAmount);

    let wallet = &ctx.accounts.wallet;
    let wallet_key = wallet.key();
    let sol_bump = [wallet.sol_treasury_bump];
    let usdc_bump = [wallet.usdc_treasury_bump];

    // Validate treasury matches and build seeds
    let treasury_seeds: &[&[u8]] = if is_sol {
        require!(
            ctx.accounts.treasury.key() == wallet.sol_treasury,
            WalletError::Unauthorized
        );
        &[seeds::WALLET_SOL_TREASURY, wallet_key.as_ref(), &sol_bump]
    } else {
        require!(
            ctx.accounts.treasury.key() == wallet.usdc_treasury,
            WalletError::Unauthorized
        );
        &[seeds::WALLET_USDC_TREASURY, wallet_key.as_ref(), &usdc_bump]
    };

    // Check sufficient balance
    require!(
        ctx.accounts.treasury.amount >= amount,
        WalletError::InsufficientBalance
    );

    // Transfer from treasury to user (treasury signs with self-authority)
    let signer_seeds = &[treasury_seeds];
    let cpi_accounts = Transfer {
        from: ctx.accounts.treasury.to_account_info(),
        to: ctx.accounts.user_token_account.to_account_info(),
        authority: ctx.accounts.treasury.to_account_info(),
    };
    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        cpi_accounts,
        signer_seeds,
    );
    token::transfer(cpi_ctx, amount)?;

    // Update timestamp
    let wallet = &mut ctx.accounts.wallet;
    wallet.updated_at = Clock::get()?.unix_timestamp;

    emit!(WalletWithdrawEvent {
        wallet: wallet.key(),
        user: ctx.accounts.user.key(),
        amount,
        is_sol,
    });

    Ok(())
}
