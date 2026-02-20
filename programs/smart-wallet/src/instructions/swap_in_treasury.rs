use anchor_lang::prelude::*;
use anchor_spl::memo::Memo;
use anchor_spl::token::Token;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_clmm_cpi::{
    cpi,
    states::{AmmConfig, ObservationState, PoolState},
};

use crate::errors::WalletError;
use crate::events::WalletSwapEvent;
use crate::state::{seeds, SmartWallet};

/// Swap direction enum
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum SwapDirection {
    SolToUsdc,
    UsdcToSol,
}

#[derive(Accounts)]
pub struct SwapInTreasury<'info> {
    /// Operator (delegate or owner)
    #[account(mut)]
    pub operator: Signer<'info>,

    /// Smart wallet state
    #[account(
        mut,
        constraint = wallet.owner == operator.key() || wallet.delegate == operator.key() @ WalletError::Unauthorized,
    )]
    pub wallet: Box<Account<'info, SmartWallet>>,

    /// SOL treasury PDA (wSOL)
    #[account(
        mut,
        seeds = [seeds::WALLET_SOL_TREASURY, wallet.key().as_ref()],
        bump = wallet.sol_treasury_bump,
    )]
    pub sol_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    /// USDC treasury PDA
    #[account(
        mut,
        seeds = [seeds::WALLET_USDC_TREASURY, wallet.key().as_ref()],
        bump = wallet.usdc_treasury_bump,
    )]
    pub usdc_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    // ============ Raydium CLMM accounts ============

    pub amm_config: Box<Account<'info, AmmConfig>>,

    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    #[account(mut)]
    pub input_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub output_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub observation_state: AccountLoader<'info, ObservationState>,

    pub input_vault_mint: Box<InterfaceAccount<'info, Mint>>,
    pub output_vault_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: Validated by address constraint
    #[account(address = raydium_clmm_cpi::id())]
    pub clmm_program: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub memo_program: Program<'info, Memo>,
}

pub fn handler<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, SwapInTreasury<'info>>,
    amount_in: u64,
    minimum_amount_out: u64,
    direction: SwapDirection,
) -> Result<()> {
    require!(amount_in > 0, WalletError::InvalidAmount);
    require!(!ctx.accounts.wallet.is_paused, WalletError::WalletPaused);

    let wallet = &ctx.accounts.wallet;

    let (input_treasury, output_treasury, is_base_input) = match direction {
        SwapDirection::SolToUsdc => {
            require!(
                ctx.accounts.sol_treasury.amount >= amount_in,
                WalletError::InsufficientBalance
            );
            (&ctx.accounts.sol_treasury, &ctx.accounts.usdc_treasury, true)
        }
        SwapDirection::UsdcToSol => {
            require!(
                ctx.accounts.usdc_treasury.amount >= amount_in,
                WalletError::InsufficientBalance
            );
            (&ctx.accounts.usdc_treasury, &ctx.accounts.sol_treasury, false)
        }
    };

    // Build signer seeds for treasury PDA (self-authority)
    let wallet_key = wallet.key();
    let (treasury_seed, treasury_bump): (&[u8], u8) = match direction {
        SwapDirection::SolToUsdc => (seeds::WALLET_SOL_TREASURY, wallet.sol_treasury_bump),
        SwapDirection::UsdcToSol => (seeds::WALLET_USDC_TREASURY, wallet.usdc_treasury_bump),
    };

    let signer_seeds: &[&[&[u8]]] = &[&[
        treasury_seed,
        wallet_key.as_ref(),
        &[treasury_bump],
    ]];

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

    let cpi_ctx = cpi_ctx.with_remaining_accounts(ctx.remaining_accounts.to_vec());

    cpi::swap_v2(cpi_ctx, amount_in, minimum_amount_out, 0, is_base_input)?;

    ctx.accounts.sol_treasury.reload()?;
    ctx.accounts.usdc_treasury.reload()?;

    let wallet = &mut ctx.accounts.wallet;
    wallet.updated_at = Clock::get()?.unix_timestamp;

    emit!(WalletSwapEvent {
        wallet: wallet.key(),
        amount_in,
        direction: if direction == SwapDirection::SolToUsdc { "SOL->USDC".to_string() } else { "USDC->SOL".to_string() },
    });

    Ok(())
}
