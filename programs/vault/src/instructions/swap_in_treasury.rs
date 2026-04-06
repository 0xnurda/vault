use anchor_lang::prelude::*;
use anchor_spl::memo::Memo;
use anchor_spl::token::Token;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_clmm_cpi::{
    cpi,
    states::{AmmConfig, ObservationState, PoolState},
};

use crate::errors::VaultError;
use crate::events::SwapEvent;
use crate::state::{seeds, Vault};

/// Swap direction enum
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum SwapDirection {
    /// Swap SOL → USDC
    SolToUsdc,
    /// Swap USDC → SOL
    UsdcToSol,
}

#[derive(Accounts)]
pub struct SwapInTreasury<'info> {
    /// Admin performing the swap
    #[account(mut)]
    pub admin: Signer<'info>,

    /// Vault state
    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// SOL treasury PDA (wSOL)
    #[account(
        mut,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    /// USDC treasury PDA
    #[account(
        mut,
        seeds = [seeds::USDC_TREASURY, vault.key().as_ref()],
        bump = vault.usdc_treasury_bump,
    )]
    pub usdc_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    // ============ Raydium CLMM accounts ============

    /// AMM config account
    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Pool state account
    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Input vault (Raydium pool vault for input token)
    #[account(mut)]
    pub input_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Output vault (Raydium pool vault for output token)
    #[account(mut)]
    pub output_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Observation state for price oracle
    #[account(mut)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    /// Input vault mint
    pub input_vault_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Output vault mint
    pub output_vault_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Raydium CLMM program
    /// CHECK: Validated by address constraint
    #[account(address = raydium_clmm_cpi::id())]
    pub clmm_program: UncheckedAccount<'info>,

    /// Token program
    pub token_program: Program<'info, Token>,

    /// Token 2022 program
    pub token_program_2022: Program<'info, Token2022>,

    /// Memo program
    pub memo_program: Program<'info, Memo>,
}

pub fn handler<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, SwapInTreasury<'info>>,
    amount_in: u64,
    minimum_amount_out: u64,
    direction: SwapDirection,
) -> Result<()> {
    require!(amount_in > 0, VaultError::InvalidAmount);
    require!(minimum_amount_out > 0, VaultError::InvalidAmount); // Prevent 0-slippage swaps

    let vault = &ctx.accounts.vault;

    // Determine input/output treasuries based on direction
    let (input_treasury, output_treasury, is_base_input) = match direction {
        SwapDirection::SolToUsdc => {
            require!(
                ctx.accounts.sol_treasury.amount >= amount_in,
                VaultError::InsufficientTreasuryBalance
            );
            (&ctx.accounts.sol_treasury, &ctx.accounts.usdc_treasury, true)
        }
        SwapDirection::UsdcToSol => {
            require!(
                ctx.accounts.usdc_treasury.amount >= amount_in,
                VaultError::InsufficientTreasuryBalance
            );
            (&ctx.accounts.usdc_treasury, &ctx.accounts.sol_treasury, false)
        }
    };

    // Build signer seeds for treasury PDA
    let vault_key = vault.key();
    let (treasury_seeds, treasury_bump): (&[u8], u8) = match direction {
        SwapDirection::SolToUsdc => (seeds::SOL_TREASURY, vault.sol_treasury_bump),
        SwapDirection::UsdcToSol => (seeds::USDC_TREASURY, vault.usdc_treasury_bump),
    };

    let signer_seeds: &[&[&[u8]]] = &[&[
        treasury_seeds,
        vault_key.as_ref(),
        &[treasury_bump],
    ]];

    // Build CPI context for Raydium swap
    let cpi_accounts = cpi::accounts::SwapSingleV2 {
        payer: input_treasury.to_account_info(),
        amm_config: ctx.accounts.amm_config.to_account_info(),
        pool_state: ctx.accounts.pool_state.to_account_info(),
        input_token_account: input_treasury.to_account_info(),
        output_token_account: output_treasury.to_account_info(),
        input_vault: ctx.accounts.input_vault.to_account_info(),
        output_vault: ctx.accounts.output_vault.to_account_info(),
        observation_state: ctx.accounts.observation_state.to_account_info(),
        token_program: ctx.accounts.token_program.to_account_info(),
        token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
        memo_program: ctx.accounts.memo_program.to_account_info(),
        input_vault_mint: ctx.accounts.input_vault_mint.to_account_info(),
        output_vault_mint: ctx.accounts.output_vault_mint.to_account_info(),
    };

    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.clmm_program.to_account_info(),
        cpi_accounts,
        signer_seeds,
    );

    // Add remaining accounts (tick arrays)
    let cpi_ctx = cpi_ctx.with_remaining_accounts(ctx.remaining_accounts.to_vec());

    // Execute swap via CPI
    // sqrt_price_limit_x64 = 0 means no price limit
    cpi::swap_v2(
        cpi_ctx,
        amount_in,
        minimum_amount_out,
        0, // sqrt_price_limit_x64 - no limit
        is_base_input,
    )?;

    // Reload accounts to get updated balances
    ctx.accounts.sol_treasury.reload()?;
    ctx.accounts.usdc_treasury.reload()?;

    // Update vault state with new treasury balances
    let vault = &mut ctx.accounts.vault.as_mut();
    vault.treasury_sol = ctx.accounts.sol_treasury.amount;
    vault.treasury_usdc = ctx.accounts.usdc_treasury.amount;

    emit!(SwapEvent {
        amount_in,
        direction: if direction == SwapDirection::SolToUsdc { "SOL->USDC".to_string() } else { "USDC->SOL".to_string() },
        treasury_sol: vault.treasury_sol,
        treasury_usdc: vault.treasury_usdc,
    });

    Ok(())
}
