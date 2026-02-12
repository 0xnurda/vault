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

#[derive(Accounts)]
pub struct ClosePosition<'info> {
    /// Admin closing the position
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

    /// SOL treasury PDA (destination for token0)
    #[account(
        mut,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    /// USDC treasury PDA (destination for token1)
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

    /// Position NFT mint
    #[account(
        mut,
        constraint = position_nft_mint.key() == vault.position_mint @ VaultError::InvalidPosition,
    )]
    pub position_nft_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Position NFT account (owned by vault)
    #[account(
        mut,
        constraint = position_nft_account.amount == 1,
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
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<ClosePosition>) -> Result<()> {
    let vault = &ctx.accounts.vault;
    let liquidity = vault.position_liquidity;

    require!(liquidity > 0, VaultError::NoActivePosition);

    // Build signer seeds for vault PDA
    let vault_seeds: &[&[&[u8]]] = &[&[
        seeds::VAULT,
        &[vault.bump],
    ]];

    // First, decrease all liquidity
    let decrease_accounts = cpi::accounts::DecreaseLiquidityV2 {
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

    let decrease_ctx = CpiContext::new_with_signer(
        ctx.accounts.pool_state.to_account_info(),
        decrease_accounts,
        vault_seeds,
    );

    // Decrease all liquidity with 0 minimum (accept any amount)
    cpi::decrease_liquidity_v2(decrease_ctx, liquidity, 0, 0)?;

    // Then close the position
    let close_accounts = cpi::accounts::ClosePosition {
        nft_owner: ctx.accounts.vault.to_account_info(),
        position_nft_mint: ctx.accounts.position_nft_mint.to_account_info(),
        position_nft_account: ctx.accounts.position_nft_account.to_account_info(),
        personal_position: ctx.accounts.personal_position.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        token_program: ctx.accounts.token_program.to_account_info(),
    };

    let close_ctx = CpiContext::new_with_signer(
        ctx.accounts.pool_state.to_account_info(),
        close_accounts,
        vault_seeds,
    );

    cpi::close_position(close_ctx)?;

    // Reload treasuries
    ctx.accounts.sol_treasury.reload()?;
    ctx.accounts.usdc_treasury.reload()?;

    // Update vault state
    let vault = &mut ctx.accounts.vault;
    vault.has_active_position = false;
    vault.position_mint = Pubkey::default();
    vault.position_pool_id = Pubkey::default();
    vault.position_tick_lower = 0;
    vault.position_tick_upper = 0;
    vault.position_liquidity = 0;
    vault.position_sol = 0;
    vault.position_usdc = 0;
    vault.treasury_sol = ctx.accounts.sol_treasury.amount;
    vault.treasury_usdc = ctx.accounts.usdc_treasury.amount;

    msg!("Position closed");
    msg!("Treasury SOL: {}", vault.treasury_sol);
    msg!("Treasury USDC: {}", vault.treasury_usdc);

    Ok(())
}
