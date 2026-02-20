use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};

use crate::events::WalletCreated;
use crate::state::{seeds, SmartWallet};

#[derive(Accounts)]
pub struct CreateWallet<'info> {
    /// User creating their smart wallet
    #[account(mut)]
    pub user: Signer<'info>,

    /// Smart wallet state (PDA per user)
    #[account(
        init,
        payer = user,
        space = SmartWallet::LEN,
        seeds = [seeds::SMART_WALLET, user.key().as_ref()],
        bump,
    )]
    pub wallet: Box<Account<'info, SmartWallet>>,

    /// SOL treasury (wSOL token account, self-authority)
    #[account(
        init,
        payer = user,
        seeds = [seeds::WALLET_SOL_TREASURY, wallet.key().as_ref()],
        bump,
        token::mint = wsol_mint,
        token::authority = sol_treasury,
    )]
    pub sol_treasury: Box<Account<'info, TokenAccount>>,

    /// USDC treasury (USDC token account, self-authority)
    #[account(
        init,
        payer = user,
        seeds = [seeds::WALLET_USDC_TREASURY, wallet.key().as_ref()],
        bump,
        token::mint = usdc_mint,
        token::authority = usdc_treasury,
    )]
    pub usdc_treasury: Box<Account<'info, TokenAccount>>,

    /// Wrapped SOL mint
    #[account(
        constraint = wsol_mint.key() == anchor_spl::token::spl_token::native_mint::ID,
    )]
    pub wsol_mint: Box<Account<'info, Mint>>,

    /// USDC mint
    pub usdc_mint: Box<Account<'info, Mint>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn handler(ctx: Context<CreateWallet>) -> Result<()> {
    let wallet = &mut ctx.accounts.wallet;

    wallet.owner = ctx.accounts.user.key();
    wallet.delegate = Pubkey::default();
    wallet.sol_treasury = ctx.accounts.sol_treasury.key();
    wallet.usdc_treasury = ctx.accounts.usdc_treasury.key();
    wallet.usdc_mint = ctx.accounts.usdc_mint.key();

    wallet.position_mint = Pubkey::default();
    wallet.position_pool_id = Pubkey::default();
    wallet.has_active_position = false;
    wallet.position_liquidity = 0;
    wallet.position_tick_lower = 0;
    wallet.position_tick_upper = 0;
    wallet.position_sol = 0;
    wallet.position_usdc = 0;

    wallet.bump = ctx.bumps.wallet;
    wallet.sol_treasury_bump = ctx.bumps.sol_treasury;
    wallet.usdc_treasury_bump = ctx.bumps.usdc_treasury;
    wallet.is_paused = false;

    let now = Clock::get()?.unix_timestamp;
    wallet.created_at = now;
    wallet.updated_at = now;

    emit!(WalletCreated {
        owner: wallet.owner,
        sol_treasury: wallet.sol_treasury,
        usdc_treasury: wallet.usdc_treasury,
    });

    Ok(())
}
