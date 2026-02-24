use anchor_lang::prelude::*;
use anchor_spl::token::Token;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_clmm_cpi::{
    cpi,
    states::{PoolState, PersonalPositionState, TickArrayState},
};

use crate::errors::WalletError;
use crate::events::WalletLiquidityIncreased;
use crate::state::{seeds, SmartWallet};

#[derive(Accounts)]
pub struct IncreaseLiquidity<'info> {
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

    /// SOL treasury PDA (source for token0)
    #[account(
        mut,
        seeds = [seeds::WALLET_SOL_TREASURY, wallet.key().as_ref()],
        bump = wallet.sol_treasury_bump,
    )]
    pub sol_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    /// USDC treasury PDA (source for token1)
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
}

pub fn handler(
    ctx: Context<IncreaseLiquidity>,
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
) -> Result<()> {
    require!(liquidity > 0 || amount_0_max > 0, WalletError::InvalidAmount);
    require!(!ctx.accounts.wallet.is_paused, WalletError::WalletPaused);

    let wallet = &ctx.accounts.wallet;

    require!(
        ctx.accounts.sol_treasury.amount >= amount_0_max,
        WalletError::InsufficientBalance
    );
    require!(
        ctx.accounts.usdc_treasury.amount >= amount_1_max,
        WalletError::InsufficientBalance
    );

    // Save balances before CPI
    let sol_before = ctx.accounts.sol_treasury.amount;
    let usdc_before = ctx.accounts.usdc_treasury.amount;

    // Build signer seeds
    let owner_key = wallet.owner;
    let wallet_key = wallet.key();
    let wallet_seeds: &[&[&[u8]]] = &[&[
        seeds::SMART_WALLET,
        owner_key.as_ref(),
        &[wallet.bump],
    ]];
    let sol_treasury_seeds: &[&[u8]] = &[
        seeds::WALLET_SOL_TREASURY,
        wallet_key.as_ref(),
        &[wallet.sol_treasury_bump],
    ];
    let usdc_treasury_seeds: &[&[u8]] = &[
        seeds::WALLET_USDC_TREASURY,
        wallet_key.as_ref(),
        &[wallet.usdc_treasury_bump],
    ];

    // Approve wallet PDA as delegate on treasury accounts so Raydium can use it as authority
    anchor_spl::token_interface::approve(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token_interface::Approve {
                to: ctx.accounts.sol_treasury.to_account_info(),
                delegate: ctx.accounts.wallet.to_account_info(),
                authority: ctx.accounts.sol_treasury.to_account_info(),
            },
            &[sol_treasury_seeds],
        ),
        amount_0_max,
    )?;
    anchor_spl::token_interface::approve(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token_interface::Approve {
                to: ctx.accounts.usdc_treasury.to_account_info(),
                delegate: ctx.accounts.wallet.to_account_info(),
                authority: ctx.accounts.usdc_treasury.to_account_info(),
            },
            &[usdc_treasury_seeds],
        ),
        amount_1_max,
    )?;

    let cpi_accounts = cpi::accounts::IncreaseLiquidityV2 {
        nft_owner: ctx.accounts.wallet.to_account_info(),
        nft_account: ctx.accounts.position_nft_account.to_account_info(),
        pool_state: ctx.accounts.pool_state.to_account_info(),
        protocol_position: ctx.accounts.personal_position.to_account_info(),
        personal_position: ctx.accounts.personal_position.to_account_info(),
        tick_array_lower: ctx.accounts.tick_array_lower.to_account_info(),
        tick_array_upper: ctx.accounts.tick_array_upper.to_account_info(),
        token_account_0: ctx.accounts.sol_treasury.to_account_info(),
        token_account_1: ctx.accounts.usdc_treasury.to_account_info(),
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
        wallet_seeds,
    );

    cpi::increase_liquidity_v2(cpi_ctx, liquidity, amount_0_max, amount_1_max, Some(true))?;

    // Revoke delegations after CPI
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

    // Reload treasuries
    ctx.accounts.sol_treasury.reload()?;
    ctx.accounts.usdc_treasury.reload()?;

    let sol_used = sol_before.saturating_sub(ctx.accounts.sol_treasury.amount);
    let usdc_used = usdc_before.saturating_sub(ctx.accounts.usdc_treasury.amount);

    // Read actual liquidity from personal_position after CPI
    ctx.accounts.personal_position.reload()?;
    let actual_liquidity = ctx.accounts.personal_position.liquidity;

    // Update wallet state
    let wallet = &mut ctx.accounts.wallet;
    wallet.position_liquidity = actual_liquidity;
    wallet.position_sol = wallet.position_sol.saturating_add(sol_used);
    wallet.position_usdc = wallet.position_usdc.saturating_add(usdc_used);
    wallet.updated_at = Clock::get()?.unix_timestamp;

    emit!(WalletLiquidityIncreased {
        wallet: wallet.key(),
        sol_added: sol_used,
        usdc_added: usdc_used,
        new_liquidity: wallet.position_liquidity,
    });

    Ok(())
}
