use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_clmm_cpi::{
    cpi,
    states::{PoolState, PersonalPositionState, TickArrayState},
};

use crate::errors::VaultError;
use crate::events::LiquidityIncreased;
use crate::state::{seeds, Vault};

#[derive(Accounts)]
pub struct IncreaseLiquidity<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
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
}

const MAX_SLIPPAGE_BPS: u16 = 500;

pub fn handler(
    ctx: Context<IncreaseLiquidity>,
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
    slippage_bps: u16,
) -> Result<()> {
    require!(liquidity > 0 || amount_0_max > 0, VaultError::InvalidAmount);
    require!(slippage_bps <= MAX_SLIPPAGE_BPS, VaultError::SlippageTooHigh);

    let vault = &ctx.accounts.vault;

    require!(
        ctx.accounts.token0_treasury.amount >= amount_0_max,
        VaultError::InsufficientTreasuryBalance
    );
    require!(
        ctx.accounts.token1_treasury.amount >= amount_1_max,
        VaultError::InsufficientTreasuryBalance
    );

    let token0_before = ctx.accounts.token0_treasury.amount;
    let token1_before = ctx.accounts.token1_treasury.amount;

    let token0_treasury_seeds: &[&[u8]] = &[
        seeds::TOKEN0_TREASURY,
        &ctx.accounts.vault.key().to_bytes(),
        &[vault.token0_treasury_bump],
    ];
    let token1_treasury_seeds: &[&[u8]] = &[
        seeds::TOKEN1_TREASURY,
        &ctx.accounts.vault.key().to_bytes(),
        &[vault.token1_treasury_bump],
    ];

    if amount_0_max > 0 {
        anchor_spl::token_interface::approve(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token_interface::Approve {
                    to:        ctx.accounts.token0_treasury.to_account_info(),
                    delegate:  ctx.accounts.vault.to_account_info(),
                    authority: ctx.accounts.token0_treasury.to_account_info(),
                },
                &[token0_treasury_seeds],
            ),
            amount_0_max,
        )?;
    }

    if amount_1_max > 0 {
        anchor_spl::token_interface::approve(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token_interface::Approve {
                    to:        ctx.accounts.token1_treasury.to_account_info(),
                    delegate:  ctx.accounts.vault.to_account_info(),
                    authority: ctx.accounts.token1_treasury.to_account_info(),
                },
                &[token1_treasury_seeds],
            ),
            amount_1_max,
        )?;
    }

    let pool_id = vault.pool_id;
    let vault_seeds: &[&[&[u8]]] = &[&[seeds::VAULT, pool_id.as_ref(), &[vault.bump]]];

    let cpi_accounts = cpi::accounts::IncreaseLiquidityV2 {
        nft_owner: ctx.accounts.vault.to_account_info(),
        nft_account: ctx.accounts.position_nft_account.to_account_info(),
        pool_state: ctx.accounts.pool_state.to_account_info(),
        protocol_position: ctx.accounts.personal_position.to_account_info(),
        personal_position: ctx.accounts.personal_position.to_account_info(),
        tick_array_lower: ctx.accounts.tick_array_lower.to_account_info(),
        tick_array_upper: ctx.accounts.tick_array_upper.to_account_info(),
        token_account_0: ctx.accounts.token0_treasury.to_account_info(),
        token_account_1: ctx.accounts.token1_treasury.to_account_info(),
        token_vault_0: ctx.accounts.token_vault_0.to_account_info(),
        token_vault_1: ctx.accounts.token_vault_1.to_account_info(),
        token_program: ctx.accounts.token_program.to_account_info(),
        token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
        vault_0_mint: ctx.accounts.vault_0_mint.to_account_info(),
        vault_1_mint: ctx.accounts.vault_1_mint.to_account_info(),
    };

    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.clmm_program.to_account_info(),
        cpi_accounts,
        vault_seeds,
    );

    // Save result WITHOUT `?` — must ALWAYS revoke regardless of CPI outcome.
    let increase_result = cpi::increase_liquidity_v2(cpi_ctx, liquidity, amount_0_max, amount_1_max, Some(true));

    // ALWAYS revoke both delegations, even if CPI failed.
    if amount_0_max > 0 {
        anchor_spl::token_interface::revoke(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token_interface::Revoke {
                    source:    ctx.accounts.token0_treasury.to_account_info(),
                    authority: ctx.accounts.token0_treasury.to_account_info(),
                },
                &[token0_treasury_seeds],
            ),
        )?;
    }

    if amount_1_max > 0 {
        anchor_spl::token_interface::revoke(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                anchor_spl::token_interface::Revoke {
                    source:    ctx.accounts.token1_treasury.to_account_info(),
                    authority: ctx.accounts.token1_treasury.to_account_info(),
                },
                &[token1_treasury_seeds],
            ),
        )?;
    }

    // Now propagate the CPI result.
    increase_result?;

    ctx.accounts.token0_treasury.reload()?;
    ctx.accounts.token1_treasury.reload()?;

    let token0_used = token0_before.saturating_sub(ctx.accounts.token0_treasury.amount);
    let token1_used = token1_before.saturating_sub(ctx.accounts.token1_treasury.amount);

    ctx.accounts.personal_position.reload()?;
    let actual_liquidity = ctx.accounts.personal_position.liquidity;

    let vault = &mut ctx.accounts.vault;
    vault.position_liquidity = actual_liquidity;
    vault.position_token0 = vault.position_token0.saturating_add(token0_used);
    vault.position_token1 = vault.position_token1.saturating_add(token1_used);
    vault.treasury_token0 = ctx.accounts.token0_treasury.amount;
    vault.treasury_token1 = ctx.accounts.token1_treasury.amount;

    emit!(LiquidityIncreased {
        token0_added: token0_used,
        token1_added: token1_used,
        new_liquidity: vault.position_liquidity,
    });

    Ok(())
}
