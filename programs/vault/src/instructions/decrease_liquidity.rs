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
use crate::events::LiquidityDecreased;
use crate::state::{seeds, Vault};

#[derive(Accounts)]
pub struct DecreaseLiquidity<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
        constraint = vault.is_operator(&admin.key()) @ VaultError::Unauthorized,
        constraint = vault.has_active_position @ VaultError::NoActivePosition,
    )]
    pub vault: Box<Account<'info, Vault>>,

    #[account(
        mut,
        seeds = [seeds::TOKEN0_TREASURY, vault.key().as_ref()],
        bump = vault.token0_treasury_bump,
    )]
    pub token0_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [seeds::TOKEN1_TREASURY, vault.key().as_ref()],
        bump = vault.token1_treasury_bump,
    )]
    pub token1_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    #[account(
        constraint = position_nft_account.amount == 1,
        constraint = position_nft_account.mint == vault.position_mint @ VaultError::InvalidPosition,
    )]
    pub position_nft_account: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = personal_position.pool_id == pool_state.key(),
    )]
    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    #[account(mut)]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,

    #[account(mut)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    pub vault_0_mint: Box<InterfaceAccount<'info, Mint>>,
    pub vault_1_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: Validated by address constraint
    #[account(address = raydium_clmm_cpi::id())]
    pub clmm_program: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub memo_program: Program<'info, Memo>,
}

pub fn handler<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, DecreaseLiquidity<'info>>,
    liquidity: u128,
    amount_0_min: u64,
    amount_1_min: u64,
) -> Result<()> {
    // Must clone remaining_accounts before any borrow of ctx.
    // Both CPIs need them (reward token accounts for pools with initialized rewards).
    let remaining = ctx.remaining_accounts.to_vec();

    let vault = &ctx.accounts.vault;

    require!(liquidity > 0, VaultError::InvalidAmount);
    require!(
        liquidity <= vault.position_liquidity,
        VaultError::InvalidAmount
    );

    let pool_id = vault.pool_id;
    let vault_seeds: &[&[&[u8]]] = &[&[seeds::VAULT, pool_id.as_ref(), &[vault.bump]]];

    // ── Lean partial withdraw (Kamino-style): single CPI, no fee split ────
    // Trading fees are harvested separately by `collect_fees` (10% taken there).
    // This removes the requested liquidity in ONE DecreaseLiquidityV2 CPI
    // (which also auto-collects residual fees into treasury → benefit users).
    let token0_before_liq = ctx.accounts.token0_treasury.amount;
    let token1_before_liq = ctx.accounts.token1_treasury.amount;

    let liq_cpi_accounts = cpi::accounts::DecreaseLiquidityV2 {
        nft_owner: ctx.accounts.vault.to_account_info(),
        nft_account: ctx.accounts.position_nft_account.to_account_info(),
        personal_position: ctx.accounts.personal_position.to_account_info(),
        pool_state: ctx.accounts.pool_state.to_account_info(),
        protocol_position: ctx.accounts.personal_position.to_account_info(),
        token_vault_0: ctx.accounts.token_vault_0.to_account_info(),
        token_vault_1: ctx.accounts.token_vault_1.to_account_info(),
        tick_array_lower: ctx.accounts.tick_array_lower.to_account_info(),
        tick_array_upper: ctx.accounts.tick_array_upper.to_account_info(),
        recipient_token_account_0: ctx.accounts.token0_treasury.to_account_info(),
        recipient_token_account_1: ctx.accounts.token1_treasury.to_account_info(),
        token_program: ctx.accounts.token_program.to_account_info(),
        token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
        memo_program: ctx.accounts.memo_program.to_account_info(),
        vault_0_mint: ctx.accounts.vault_0_mint.to_account_info(),
        vault_1_mint: ctx.accounts.vault_1_mint.to_account_info(),
    };

    cpi::decrease_liquidity_v2(
        CpiContext::new_with_signer(
            ctx.accounts.clmm_program.to_account_info(),
            liq_cpi_accounts,
            vault_seeds,
        )
        .with_remaining_accounts(remaining),
        liquidity,
        amount_0_min,
        amount_1_min,
    )?;

    ctx.accounts.token0_treasury.reload()?;
    ctx.accounts.token1_treasury.reload()?;

    let token0_removed = ctx.accounts.token0_treasury.amount.saturating_sub(token0_before_liq);
    let token1_removed = ctx.accounts.token1_treasury.amount.saturating_sub(token1_before_liq);

    // ── Update vault state ────────────────────────────────────────────────
    let vault = &mut ctx.accounts.vault;
    vault.position_liquidity = vault.position_liquidity.saturating_sub(liquidity);
    vault.position_token0 = vault.position_token0.saturating_sub(token0_removed);
    vault.position_token1 = vault.position_token1.saturating_sub(token1_removed);
    vault.treasury_token0 = ctx.accounts.token0_treasury.amount;
    vault.treasury_token1 = ctx.accounts.token1_treasury.amount;

    // Note: protocol fee 10% is taken in `collect_fees`, not here (lean op).

    emit!(LiquidityDecreased {
        token0_received: token0_removed,
        token1_received: token1_removed,
        remaining_liquidity: vault.position_liquidity,
    });

    Ok(())
}
