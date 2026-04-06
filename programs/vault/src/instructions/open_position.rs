use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::Token;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_clmm_cpi::{
    cpi,
    states::{PersonalPositionState, PoolState},
};

use crate::errors::VaultError;
use crate::events::PositionOpened;
use crate::state::{seeds, Vault};

#[derive(Accounts)]
#[instruction(tick_lower_index: i32, tick_upper_index: i32, tick_array_lower_start_index: i32, tick_array_upper_start_index: i32)]
pub struct OpenPosition<'info> {
    /// Admin opening the position
    #[account(mut)]
    pub admin: Signer<'info>,

    /// Vault state
    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
        constraint = !vault.has_active_position @ VaultError::PositionAlreadyExists,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// SOL treasury PDA (source for token0)
    #[account(
        mut,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    /// USDC treasury PDA (source for token1)
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

    /// Position NFT mint (will be created)
    #[account(mut)]
    pub position_nft_mint: Signer<'info>,

    /// Position NFT account (vault will own the NFT)
    /// CHECK: Will be initialized by Raydium
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    /// Personal position state (created by Raydium)
    /// CHECK: Will be initialized by Raydium
    #[account(mut)]
    pub personal_position: UncheckedAccount<'info>,

    /// Tick array for lower bound
    /// CHECK: Validated by Raydium
    #[account(mut)]
    pub tick_array_lower: UncheckedAccount<'info>,

    /// Tick array for upper bound
    /// CHECK: Validated by Raydium
    #[account(mut)]
    pub tick_array_upper: UncheckedAccount<'info>,

    /// Token vault 0 (pool's SOL vault)
    #[account(mut)]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Token vault 1 (pool's USDC vault)
    #[account(mut)]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Mint of vault 0
    pub vault_0_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Mint of vault 1
    pub vault_1_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Tick array bitmap extension
    /// CHECK: Validated by Raydium
    pub tick_array_bitmap: UncheckedAccount<'info>,

    /// Raydium CLMM program
    /// CHECK: Validated by address constraint
    #[account(address = raydium_clmm_cpi::id())]
    pub clmm_program: UncheckedAccount<'info>,

    pub rent: Sysvar<'info, Rent>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

