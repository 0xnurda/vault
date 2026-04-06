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
use crate::events::FeesCollected;
use crate::state::{seeds, Vault};

/// Collect accumulated trading fees from the position.
/// 10% of fees → accumulated_protocol_fees (tracked separately, excluded from TVL).
/// 90% stays in treasury → increases user TVL (users profit via share price appreciation).
#[derive(Accounts)]
pub struct CollectFees<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
        constraint = vault.has_active_position @ VaultError::NoActivePosition,
    )]
    pub vault: Box<Account<'info, Vault>>,

    #[account(
        mut,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [seeds::USDC_TREASURY, vault.key().as_ref()],
        bump = vault.usdc_treasury_bump,
    )]
    pub usdc_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

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

pub fn handler(ctx: Context<CollectFees>) -> Result<()> {
    let vault = &ctx.accounts.vault;

    let vault_seeds: &[&[&[u8]]] = &[&[seeds::VAULT, &[vault.bump]]];

    let sol_before = ctx.accounts.sol_treasury.amount;
    let usdc_before = ctx.accounts.usdc_treasury.amount;

    // Calling decrease_liquidity_v2 with 0 liquidity collects accumulated fees
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

    cpi::decrease_liquidity_v2(
        CpiContext::new_with_signer(
            ctx.accounts.clmm_program.to_account_info(),
            cpi_accounts,
            vault_seeds,
        ),
        0,
        0,
        0,
    )?;

    ctx.accounts.sol_treasury.reload()?;
    ctx.accounts.usdc_treasury.reload()?;

    let total_sol_fees = ctx.accounts.sol_treasury.amount.saturating_sub(sol_before);
    let total_usdc_fees = ctx.accounts.usdc_treasury.amount.saturating_sub(usdc_before);

    // 10% to protocol, rounded down (protocol gets slightly less in edge cases)
    let protocol_sol = total_sol_fees / 10;
    let protocol_usdc = total_usdc_fees / 10;

    let vault = &mut ctx.accounts.vault;

    // Accumulate protocol fees (excluded from TVL — see state.rs::calculate_tvl)
    vault.accumulated_protocol_fees_sol = vault.accumulated_protocol_fees_sol
        .checked_add(protocol_sol)
        .ok_or(error!(VaultError::MathOverflow))?;
    vault.accumulated_protocol_fees_usdc = vault.accumulated_protocol_fees_usdc
        .checked_add(protocol_usdc)
        .ok_or(error!(VaultError::MathOverflow))?;

    // Update treasury balances (includes both user 90% and protocol 10%)
    vault.treasury_sol = ctx.accounts.sol_treasury.amount;
    vault.treasury_usdc = ctx.accounts.usdc_treasury.amount;

    // 90% stays in treasury → TVL increases → share price increases for all users

    emit!(FeesCollected {
        total_sol_fees,
        total_usdc_fees,
        protocol_sol_fees: protocol_sol,
        protocol_usdc_fees: protocol_usdc,
    });

    Ok(())
}
