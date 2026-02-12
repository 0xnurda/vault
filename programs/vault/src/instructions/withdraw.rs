use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, Token, TokenAccount, Transfer};

use crate::errors::VaultError;
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
    )]
    pub user_wsol_account: Box<Account<'info, TokenAccount>>,

    /// User's USDC token account (destination for USDC)
    #[account(
        mut,
        constraint = user_usdc_account.owner == user.key(),
    )]
    pub user_usdc_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<Withdraw>, shares_amount: u64) -> Result<()> {
    require!(shares_amount > 0, VaultError::InvalidAmount);

    let vault = &mut ctx.accounts.vault;
    let user_deposit = &mut ctx.accounts.user_deposit;

    // Check user has enough shares
    require!(
        user_deposit.shares >= shares_amount,
        VaultError::InsufficientShares
    );
    require!(
        ctx.accounts.user_share_account.amount >= shares_amount,
        VaultError::InsufficientShares
    );

    // Calculate withdrawal value in USD
    let withdrawal_value_usd = vault.calculate_withdrawal_value(shares_amount);

    // Calculate proportional SOL and USDC amounts from treasury
    // user_ratio = shares_amount / total_shares
    let user_ratio_num = shares_amount;
    let user_ratio_den = vault.total_shares;

    // SOL to withdraw = treasury_sol * user_ratio
    let sol_to_withdraw = vault
        .treasury_sol
        .checked_mul(user_ratio_num)
        .unwrap()
        .checked_div(user_ratio_den)
        .unwrap_or(0);

    // USDC to withdraw = treasury_usdc * user_ratio
    let usdc_to_withdraw = vault
        .treasury_usdc
        .checked_mul(user_ratio_num)
        .unwrap()
        .checked_div(user_ratio_den)
        .unwrap_or(0);

    // Check treasury has enough
    require!(
        sol_to_withdraw <= ctx.accounts.sol_treasury.amount,
        VaultError::WithdrawalExceedsTreasury
    );
    require!(
        usdc_to_withdraw <= ctx.accounts.usdc_treasury.amount,
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

    // Update vault state
    vault.treasury_sol = vault.treasury_sol.checked_sub(sol_to_withdraw).unwrap();
    vault.treasury_usdc = vault.treasury_usdc.checked_sub(usdc_to_withdraw).unwrap();
    vault.total_shares = vault.total_shares.checked_sub(shares_amount).unwrap();
    vault.tvl_usd = vault.tvl_usd.saturating_sub(withdrawal_value_usd);

    // Update user deposit record
    user_deposit.shares = user_deposit.shares.checked_sub(shares_amount).unwrap();
    user_deposit.total_withdrawn_usd = user_deposit
        .total_withdrawn_usd
        .checked_add(withdrawal_value_usd)
        .unwrap();
    user_deposit.updated_at = Clock::get()?.unix_timestamp;

    msg!("Burned {} shares", shares_amount);
    msg!("Withdrawn {} lamports SOL", sol_to_withdraw);
    msg!("Withdrawn {} USDC", usdc_to_withdraw);
    msg!("Withdrawal value: ${} USD", withdrawal_value_usd);

    Ok(())
}
