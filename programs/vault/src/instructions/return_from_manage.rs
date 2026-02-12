use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::VaultError;
use crate::state::{seeds, Vault};

#[derive(Accounts)]
pub struct ReturnFromManage<'info> {
    /// Admin returning funds
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

    /// Admin's wSOL account (source for SOL)
    #[account(
        mut,
        constraint = admin_wsol_account.owner == admin.key(),
    )]
    pub admin_wsol_account: Account<'info, TokenAccount>,

    /// Admin's USDC account (source for USDC)
    #[account(
        mut,
        constraint = admin_usdc_account.owner == admin.key(),
    )]
    pub admin_usdc_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(
    ctx: Context<ReturnFromManage>,
    sol_amount: u64,
    usdc_amount: u64,
) -> Result<()> {
    require!(
        sol_amount > 0 || usdc_amount > 0,
        VaultError::InvalidAmount
    );

    let vault = &mut ctx.accounts.vault;

    // Check admin has enough funds to return
    require!(
        sol_amount <= ctx.accounts.admin_wsol_account.amount,
        VaultError::InsufficientTreasuryBalance
    );
    require!(
        usdc_amount <= ctx.accounts.admin_usdc_account.amount,
        VaultError::InsufficientTreasuryBalance
    );

    // Transfer SOL from admin to treasury
    if sol_amount > 0 {
        let cpi_accounts = Transfer {
            from: ctx.accounts.admin_wsol_account.to_account_info(),
            to: ctx.accounts.sol_treasury.to_account_info(),
            authority: ctx.accounts.admin.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
        );
        token::transfer(cpi_ctx, sol_amount)?;

        // Update vault state (treasury balance increased)
        vault.treasury_sol = vault.treasury_sol.checked_add(sol_amount).unwrap();
    }

    // Transfer USDC from admin to treasury
    if usdc_amount > 0 {
        let cpi_accounts = Transfer {
            from: ctx.accounts.admin_usdc_account.to_account_info(),
            to: ctx.accounts.usdc_treasury.to_account_info(),
            authority: ctx.accounts.admin.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
        );
        token::transfer(cpi_ctx, usdc_amount)?;

        // Update vault state
        vault.treasury_usdc = vault.treasury_usdc.checked_add(usdc_amount).unwrap();
    }

    // Note: TVL should be updated via update_tvl instruction after this
    // to reflect the new treasury balance + any remaining positions

    msg!("Admin returned {} lamports SOL to treasury", sol_amount);
    msg!("Admin returned {} USDC to treasury", usdc_amount);

    Ok(())
}
