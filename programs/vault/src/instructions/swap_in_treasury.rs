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
    /// Swap token0 → token1
    Token0ToToken1,
    /// Swap token1 → token0
    Token1ToToken0,
}

#[derive(Accounts)]
pub struct SwapInTreasury<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
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
    require!(amount_in > 0, VaultError::InvalidAmount);
    require!(minimum_amount_out > 0, VaultError::InvalidAmount);

    let vault = &ctx.accounts.vault;

    // Swapping with an active position would leave vault.position_token0/token1
    // stale relative to the post-swap treasury, breaking withdrawal entitlements.
    // Close the position first, then swap, then open a new position.
    require!(!vault.has_active_position, VaultError::PositionAlreadyExists);

    let (input_treasury, output_treasury) = match direction {
        SwapDirection::Token0ToToken1 => {
            require!(
                ctx.accounts.token0_treasury.amount >= amount_in,
                VaultError::InsufficientTreasuryBalance
            );
            // Validate that the caller passed Raydium vaults matching our token ordering.
            // pool.token_vault_N may be vault.token0 or vault.token1 depending on the pool —
            // this check ensures the script isn't hard-coded for a specific pool layout.
            require!(
                ctx.accounts.input_vault.mint == vault.token0_mint,
                VaultError::InvalidMint
            );
            require!(
                ctx.accounts.output_vault.mint == vault.token1_mint,
                VaultError::InvalidMint
            );
            (&ctx.accounts.token0_treasury, &ctx.accounts.token1_treasury)
        }
        SwapDirection::Token1ToToken0 => {
            require!(
                ctx.accounts.token1_treasury.amount >= amount_in,
                VaultError::InsufficientTreasuryBalance
            );
            require!(
                ctx.accounts.input_vault.mint == vault.token1_mint,
                VaultError::InvalidMint
            );
            require!(
                ctx.accounts.output_vault.mint == vault.token0_mint,
                VaultError::InvalidMint
            );
            (&ctx.accounts.token1_treasury, &ctx.accounts.token0_treasury)
        }
    };

    let vault_key = vault.key();
    let (treasury_seed, treasury_bump): (&[u8], u8) = match direction {
        SwapDirection::Token0ToToken1 => (seeds::TOKEN0_TREASURY, vault.token0_treasury_bump),
        SwapDirection::Token1ToToken0 => (seeds::TOKEN1_TREASURY, vault.token1_treasury_bump),
    };

    let signer_seeds: &[&[&[u8]]] = &[&[
        treasury_seed,
        vault_key.as_ref(),
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
    )
    .with_remaining_accounts(ctx.remaining_accounts.to_vec());

    cpi::swap_v2(cpi_ctx, amount_in, minimum_amount_out, 0, true)?;

    ctx.accounts.token0_treasury.reload()?;
    ctx.accounts.token1_treasury.reload()?;

    let vault = &mut ctx.accounts.vault.as_mut();
    vault.treasury_token0 = ctx.accounts.token0_treasury.amount;
    vault.treasury_token1 = ctx.accounts.token1_treasury.amount;

    emit!(SwapEvent {
        amount_in,
        direction: if direction == SwapDirection::Token0ToToken1 {
            "TOKEN0->TOKEN1".to_string()
        } else {
            "TOKEN1->TOKEN0".to_string()
        },
        treasury_token0: vault.treasury_token0,
        treasury_token1: vault.treasury_token1,
    });

    Ok(())
}
