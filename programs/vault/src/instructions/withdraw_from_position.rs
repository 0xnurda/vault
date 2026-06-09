use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, Token, TokenAccount, Transfer};
use anchor_spl::token_2022::Token2022;
use anchor_spl::memo::Memo;
use anchor_spl::token_interface::{
    Mint as InterfaceMint, TokenAccount as InterfaceTokenAccount,
};
use raydium_clmm_cpi::{
    cpi,
    states::{PoolState, PersonalPositionState, TickArrayState},
};

use crate::errors::VaultError;
use crate::events::WithdrawEvent;
use crate::state::{seeds, UserDeposit, Vault};

#[derive(Accounts)]
pub struct WithdrawFromPosition<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
        constraint = vault.has_active_position @ VaultError::NoActivePosition,
    )]
    pub vault: Box<Account<'info, Vault>>,

    #[account(
        mut,
        seeds = [seeds::USER_DEPOSIT, vault.key().as_ref(), user.key().as_ref()],
        bump = user_deposit.bump,
        constraint = user_deposit.user == user.key(),
    )]
    pub user_deposit: Box<Account<'info, UserDeposit>>,

    #[account(
        mut,
        seeds = [seeds::SHARE_MINT, vault.key().as_ref()],
        bump = vault.share_mint_bump,
    )]
    pub share_mint: Box<Account<'info, Mint>>,

    #[account(
        mut,
        constraint = user_share_account.owner == user.key(),
        constraint = user_share_account.mint == share_mint.key(),
    )]
    pub user_share_account: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [seeds::TOKEN0_TREASURY, vault.key().as_ref()],
        bump = vault.token0_treasury_bump,
    )]
    pub token0_treasury: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(
        mut,
        seeds = [seeds::TOKEN1_TREASURY, vault.key().as_ref()],
        bump = vault.token1_treasury_bump,
    )]
    pub token1_treasury: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(
        mut,
        constraint = user_token0_account.owner == user.key(),
        constraint = user_token0_account.mint == token0_treasury.mint @ VaultError::InvalidMint,
    )]
    pub user_token0_account: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = user_token1_account.owner == user.key(),
        constraint = user_token1_account.mint == token1_treasury.mint @ VaultError::InvalidMint,
    )]
    pub user_token1_account: Box<Account<'info, TokenAccount>>,

    #[account(mut)]
    pub pool_state: AccountLoader<'info, PoolState>,

    #[account(
        constraint = position_nft_account.amount == 1,
        constraint = position_nft_account.mint == vault.position_mint @ VaultError::InvalidPosition,
        constraint = position_nft_account.owner == vault.key() @ VaultError::InvalidPosition,
    )]
    pub position_nft_account: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(
        mut,
        constraint = personal_position.pool_id == pool_state.key(),
    )]
    pub personal_position: Box<Account<'info, PersonalPositionState>>,

    #[account(mut)]
    pub token_vault_0: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(mut)]
    pub token_vault_1: Box<InterfaceAccount<'info, InterfaceTokenAccount>>,

    #[account(mut)]
    pub tick_array_lower: AccountLoader<'info, TickArrayState>,

    #[account(mut)]
    pub tick_array_upper: AccountLoader<'info, TickArrayState>,

    pub vault_0_mint: Box<InterfaceAccount<'info, InterfaceMint>>,
    pub vault_1_mint: Box<InterfaceAccount<'info, InterfaceMint>>,

    /// CHECK: Validated by address constraint
    #[account(address = raydium_clmm_cpi::id())]
    pub clmm_program: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub memo_program: Program<'info, Memo>,
}

