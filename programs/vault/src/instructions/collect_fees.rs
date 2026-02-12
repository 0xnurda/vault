use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use anchor_spl::token_2022::Token2022;
use anchor_spl::memo::Memo;
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_clmm_cpi::{
    cpi,
    states::{PoolState, PersonalPositionState, TickArrayState},
};

use crate::errors::VaultError;
use crate::state::{seeds, Vault};

/// Collect accumulated trading fees from the position.
/// In Raydium CLMM, fees are collected by calling decrease_liquidity with 0 liquidity.
#[derive(Accounts)]
pub struct CollectFees<'info> {
    /// Admin collecting fees
    #[account(mut)]
    pub admin: Signer<'info>,

    /// Vault state
    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
        constraint = vault.has_active_position @ VaultError::NoActivePosition,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// SOL treasury PDA (destination for token0 fees)
    #[account(
        mut,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    /// USDC treasury PDA (destination for token1 fees)
    #[account(
        mut,
        seeds = [seeds::USDC_TREASURY, vault.key().as_ref()],
        bump = vault.usdc_treasury_bump,
    )]
    pub usdc_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    // ============ Raydium CLMM accounts ============

    /// Pool state
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Position NFT account (owned by vault)
    #[account(
        constraint = position_nft_account.amount == 1,
        constraint = position_nft_account.mint == vault.position_mint @ VaultError::InvalidPosition,
    )]
    pub position_nft_account: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Personal position state
    #[account(
        mut,
        constraint = personal_position.pool_id == pool_state.key(),
    )]
    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    /// Token vault 0 (pool's SOL vault)
    #[account(mut)]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Token vault 1 (pool's USDC vault)
    #[account(mut)]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Tick array for lower bound
    #[account(mut)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,

    /// Tick array for upper bound
    #[account(mut)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    /// Mint of vault 0
    pub vault_0_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Mint of vault 1
    pub vault_1_mint: Box<InterfaceAccount<'info, Mint>>,

    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub memo_program: Program<'info, Memo>,
}

pub fn handler(ctx: Context<CollectFees>) -> Result<()> {
    let vault = &ctx.accounts.vault;

    // Build signer seeds for vault PDA
    let vault_seeds: &[&[&[u8]]] = &[&[
        seeds::VAULT,
        &[vault.bump],
    ]];

    // Record balances before
    let sol_before = ctx.accounts.sol_treasury.amount;
    let usdc_before = ctx.accounts.usdc_treasury.amount;

    // Build CPI context
    // Calling decrease_liquidity with 0 liquidity collects accumulated fees
    let cpi_accounts = cpi::accounts::DecreaseLiquidityV2 {
        nft_owner: ctx.accounts.vault.to_account_info(),
        nft_account: ctx.accounts.position_nft_account.to_account_info(),
        personal_position: ctx.accounts.personal_position.to_account_info(),
        pool_state: ctx.accounts.pool_state.to_account_info(),
        protocol_position: ctx.accounts.personal_position.to_account_info(),
        token_vault_0: ctx.accounts.token_vault_0.to_account_info(),
        token_vault_1: ctx.accounts.token_vault_1.to_account_info(),
        tick_array_lower: ctx.accounts.tick_array_lower.to_account_info(),
        tick_array_upper: ctx.accounts.tick_array_upper.to_account_info(),
        recipient_token_account_0: ctx.accounts.sol_treasury.to_account_info(),
        recipient_token_account_1: ctx.accounts.usdc_treasury.to_account_info(),
        token_program: ctx.accounts.token_program.to_account_info(),
        token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
        memo_program: ctx.accounts.memo_program.to_account_info(),
        vault_0_mint: ctx.accounts.vault_0_mint.to_account_info(),
        vault_1_mint: ctx.accounts.vault_1_mint.to_account_info(),
    };

    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.pool_state.to_account_info(),
        cpi_accounts,
        vault_seeds,
    );

    // Execute CPI with 0 liquidity to collect fees only
    cpi::decrease_liquidity_v2(cpi_ctx, 0, 0, 0)?;

    // Reload treasuries
    ctx.accounts.sol_treasury.reload()?;
    ctx.accounts.usdc_treasury.reload()?;

    // Calculate fees collected
    let sol_fees = ctx.accounts.sol_treasury.amount.saturating_sub(sol_before);
    let usdc_fees = ctx.accounts.usdc_treasury.amount.saturating_sub(usdc_before);

    // Update vault treasury balances
    let vault = &mut ctx.accounts.vault;
    vault.treasury_sol = ctx.accounts.sol_treasury.amount;
    vault.treasury_usdc = ctx.accounts.usdc_treasury.amount;

    msg!("Fees collected");
    msg!("SOL fees: {}", sol_fees);
    msg!("USDC fees: {}", usdc_fees);

    Ok(())
}
