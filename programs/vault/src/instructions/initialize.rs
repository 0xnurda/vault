use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, Token, TokenAccount};
use raydium_clmm_cpi::states::PoolState;

use crate::errors::VaultError;
use crate::events::VaultInitialized;
use crate::state::{seeds, Vault};

#[derive(Accounts)]
pub struct Initialize<'info> {
    /// Payer for account creation (deployer keypair). Does NOT become vault admin.
    /// vault.admin is set via the explicit `admin` parameter in the handler.
    #[account(mut)]
    pub payer: Signer<'info>,

    /// Vault state account (PDA). Seeds include pool key → one vault per pool.
    #[account(
        init,
        payer = payer,
        space = Vault::LEN,
        seeds = [seeds::VAULT, pool.key().as_ref()],
        bump,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// Share token mint (PDA)
    #[account(
        init,
        payer = payer,
        seeds = [seeds::SHARE_MINT, vault.key().as_ref()],
        bump,
        mint::decimals = 6,
        mint::authority = share_mint,
    )]
    pub share_mint: Box<Account<'info, Mint>>,

    /// token0 treasury token account (holds token0, e.g. wSOL)
    #[account(
        init,
        payer = payer,
        seeds = [seeds::TOKEN0_TREASURY, vault.key().as_ref()],
        bump,
        token::mint = token0_mint,
        token::authority = token0_treasury,
    )]
    pub token0_treasury: Box<Account<'info, TokenAccount>>,

    /// token1 treasury token account (holds token1, e.g. USDC)
    #[account(
        init,
        payer = payer,
        seeds = [seeds::TOKEN1_TREASURY, vault.key().as_ref()],
        bump,
        token::mint = token1_mint,
        token::authority = token1_treasury,
    )]
    pub token1_treasury: Box<Account<'info, TokenAccount>>,

    /// Mint of token0 (e.g. wSOL)
    pub token0_mint: Box<Account<'info, Mint>>,

    /// Mint of token1 (e.g. USDC)
    pub token1_mint: Box<Account<'info, Mint>>,

    /// Raydium CLMM pool — typed AccountLoader validates ownership + discriminator (audit #6).
    /// Its key becomes vault.pool_id (immutable, part of seeds).
    pub pool: AccountLoader<'info, PoolState>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn handler(
    ctx: Context<Initialize>,
    admin: Pubkey,
    protocol_wallet: Pubkey,
) -> Result<()> {
    // ── Validate admin and protocol_wallet ───────────────────────────────────
    require!(admin != Pubkey::default(), VaultError::Unauthorized);
    require!(protocol_wallet != Pubkey::default(), VaultError::Unauthorized);

    // ── Validate token mints against pool (typed access, audit #6) ───────────
    // AccountLoader validates ownership (Raydium CLMM) + discriminator automatically.
    // Read token_mint_0 and token_mint_1 directly from the typed PoolState.
    let (pool_mint_0, pool_mint_1) = {
        let pool = ctx.accounts.pool.load()?;
        (pool.token_mint_0, pool.token_mint_1)
    };

    let token0_key = ctx.accounts.token0_mint.key();
    let token1_key = ctx.accounts.token1_mint.key();

    require!(token0_key != token1_key, VaultError::InvalidMint);

    // The two supplied mints must be exactly the pool's two mints (any order).
    require!(
        (token0_key == pool_mint_0 && token1_key == pool_mint_1)
            || (token0_key == pool_mint_1 && token1_key == pool_mint_0),
        VaultError::InvalidMint
    );

    // ── Read decimals from Anchor-validated mint accounts ─────────────────────
    // Bound them so 10^decimals never overflows/panics in the math (audit L-1).
    let token0_decimals = ctx.accounts.token0_mint.decimals;
    let token1_decimals = ctx.accounts.token1_mint.decimals;
    require!(token0_decimals <= 18 && token1_decimals <= 18, VaultError::InvalidMint);

    // ── Initialize vault ──────────────────────────────────────────────────────
    let vault = &mut ctx.accounts.vault;

    vault.admin = admin;
    vault.operator = admin;   // default operator = admin; change later via set_operator
    vault.swap_window_start = 0;
    vault.swap_volume_in_window = 0;
    vault.share_mint = ctx.accounts.share_mint.key();
    vault.pool_id = ctx.accounts.pool.key();
    vault.token0_mint = token0_key;
    vault.token1_mint = token1_key;
    vault.token0_treasury = ctx.accounts.token0_treasury.key();
    vault.token1_treasury = ctx.accounts.token1_treasury.key();
    vault.protocol_wallet = protocol_wallet;

    vault.total_shares = 0;
    vault.treasury_token0 = 0;
    vault.treasury_token1 = 0;
    vault.token0_decimals = token0_decimals;
    vault.token1_decimals = token1_decimals;
    vault.accumulated_protocol_fees_token0 = 0;
    vault.accumulated_protocol_fees_token1 = 0;

    vault.bump = ctx.bumps.vault;
    vault.token0_treasury_bump = ctx.bumps.token0_treasury;
    vault.token1_treasury_bump = ctx.bumps.token1_treasury;
    vault.share_mint_bump = ctx.bumps.share_mint;

    vault.is_paused = false;
    vault.is_rebalancing = false;
    vault.pending_admin = Pubkey::default();

    vault.has_active_position = false;
    vault.position_mint = Pubkey::default();
    vault.position_token0 = 0;
    vault.position_token1 = 0;
    vault.position_liquidity = 0;
    vault.position_tick_lower = 0;
    vault.position_tick_upper = 0;
    vault.rebalance_started_at = 0;

    emit!(VaultInitialized {
        admin: vault.admin,
        protocol_wallet: vault.protocol_wallet,
        share_mint: vault.share_mint,
        token0_treasury: vault.token0_treasury,
        token1_treasury: vault.token1_treasury,
        pool_id: vault.pool_id,
    });

    Ok(())
}
