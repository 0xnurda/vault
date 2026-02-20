use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, Token, TokenAccount, Transfer};

use crate::errors::VaultError;
use crate::events::WithdrawEvent;
use crate::state::{seeds, UserDeposit, Vault};

#[derive(Accounts)]
pub struct Withdraw<'info> {
    /// User making the withdrawal
    #[account(mut)]
    pub user: Signer<'info>,

    /// Vault state
    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// User's deposit record
    #[account(
        mut,
        seeds = [seeds::USER_DEPOSIT, vault.key().as_ref(), user.key().as_ref()],
        bump = user_deposit.bump,
        constraint = user_deposit.user == user.key(),
    )]
    pub user_deposit: Box<Account<'info, UserDeposit>>,

    /// Share mint (for burning)
    #[account(
        mut,
        seeds = [seeds::SHARE_MINT, vault.key().as_ref()],
        bump = vault.share_mint_bump,
    )]
    pub share_mint: Box<Account<'info, Mint>>,

    /// User's share token account (source - will burn from here)
    #[account(
        mut,
        constraint = user_share_account.owner == user.key(),
        constraint = user_share_account.mint == share_mint.key(),
    )]
    pub user_share_account: Box<Account<'info, TokenAccount>>,

    /// SOL treasury
    #[account(
        mut,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Box<Account<'info, TokenAccount>>,

    /// USDC treasury
    #[account(
        mut,
        seeds = [seeds::USDC_TREASURY, vault.key().as_ref()],
        bump = vault.usdc_treasury_bump,
    )]
    pub usdc_treasury: Box<Account<'info, TokenAccount>>,

    /// User's wSOL token account (destination for SOL)
    #[account(
        mut,
        constraint = user_wsol_account.owner == user.key(),
        constraint = user_wsol_account.mint == sol_treasury.mint @ VaultError::InvalidMint,
    )]
    pub user_wsol_account: Box<Account<'info, TokenAccount>>,

    /// User's USDC token account (destination for USDC)
    #[account(
        mut,
        constraint = user_usdc_account.owner == user.key(),
        constraint = user_usdc_account.mint == usdc_treasury.mint @ VaultError::InvalidMint,
    )]
    pub user_usdc_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<Withdraw>) -> Result<()> {
    let vault = &mut ctx.accounts.vault;
    let user_deposit = &mut ctx.accounts.user_deposit;

    // M-01: Check pause
    require!(!vault.is_paused, VaultError::VaultPaused);

    // H-01: Check TVL freshness
    let current_time = Clock::get()?.unix_timestamp;
    require!(
        current_time - vault.last_tvl_update < 600,
        VaultError::StaleTvl
    );

    // Full withdrawal only: burn ALL user shares
    let shares_amount = user_deposit.shares;
    require!(shares_amount > 0, VaultError::InsufficientShares);
    require!(
        ctx.accounts.user_share_account.amount >= shares_amount,
        VaultError::InsufficientShares
    );

    // Calculate withdrawal value in USD
    let withdrawal_value_usd = vault.calculate_withdrawal_value(shares_amount);

    // H-03: Use actual on-chain balances instead of state
    let actual_sol = ctx.accounts.sol_treasury.amount;
    let actual_usdc = ctx.accounts.usdc_treasury.amount;

    // C-01: Use u128 for intermediate math to prevent overflow
    let user_ratio_num = shares_amount;
    let user_ratio_den = vault.total_shares;

    let sol_to_withdraw = (actual_sol as u128)
        .checked_mul(user_ratio_num as u128)
        .and_then(|v| v.checked_div(user_ratio_den as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or(error!(VaultError::MathOverflow))?;

    let usdc_to_withdraw = (actual_usdc as u128)
        .checked_mul(user_ratio_num as u128)
        .and_then(|v| v.checked_div(user_ratio_den as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or(error!(VaultError::MathOverflow))?;

    // Check treasury has enough
    require!(
        sol_to_withdraw <= actual_sol,
        VaultError::WithdrawalExceedsTreasury
    );
    require!(
        usdc_to_withdraw <= actual_usdc,
        VaultError::WithdrawalExceedsTreasury
    );

    // Burn shares
    let cpi_accounts = Burn {
        mint: ctx.accounts.share_mint.to_account_info(),
        from: ctx.accounts.user_share_account.to_account_info(),
        authority: ctx.accounts.user.to_account_info(),
    };
    let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
    token::burn(cpi_ctx, shares_amount)?;

    // Transfer SOL from treasury to user
    if sol_to_withdraw > 0 {
        let vault_key = vault.key();
        let seeds = &[
            seeds::SOL_TREASURY,
            vault_key.as_ref(),
            &[vault.sol_treasury_bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.sol_treasury.to_account_info(),
            to: ctx.accounts.user_wsol_account.to_account_info(),
            authority: ctx.accounts.sol_treasury.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, sol_to_withdraw)?;
    }

    // Transfer USDC from treasury to user
    if usdc_to_withdraw > 0 {
        let vault_key = vault.key();
        let seeds = &[
            seeds::USDC_TREASURY,
            vault_key.as_ref(),
            &[vault.usdc_treasury_bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.usdc_treasury.to_account_info(),
            to: ctx.accounts.user_usdc_account.to_account_info(),
            authority: ctx.accounts.usdc_treasury.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, usdc_to_withdraw)?;
    }

    // Update vault state — M-03: proper error handling
    vault.treasury_sol = vault.treasury_sol.saturating_sub(sol_to_withdraw);
    vault.treasury_usdc = vault.treasury_usdc.saturating_sub(usdc_to_withdraw);
    vault.total_shares = vault.total_shares
        .checked_sub(shares_amount)
        .ok_or(error!(VaultError::MathOverflow))?;
    vault.tvl_usd = vault.tvl_usd.saturating_sub(withdrawal_value_usd);

    // Update user deposit record
    user_deposit.shares = user_deposit.shares
        .checked_sub(shares_amount)
        .ok_or(error!(VaultError::MathOverflow))?;
    user_deposit.total_withdrawn_usd = user_deposit
        .total_withdrawn_usd
        .checked_add(withdrawal_value_usd)
        .ok_or(error!(VaultError::MathOverflow))?;
    user_deposit.updated_at = current_time;

    emit!(WithdrawEvent {
        user: ctx.accounts.user.key(),
        shares_burned: shares_amount,
        sol_withdrawn: sol_to_withdraw,
        usdc_withdrawn: usdc_to_withdraw,
        withdrawal_value_usd,
    });

    Ok(())
}
