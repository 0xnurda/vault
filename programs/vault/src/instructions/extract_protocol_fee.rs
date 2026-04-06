use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::VaultError;
use crate::events::ProtocolFeeExtracted;
use crate::state::{seeds, Vault};

/// Extract accumulated protocol fees (10% of collected fees) to protocol_wallet.
/// Can be called at any time by admin — once a week, once a month, etc.
/// TVL is not affected: accumulated_protocol_fees were already excluded from TVL in calculate_tvl().
#[derive(Accounts)]
pub struct ExtractProtocolFee<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// SOL treasury (source for SOL fees)
    #[account(
        mut,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Box<Account<'info, TokenAccount>>,

    /// USDC treasury (source for USDC fees)
    #[account(
        mut,
        seeds = [seeds::USDC_TREASURY, vault.key().as_ref()],
        bump = vault.usdc_treasury_bump,
    )]
    pub usdc_treasury: Box<Account<'info, TokenAccount>>,

    /// Protocol wallet SOL (wSOL) token account — destination for SOL fees.
    /// Must be owned by vault.protocol_wallet.
    #[account(
        mut,
        constraint = protocol_sol_account.owner == vault.protocol_wallet @ VaultError::Unauthorized,
        constraint = protocol_sol_account.mint == sol_treasury.mint @ VaultError::InvalidMint,
    )]
    pub protocol_sol_account: Box<Account<'info, TokenAccount>>,

    /// Protocol wallet USDC token account — destination for USDC fees.
    /// Must be owned by vault.protocol_wallet.
    #[account(
        mut,
        constraint = protocol_usdc_account.owner == vault.protocol_wallet @ VaultError::Unauthorized,
        constraint = protocol_usdc_account.mint == usdc_treasury.mint @ VaultError::InvalidMint,
    )]
    pub protocol_usdc_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<ExtractProtocolFee>) -> Result<()> {
    let vault = &ctx.accounts.vault;

    require!(
        vault.accumulated_protocol_fees_sol > 0 || vault.accumulated_protocol_fees_usdc > 0,
        VaultError::NoFeesToExtract
    );

    let sol_to_extract = vault.accumulated_protocol_fees_sol;
    let usdc_to_extract = vault.accumulated_protocol_fees_usdc;

    // Transfer SOL fees to protocol wallet
    if sol_to_extract > 0 {
        let vault_key = vault.key();
        let seeds = &[seeds::SOL_TREASURY, vault_key.as_ref(), &[vault.sol_treasury_bump]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.sol_treasury.to_account_info(),
                    to: ctx.accounts.protocol_sol_account.to_account_info(),
                    authority: ctx.accounts.sol_treasury.to_account_info(),
                },
                &[&seeds[..]],
            ),
            sol_to_extract,
        )?;
    }

    // Transfer USDC fees to protocol wallet
    if usdc_to_extract > 0 {
        let vault_key = vault.key();
        let seeds = &[seeds::USDC_TREASURY, vault_key.as_ref(), &[vault.usdc_treasury_bump]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.usdc_treasury.to_account_info(),
                    to: ctx.accounts.protocol_usdc_account.to_account_info(),
                    authority: ctx.accounts.usdc_treasury.to_account_info(),
                },
                &[&seeds[..]],
            ),
            usdc_to_extract,
        )?;
    }

    // Update vault state
    let vault = &mut ctx.accounts.vault;
    vault.treasury_sol = vault.treasury_sol.saturating_sub(sol_to_extract);
    vault.treasury_usdc = vault.treasury_usdc.saturating_sub(usdc_to_extract);
    vault.accumulated_protocol_fees_sol = 0;
    vault.accumulated_protocol_fees_usdc = 0;

    emit!(ProtocolFeeExtracted {
        sol_amount: sol_to_extract,
        usdc_amount: usdc_to_extract,
        protocol_wallet: vault.protocol_wallet,
    });

    Ok(())
}
