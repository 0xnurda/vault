use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};

use crate::errors::VaultError;
use crate::events::ProtocolFeeExtracted;
use crate::state::{seeds, Vault};

/// Extract accumulated protocol fees (10% of collected fees) to protocol_wallet.
#[derive(Accounts)]
pub struct ExtractProtocolFee<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
    )]
    pub vault: Box<Account<'info, Vault>>,

    #[account(
        mut,
        seeds = [seeds::TOKEN0_TREASURY, vault.key().as_ref()],
        bump = vault.token0_treasury_bump,
    )]
    pub token0_treasury: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [seeds::TOKEN1_TREASURY, vault.key().as_ref()],
        bump = vault.token1_treasury_bump,
    )]
    pub token1_treasury: Box<Account<'info, TokenAccount>>,

    /// Protocol wallet token0 account — destination for token0 fees.
    #[account(
        mut,
        constraint = protocol_token0_account.owner == vault.protocol_wallet @ VaultError::Unauthorized,
        constraint = protocol_token0_account.mint == token0_treasury.mint @ VaultError::InvalidMint,
    )]
    pub protocol_token0_account: Box<Account<'info, TokenAccount>>,

    /// Protocol wallet token1 account — destination for token1 fees.
    #[account(
        mut,
        constraint = protocol_token1_account.owner == vault.protocol_wallet @ VaultError::Unauthorized,
        constraint = protocol_token1_account.mint == token1_treasury.mint @ VaultError::InvalidMint,
    )]
    pub protocol_token1_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<ExtractProtocolFee>) -> Result<()> {
    let vault = &ctx.accounts.vault;

    require!(
        vault.accumulated_protocol_fees_token0 > 0 || vault.accumulated_protocol_fees_token1 > 0,
        VaultError::NoFeesToExtract
    );

    let token0_to_extract = vault.accumulated_protocol_fees_token0;
    let token1_to_extract = vault.accumulated_protocol_fees_token1;

    if token0_to_extract > 0 {
        let vault_key = vault.key();
        let seeds = &[seeds::TOKEN0_TREASURY, vault_key.as_ref(), &[vault.token0_treasury_bump]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.token0_treasury.to_account_info(),
                    to: ctx.accounts.protocol_token0_account.to_account_info(),
                    authority: ctx.accounts.token0_treasury.to_account_info(),
                },
                &[&seeds[..]],
            ),
            token0_to_extract,
        )?;
    }

    if token1_to_extract > 0 {
        let vault_key = vault.key();
        let seeds = &[seeds::TOKEN1_TREASURY, vault_key.as_ref(), &[vault.token1_treasury_bump]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.token1_treasury.to_account_info(),
                    to: ctx.accounts.protocol_token1_account.to_account_info(),
                    authority: ctx.accounts.token1_treasury.to_account_info(),
                },
                &[&seeds[..]],
            ),
            token1_to_extract,
        )?;
    }

    // Reload actual balances before updating the cache — ensures
    // vault.treasury_token0/token1 is always ground-truth after extraction.
    ctx.accounts.token0_treasury.reload()?;
    ctx.accounts.token1_treasury.reload()?;

    let vault = &mut ctx.accounts.vault;
    vault.treasury_token0 = ctx.accounts.token0_treasury.amount;
    vault.treasury_token1 = ctx.accounts.token1_treasury.amount;
    vault.accumulated_protocol_fees_token0 = 0;
    vault.accumulated_protocol_fees_token1 = 0;

    emit!(ProtocolFeeExtracted {
        token0_amount: token0_to_extract,
        token1_amount: token1_to_extract,
        protocol_wallet: vault.protocol_wallet,
    });

    Ok(())
}