pub fn handler<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, OpenPosition<'info>>,
    tick_lower_index: i32,
    tick_upper_index: i32,
    tick_array_lower_start_index: i32,
    tick_array_upper_start_index: i32,
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
) -> Result<()> {
    require!(tick_lower_index < tick_upper_index, VaultError::InvalidTickRange);
    require!(liquidity > 0 || amount_0_max > 0, VaultError::InvalidAmount);

    let vault = &ctx.accounts.vault;

    // Check treasury has enough funds
    require!(
        ctx.accounts.sol_treasury.amount >= amount_0_max,
        VaultError::InsufficientTreasuryBalance
    );
    require!(
        ctx.accounts.usdc_treasury.amount >= amount_1_max,
        VaultError::InsufficientTreasuryBalance
    );

    // Build signer seeds for vault PDA
    let vault_seeds: &[&[&[u8]]] = &[&[
        seeds::VAULT,
        &[vault.bump],
    ]];

    // H-04: Save balances before CPI for accurate position tracking
    let sol_before = ctx.accounts.sol_treasury.amount;
    let usdc_before = ctx.accounts.usdc_treasury.amount;

    // Approve admin as delegate on treasury accounts so Raydium can use admin (payer) as authority
    let sol_treasury_seeds: &[&[u8]] = &[
        seeds::SOL_TREASURY,
        &ctx.accounts.vault.key().to_bytes(),
        &[vault.sol_treasury_bump],
    ];
    anchor_spl::token_interface::approve(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token_interface::Approve {
                to: ctx.accounts.sol_treasury.to_account_info(),
                delegate: ctx.accounts.admin.to_account_info(),
                authority: ctx.accounts.sol_treasury.to_account_info(),
            },
            &[sol_treasury_seeds],
        ),
        amount_0_max,
    )?;

    let usdc_treasury_seeds: &[&[u8]] = &[
        seeds::USDC_TREASURY,
        &ctx.accounts.vault.key().to_bytes(),
        &[vault.usdc_treasury_bump],
    ];
    anchor_spl::token_interface::approve(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token_interface::Approve {
                to: ctx.accounts.usdc_treasury.to_account_info(),
                delegate: ctx.accounts.admin.to_account_info(),
                authority: ctx.accounts.usdc_treasury.to_account_info(),
            },
            &[usdc_treasury_seeds],
        ),
        amount_1_max,
    )?;

    // Build CPI context for opening position
    // Admin is payer (for rent), vault owns the NFT, admin is delegated authority on treasuries
    let cpi_accounts = cpi::accounts::OpenPositionWithToken22Nft {
        payer: ctx.accounts.admin.to_account_info(),
        position_nft_owner: ctx.accounts.vault.to_account_info(), // Vault owns the NFT
        position_nft_mint: ctx.accounts.position_nft_mint.to_account_info(),
        position_nft_account: ctx.accounts.position_nft_account.to_account_info(),
        pool_state: ctx.accounts.pool_state.to_account_info(),
        protocol_position: ctx.accounts.personal_position.to_account_info(),
        tick_array_lower: ctx.accounts.tick_array_lower.to_account_info(),
        tick_array_upper: ctx.accounts.tick_array_upper.to_account_info(),
        personal_position: ctx.accounts.personal_position.to_account_info(),
        token_account_0: ctx.accounts.sol_treasury.to_account_info(),
        token_account_1: ctx.accounts.usdc_treasury.to_account_info(),
        token_vault_0: ctx.accounts.token_vault_0.to_account_info(),
        token_vault_1: ctx.accounts.token_vault_1.to_account_info(),
        rent: ctx.accounts.rent.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        token_program: ctx.accounts.token_program.to_account_info(),
        associated_token_program: ctx.accounts.associated_token_program.to_account_info(),
        token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
        vault_0_mint: ctx.accounts.vault_0_mint.to_account_info(),
        vault_1_mint: ctx.accounts.vault_1_mint.to_account_info(),
    };

    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.clmm_program.to_account_info(),
        cpi_accounts,
        vault_seeds,
    );

    // Add remaining accounts (tick array bitmap)
    let cpi_ctx = cpi_ctx.with_remaining_accounts(vec![
        ctx.accounts.tick_array_bitmap.to_account_info(),
    ]);

    // Execute CPI to open position
    cpi::open_position_with_token22_nft(
        cpi_ctx,
        tick_lower_index,
        tick_upper_index,
        tick_array_lower_start_index,
        tick_array_upper_start_index,
        liquidity,
        amount_0_max,
        amount_1_max,
        true, // with_metadata
        Some(false), // base_flag: calculate from amount_1 (USDC)
    )?;

    // H-05: Revoke delegations after CPI
    anchor_spl::token_interface::revoke(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token_interface::Revoke {
                source: ctx.accounts.sol_treasury.to_account_info(),
                authority: ctx.accounts.sol_treasury.to_account_info(),
            },
            &[sol_treasury_seeds],
        ),
    )?;
    anchor_spl::token_interface::revoke(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token_interface::Revoke {
                source: ctx.accounts.usdc_treasury.to_account_info(),
                authority: ctx.accounts.usdc_treasury.to_account_info(),
            },
            &[usdc_treasury_seeds],
        ),
    )?;

    // Reload treasuries to get updated balances
    ctx.accounts.sol_treasury.reload()?;
    ctx.accounts.usdc_treasury.reload()?;

    // H-04: Calculate actual amounts used (before - after)
    let sol_used = sol_before.saturating_sub(ctx.accounts.sol_treasury.amount);
    let usdc_used = usdc_before.saturating_sub(ctx.accounts.usdc_treasury.amount);

    // Update vault state
    let vault = &mut ctx.accounts.vault;
    vault.has_active_position = true;
    vault.position_mint = ctx.accounts.position_nft_mint.key();
    vault.position_pool_id = ctx.accounts.pool_state.key();
    vault.position_tick_lower = tick_lower_index;
    vault.position_tick_upper = tick_upper_index;
    // M-07: Read actual liquidity from personal_position after CPI (not the requested value)
    let position_data = ctx.accounts.personal_position.try_borrow_data()?;
    let personal_pos = PersonalPositionState::try_deserialize(&mut &position_data[..])?;
    vault.position_liquidity = personal_pos.liquidity;
    vault.position_sol = sol_used;
    vault.position_usdc = usdc_used;
    vault.treasury_sol = ctx.accounts.sol_treasury.amount;
    vault.treasury_usdc = ctx.accounts.usdc_treasury.amount;
    // ARCH-001: rebalance complete, unblock user deposits/withdrawals
    vault.is_rebalancing = false;

    emit!(PositionOpened {
        position_mint: ctx.accounts.position_nft_mint.key(),
        pool_id: vault.position_pool_id,
        tick_lower: tick_lower_index,
        tick_upper: tick_upper_index,
        liquidity: vault.position_liquidity,
        sol_used: vault.position_sol,
        usdc_used: vault.position_usdc,
    });

    Ok(())
}
