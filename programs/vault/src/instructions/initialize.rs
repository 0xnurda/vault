use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::events::VaultInitialized;
use crate::state::{seeds, Vault};

#[derive(Accounts)]
pub struct Initialize<'info> {
    /// Admin who will manage the vault
    #[account(mut)]
    pub admin: Signer<'info>,

    /// Vault state account (PDA)
    #[account(
        init,
        payer = admin,
        space = Vault::LEN,
        seeds = [seeds::VAULT],
        bump,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// Share token mint (PDA)
    #[account(
        init,
        payer = admin,
        seeds = [seeds::SHARE_MINT, vault.key().as_ref()],
        bump,
        mint::decimals = 6,
        mint::authority = share_mint,
    )]
    pub share_mint: Box<Account<'info, Mint>>,

    /// SOL treasury token account (holds wSOL)
    #[account(
        init,
        payer = admin,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump,
        token::mint = wsol_mint,
        token::authority = sol_treasury,
    )]
    pub sol_treasury: Box<Account<'info, TokenAccount>>,

    /// USDC treasury token account
    #[account(
        init,
        payer = admin,
        seeds = [seeds::USDC_TREASURY, vault.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = usdc_treasury,
    )]
    pub usdc_treasury: Box<Account<'info, TokenAccount>>,

    /// Wrapped SOL mint
    pub wsol_mint: Box<Account<'info, Mint>>,

    /// USDC mint (EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v on mainnet)
    pub usdc_mint: Box<Account<'info, Mint>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn handler(
    ctx: Context<Initialize>,
    protocol_wallet: Pubkey,
    sol_price_feed: Pubkey,
) -> Result<()> {
    let vault = &mut ctx.accounts.vault;

    vault.admin = ctx.accounts.admin.key();
    vault.share_mint = ctx.accounts.share_mint.key();
    vault.sol_treasury = ctx.accounts.sol_treasury.key();
    vault.usdc_treasury = ctx.accounts.usdc_treasury.key();
    vault.usdc_mint = ctx.accounts.usdc_mint.key();
    vault.protocol_wallet = protocol_wallet;
    vault.sol_price_feed = sol_price_feed;

    vault.total_shares = 0;
    vault.treasury_sol = 0;
    vault.treasury_usdc = 0;
    vault.accumulated_protocol_fees_sol = 0;
    vault.accumulated_protocol_fees_usdc = 0;

    vault.bump = ctx.bumps.vault;
    vault.sol_treasury_bump = ctx.bumps.sol_treasury;
    vault.usdc_treasury_bump = ctx.bumps.usdc_treasury;
    vault.share_mint_bump = ctx.bumps.share_mint;

    vault.is_paused = false;
    vault.is_rebalancing = false;

    emit!(VaultInitialized {
        admin: vault.admin,
        protocol_wallet: vault.protocol_wallet,
        share_mint: vault.share_mint,
        sol_treasury: vault.sol_treasury,
        usdc_treasury: vault.usdc_treasury,
        sol_price_feed: vault.sol_price_feed,
    });

    Ok(())
}
