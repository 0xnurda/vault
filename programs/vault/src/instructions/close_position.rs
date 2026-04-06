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
use crate::events::PositionClosed;
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

    /// Raydium CLMM program
    /// CHECK: Validated by address constraint
    #[account(address = raydium_clmm_cpi::id())]
    pub clmm_program: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub memo_program: Program<'info, Memo>,
    pub system_program: Program<'info, System>,
}

pub fn handler<'a, 'b, 'c: 'info, 'info>(ctx: Context<'a, 'b, 'c, 'info, ClosePosition<'info>>, amount_0_min: u64, amount_1_min: u64) -> Result<()> {
    // Collect remaining_accounts before any other borrows from ctx to avoid lifetime conflicts.
    // These are the reward token accounts required by Raydium's decrease_liquidity_v2
    // when the pool has initialized rewards: [userRewardAta, rewardVault] per reward.
    let remaining = ctx.remaining_accounts.to_vec();

    let vault = &ctx.accounts.vault;
    // Use actual liquidity from personal_position, not stored value
    let liquidity = ctx.accounts.personal_position.liquidity;

    // Build signer seeds for vault PDA
    let vault_seeds: &[&[&[u8]]] = &[&[
        seeds::VAULT,
        &[vault.bump],
    ]];

    // Only decrease liquidity if position has non-zero liquidity
    if liquidity > 0 {
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

        // Forward remaining_accounts so Raydium receives reward token accounts
        // (required by decrease_liquidity_v2 when pool has initialized rewards).
        let decrease_ctx = CpiContext::new_with_signer(
            ctx.accounts.clmm_program.to_account_info(),
            decrease_accounts,
            vault_seeds,
        ).with_remaining_accounts(remaining);

        // H-02: Use slippage params instead of 0
        cpi::decrease_liquidity_v2(decrease_ctx, liquidity, amount_0_min, amount_1_min)?;
    }

    // Then close the position
    let close_accounts = cpi::accounts::ClosePosition {
        nft_owner: ctx.accounts.vault.to_account_info(),
        position_nft_mint: ctx.accounts.position_nft_mint.to_account_info(),
        position_nft_account: ctx.accounts.position_nft_account.to_account_info(),
        personal_position: ctx.accounts.personal_position.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        token_program: ctx.accounts.token_program_2022.to_account_info(),
    };

    let close_ctx = CpiContext::new_with_signer(
        ctx.accounts.clmm_program.to_account_info(),
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
    // ARCH-001: block user deposits/withdrawals during rebalance window
    // Will be cleared by open_position when new position is opened
    vault.is_rebalancing = true;

    emit!(PositionClosed {
        treasury_sol: vault.treasury_sol,
        treasury_usdc: vault.treasury_usdc,
    });

    Ok(())
}
