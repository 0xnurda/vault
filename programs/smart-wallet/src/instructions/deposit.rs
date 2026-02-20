use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::WalletError;
use crate::events::FundTreasuryEvent;
use crate::state::{seeds, SmartWallet};

/// Fund treasury — backend (delegate) pulls tokens from user's account via approve/delegate.
/// User must have previously called SPL Token `approve()` on their token accounts,
/// granting the smart wallet PDA as delegate.
#[derive(Accounts)]
pub struct FundTreasury<'info> {
    /// Operator (delegate or owner) triggering the pull
    #[account(mut)]
    pub operator: Signer<'info>,

    /// Smart wallet state
    #[account(
        mut,
        constraint = wallet.owner == operator.key() || wallet.delegate == operator.key() @ WalletError::Unauthorized,
    )]
    pub wallet: Box<Account<'info, SmartWallet>>,

    /// User's source token account (must have approved wallet PDA as delegate)
    #[account(
        mut,
        constraint = user_token_account.owner == wallet.owner @ WalletError::Unauthorized,
    )]
    pub user_token_account: Box<Account<'info, TokenAccount>>,

    /// Wallet's destination treasury
    #[account(mut)]
    pub treasury: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<FundTreasury>, amount: u64, is_sol: bool) -> Result<()> {
    require!(amount > 0, WalletError::InvalidAmount);
    require!(!ctx.accounts.wallet.is_paused, WalletError::WalletPaused);

    let wallet = &ctx.accounts.wallet;

    // Validate treasury matches
    if is_sol {
        require!(
            ctx.accounts.treasury.key() == wallet.sol_treasury,
            WalletError::Unauthorized
        );
    } else {
        require!(
            ctx.accounts.treasury.key() == wallet.usdc_treasury,
            WalletError::Unauthorized
        );
    }

    // Verify wallet PDA is the delegate on user's token account
    require!(
        ctx.accounts.user_token_account.delegate.contains(&wallet.key()),
        WalletError::NotApproved
    );
    require!(
        ctx.accounts.user_token_account.delegated_amount >= amount,
        WalletError::InsufficientApproval
    );

    // Transfer from user to treasury using wallet PDA as delegate authority
    let owner_key = wallet.owner;
    let wallet_seeds: &[&[&[u8]]] = &[&[
        seeds::SMART_WALLET,
        owner_key.as_ref(),
        &[wallet.bump],
    ]];

    let cpi_accounts = Transfer {
        from: ctx.accounts.user_token_account.to_account_info(),
        to: ctx.accounts.treasury.to_account_info(),
        authority: ctx.accounts.wallet.to_account_info(), // wallet PDA is the delegate
    };
    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        cpi_accounts,
        wallet_seeds,
    );
    token::transfer(cpi_ctx, amount)?;

    let wallet = &mut ctx.accounts.wallet;
    wallet.updated_at = Clock::get()?.unix_timestamp;

    emit!(FundTreasuryEvent {
        wallet: wallet.key(),
        operator: ctx.accounts.operator.key(),
        amount,
        is_sol,
    });

    Ok(())
}
