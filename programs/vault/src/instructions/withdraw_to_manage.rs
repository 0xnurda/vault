use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::VaultError;
use crate::events::WithdrawToManageEvent;
use crate::state::{seeds, Vault};

#[derive(Accounts)]
pub struct WithdrawToManage<'info> {
    /// Admin performing the withdrawal
    #[account(mut)]
    pub admin: Signer<'info>,

    /// Vault state
    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
    )]
    pub vault: Account<'info, Vault>,

    /// SOL treasury PDA
    #[account(
        mut,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Account<'info, TokenAccount>,

    /// USDC treasury PDA
    #[account(
        mut,
        seeds = [seeds::USDC_TREASURY, vault.key().as_ref()],
        bump = vault.usdc_treasury_bump,
    )]
    pub usdc_treasury: Account<'info, TokenAccount>,

    /// Admin's wSOL account (destination for SOL)
    #[account(
        mut,
        constraint = admin_wsol_account.owner == admin.key(),
    )]
    pub admin_wsol_account: Account<'info, TokenAccount>,

    /// Admin's USDC account (destination for USDC)
    #[account(
        mut,
        constraint = admin_usdc_account.owner == admin.key(),
    )]
    pub admin_usdc_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(
    ctx: Context<WithdrawToManage>,
    sol_amount: u64,
    usdc_amount: u64,
) -> Result<()> {
    require!(
        sol_amount > 0 || usdc_amount > 0,
        VaultError::InvalidAmount
    );

    let vault = &mut ctx.accounts.vault;

    // Check treasury has enough funds
    require!(
        sol_amount <= ctx.accounts.sol_treasury.amount,
        VaultError::InsufficientTreasuryBalance
    );
    require!(
        usdc_amount <= ctx.accounts.usdc_treasury.amount,
        VaultError::InsufficientTreasuryBalance
    );

    // L-04: Limit withdrawals to max 50% of each treasury per call
    if sol_amount > 0 {
        require!(
            sol_amount <= ctx.accounts.sol_treasury.amount / 2,
            VaultError::WithdrawalExceedsTreasury
        );
    }
    if usdc_amount > 0 {
        require!(
            usdc_amount <= ctx.accounts.usdc_treasury.amount / 2,
            VaultError::WithdrawalExceedsTreasury
        );
    }

    // Transfer SOL from treasury to admin
    if sol_amount > 0 {
        let vault_key = vault.key();
        let seeds = &[
            seeds::SOL_TREASURY,
            vault_key.as_ref(),
            &[vault.sol_treasury_bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.sol_treasury.to_account_info(),
            to: ctx.accounts.admin_wsol_account.to_account_info(),
            authority: ctx.accounts.sol_treasury.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, sol_amount)?;

        // Update vault state (treasury balance reduced)
        vault.treasury_sol = vault.treasury_sol.checked_sub(sol_amount).ok_or(error!(VaultError::MathOverflow))?;
    }

    // Transfer USDC from treasury to admin
    if usdc_amount > 0 {
        let vault_key = vault.key();
        let seeds = &[
            seeds::USDC_TREASURY,
            vault_key.as_ref(),
            &[vault.usdc_treasury_bump],
        ];
        let signer_seeds = &[&seeds[..]];

        let cpi_accounts = Transfer {
            from: ctx.accounts.usdc_treasury.to_account_info(),
            to: ctx.accounts.admin_usdc_account.to_account_info(),
            authority: ctx.accounts.usdc_treasury.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            signer_seeds,
        );
        token::transfer(cpi_ctx, usdc_amount)?;

        // Update vault state
        vault.treasury_usdc = vault.treasury_usdc.checked_sub(usdc_amount).ok_or(error!(VaultError::MathOverflow))?;
    }

    // Note: TVL is NOT reduced here because funds are still "managed"
    // TVL will be updated by update_tvl instruction which includes position values

    emit!(WithdrawToManageEvent {
        admin: ctx.accounts.admin.key(),
        sol_amount,
        usdc_amount,
    });

    Ok(())
}