pub fn handler<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, WithdrawFromPosition<'info>>,
    min_token0_out: u64,
    min_token1_out: u64,
) -> Result<()> {
    let remaining = ctx.remaining_accounts.to_vec();

    require!(!ctx.accounts.vault.is_paused, VaultError::VaultPaused);

    // Use actual share token balance (audit finding #2 fix).
    let shares_amount = ctx.accounts.user_share_account.amount;
    require!(shares_amount > 0, VaultError::InsufficientShares);

    let total_shares = ctx.accounts.vault.total_shares;
    require!(total_shares > 0, VaultError::InsufficientShares);

    let vault_bump = ctx.accounts.vault.bump;
    let token0_treasury_bump = ctx.accounts.vault.token0_treasury_bump;
    let token1_treasury_bump = ctx.accounts.vault.token1_treasury_bump;
    let old_accumulated_fees_token0 = ctx.accounts.vault.accumulated_protocol_fees_token0;
    let old_accumulated_fees_token1 = ctx.accounts.vault.accumulated_protocol_fees_token1;
    let position_liquidity = ctx.accounts.vault.position_liquidity;
    let vault_key = ctx.accounts.vault.key();
    let pool_id = ctx.accounts.vault.pool_id;

    let vault_seeds: &[&[&[u8]]] = &[&[seeds::VAULT, pool_id.as_ref(), &[vault_bump]]];

    // Lean withdraw (Kamino-style): no inline fee collection.
    // Trading fees are harvested separately by the keeper via `collect_fees`
    // (10% protocol cut taken there). Here the user simply receives:
    //   - pro-rata of treasury (minus already-harvested protocol fees), plus
    //   - pro-rata of their position principal (one DecreaseLiquidityV2 CPI).
    // This keeps the instruction to a single CPI struct → fits the SBF stack.
    let user_accessible_token0 = ctx.accounts.token0_treasury.amount
        .saturating_sub(old_accumulated_fees_token0);
    let user_accessible_token1 = ctx.accounts.token1_treasury.amount
        .saturating_sub(old_accumulated_fees_token1);

    let user_treasury_token0 = (user_accessible_token0 as u128)
        .checked_mul(shares_amount as u128)
        .and_then(|v| v.checked_div(total_shares as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or(error!(VaultError::MathOverflow))?;

    let user_treasury_token1 = (user_accessible_token1 as u128)
        .checked_mul(shares_amount as u128)
        .and_then(|v| v.checked_div(total_shares as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or(error!(VaultError::MathOverflow))?;

    let user_liquidity: u128 = (position_liquidity as u128)
        .checked_mul(shares_amount as u128)
        .and_then(|v| v.checked_div(total_shares as u128))
        .unwrap_or(0);

    // CPI: remove user's pro-rata liquidity (auto-collects residual fees too)
    let token0_before_liq = ctx.accounts.token0_treasury.amount;
    let token1_before_liq = ctx.accounts.token1_treasury.amount;

    if user_liquidity > 0 {
        cpi::decrease_liquidity_v2(
            CpiContext::new_with_signer(
                ctx.accounts.clmm_program.to_account_info(),
                cpi::accounts::DecreaseLiquidityV2 {
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
                },
                vault_seeds,
            )
            .with_remaining_accounts(remaining),
            user_liquidity,
            0,
            0,
        )?;

        ctx.accounts.token0_treasury.reload()?;
        ctx.accounts.token1_treasury.reload()?;
    }

    let token0_from_position = ctx
        .accounts
        .token0_treasury
        .amount
        .saturating_sub(token0_before_liq);
    let token1_from_position = ctx
        .accounts
        .token1_treasury
        .amount
        .saturating_sub(token1_before_liq);

    let total_token0_out = user_treasury_token0
        .checked_add(token0_from_position)
        .ok_or(error!(VaultError::MathOverflow))?;
    let total_token1_out = user_treasury_token1
        .checked_add(token1_from_position)
        .ok_or(error!(VaultError::MathOverflow))?;

    require!(total_token0_out >= min_token0_out, VaultError::SlippageExceeded);
    require!(total_token1_out >= min_token1_out, VaultError::SlippageExceeded);

    // Burn shares
    token::burn(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Burn {
                mint: ctx.accounts.share_mint.to_account_info(),
                from: ctx.accounts.user_share_account.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        ),
        shares_amount,
    )?;

    // Transfer token0 from treasury to user
    if total_token0_out > 0 {
        let t0_seeds = &[seeds::TOKEN0_TREASURY, vault_key.as_ref(), &[token0_treasury_bump]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.token0_treasury.to_account_info(),
                    to: ctx.accounts.user_token0_account.to_account_info(),
                    authority: ctx.accounts.token0_treasury.to_account_info(),
                },
                &[&t0_seeds[..]],
            ),
            total_token0_out,
        )?;
    }

    // Transfer token1 from treasury to user
    if total_token1_out > 0 {
        let t1_seeds = &[seeds::TOKEN1_TREASURY, vault_key.as_ref(), &[token1_treasury_bump]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.token1_treasury.to_account_info(),
                    to: ctx.accounts.user_token1_account.to_account_info(),
                    authority: ctx.accounts.token1_treasury.to_account_info(),
                },
                &[&t1_seeds[..]],
            ),
            total_token1_out,
        )?;
    }

    ctx.accounts.token0_treasury.reload()?;
    ctx.accounts.token1_treasury.reload()?;

    let current_time = Clock::get()?.unix_timestamp;

    // accumulated_protocol_fees unchanged here — fees are harvested by collect_fees.
    ctx.accounts.vault.treasury_token0 = ctx.accounts.token0_treasury.amount;
    ctx.accounts.vault.treasury_token1 = ctx.accounts.token1_treasury.amount;
    ctx.accounts.vault.position_token0 = ctx
        .accounts
        .vault
        .position_token0
        .saturating_sub(token0_from_position);
    ctx.accounts.vault.position_token1 = ctx
        .accounts
        .vault
        .position_token1
        .saturating_sub(token1_from_position);
    ctx.accounts.vault.position_liquidity = ctx
        .accounts
        .vault
        .position_liquidity
        .saturating_sub(user_liquidity);
    ctx.accounts.vault.total_shares = ctx
        .accounts
        .vault
        .total_shares
        .checked_sub(shares_amount)
        .ok_or(error!(VaultError::MathOverflow))?;

    ctx.accounts.user_deposit.shares = ctx
        .accounts
        .user_deposit
        .shares
        .saturating_sub(shares_amount);
    ctx.accounts.user_deposit.updated_at = current_time;

    emit!(WithdrawEvent {
        user: ctx.accounts.user.key(),
        shares_burned: shares_amount,
        token0_withdrawn: total_token0_out,
        token1_withdrawn: total_token1_out,
        withdrawal_value: 0,
    });

    Ok(())
}
