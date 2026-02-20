use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use anchor_spl::token_2022::Token2022;
use anchor_spl::memo::Memo;
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_clmm_cpi::{
    cpi,
    states::{PoolState, PersonalPositionState, TickArrayState},
};

use crate::errors::WalletError;
use crate::events::WalletLiquidityDecreased;
use crate::state::{seeds, SmartWallet};

#[derive(Accounts)]
pub struct DecreaseLiquidity<'info> {
    /// Operator (delegate or owner)
    #[account(mut)]
    pub operator: Signer<'info>,

    /// Smart wallet state
    #[account(
        mut,
        constraint = wallet.owner == operator.key() || wallet.delegate == operator.key() @ WalletError::Unauthorized,
        constraint = wallet.has_active_position @ WalletError::NoActivePosition,
    )]
    pub wallet: Box<Account<'info, SmartWallet>>,

    /// SOL treasury PDA (destination for token0)
    #[account(
        mut,
        seeds = [seeds::WALLET_SOL_TREASURY, wallet.key().as_ref()],
        bump = wallet.sol_treasury_bump,
    )]
    pub sol_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    /// USDC treasury PDA (destination for token1)
    #[account(
        mut,
        seeds = [seeds::WALLET_USDC_TREASURY, wallet.key().as_ref()],
        bump = wallet.usdc_treasury_bump,
    )]
    pub usdc_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    // ============ Raydium CLMM accounts ============

    /// Pool state
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Position NFT account (owned by wallet PDA)
    #[account(
        constraint = position_nft_account.amount == 1,
        constraint = position_nft_account.mint == wallet.position_mint @ WalletError::InvalidPosition,
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
}

pub fn handler(
    ctx: Context<DecreaseLiquidity>,
    liquidity: u128,
    amount_0_min: u64,
    amount_1_min: u64,
) -> Result<()> {
    let wallet = &ctx.accounts.wallet;

    require!(liquidity > 0, WalletError::InvalidAmount);
    require!(
        liquidity <= wallet.position_liquidity,
        WalletError::InvalidAmount
    );

    // Build signer seeds for wallet PDA
    let owner_key = wallet.owner;
    let wallet_seeds: &[&[&[u8]]] = &[&[
        seeds::SMART_WALLET,
        owner_key.as_ref(),
        &[wallet.bump],
    ]];

    let sol_before = ctx.accounts.sol_treasury.amount;
    let usdc_before = ctx.accounts.usdc_treasury.amount;

    let cpi_accounts = cpi::accounts::DecreaseLiquidityV2 {
        nft_owner: ctx.accounts.wallet.to_account_info(),
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
        ctx.accounts.clmm_program.to_account_info(),
        cpi_accounts,
        wallet_seeds,
    );

    cpi::decrease_liquidity_v2(cpi_ctx, liquidity, amount_0_min, amount_1_min)?;

    // Reload treasuries
    ctx.accounts.sol_treasury.reload()?;
    ctx.accounts.usdc_treasury.reload()?;

    let sol_received = ctx.accounts.sol_treasury.amount.saturating_sub(sol_before);
    let usdc_received = ctx.accounts.usdc_treasury.amount.saturating_sub(usdc_before);

    // Update wallet state
    let wallet = &mut ctx.accounts.wallet;
    wallet.position_liquidity = wallet.position_liquidity.saturating_sub(liquidity);
    wallet.position_sol = wallet.position_sol.saturating_sub(sol_received);
    wallet.position_usdc = wallet.position_usdc.saturating_sub(usdc_received);
    wallet.updated_at = Clock::get()?.unix_timestamp;

    emit!(WalletLiquidityDecreased {
        wallet: wallet.key(),
        sol_received,
        usdc_received,
        remaining_liquidity: wallet.position_liquidity,
    });

    Ok(())
}
