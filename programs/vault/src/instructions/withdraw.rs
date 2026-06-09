use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, Token, TokenAccount, Transfer};

use crate::errors::VaultError;
use crate::events::WithdrawEvent;
use crate::state::{seeds, UserDeposit, Vault};

#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
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
    pub token0_treasury: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [seeds::TOKEN1_TREASURY, vault.key().as_ref()],
        bump = vault.token1_treasury_bump,
    )]
    pub token1_treasury: Box<Account<'info, TokenAccount>>,

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

    pub token_program: Program<'info, Token>,
}

/// `min_token0_out` — minimum token0 amount accepted (0 = no check)
/// `min_token1_out` — minimum token1 amount accepted (0 = no check)
pub fn handler(ctx: Context<Withdraw>, min_token0_out: u64, min_token1_out: u64) -> Result<()> {
    let vault = &mut ctx.accounts.vault;
    let user_deposit = &mut ctx.accounts.user_deposit;
    let current_time = Clock::get()?.unix_timestamp;

    require!(!vault.is_paused, VaultError::VaultPaused);

    // When a position is active users must use withdraw_from_position (which
    // removes their pro-rata liquidity) instead of this instruction (which only
    // draws from the treasury). Using this while a position is active allows
    // callers to claim an entitlement inflated by position_token0/token1 while
    // only drawing from treasury, leaving later withdrawers short.
    require!(!vault.has_active_position, VaultError::PositionAlreadyExists);

    // Emergency withdrawal: allow after 3600s of stuck rebalance
    if vault.is_rebalancing {
        let elapsed = current_time.saturating_sub(vault.rebalance_started_at);
        require!(
            vault.rebalance_started_at > 0 && elapsed >= 3600,
            VaultError::RebalancingInProgress
        );
    }

    // Use actual share token balance — not the cached user_deposit.shares counter.
    // This allows shares to be freely transferred between wallets: whoever
    // holds the share tokens can always redeem them (audit finding #2).
    let shares_amount = ctx.accounts.user_share_account.amount;
    require!(shares_amount > 0, VaultError::InsufficientShares);

    let total_shares = vault.total_shares;
    require!(total_shares > 0, VaultError::InsufficientShares);

    // Total user-accessible funds = treasury + position - protocol fees
    let total_user_token0 = vault
        .treasury_token0
        .saturating_sub(vault.accumulated_protocol_fees_token0)
        .saturating_add(vault.position_token0);

    let total_user_token1 = vault
        .treasury_token1
        .saturating_sub(vault.accumulated_protocol_fees_token1)
        .saturating_add(vault.position_token1);

    // User's proportional entitlement
    let token0_to_withdraw = (total_user_token0 as u128)
        .checked_mul(shares_amount as u128)
        .and_then(|v| v.checked_div(total_shares as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or(error!(VaultError::MathOverflow))?;

    let token1_to_withdraw = (total_user_token1 as u128)
        .checked_mul(shares_amount as u128)
        .and_then(|v| v.checked_div(total_shares as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or(error!(VaultError::MathOverflow))?;

    // Treasury availability check
    let available_token0 = ctx
        .accounts
        .token0_treasury
        .amount
        .saturating_sub(vault.accumulated_protocol_fees_token0);

    let available_token1 = ctx
        .accounts
        .token1_treasury
        .amount
        .saturating_sub(vault.accumulated_protocol_fees_token1);

    require!(
        token0_to_withdraw <= available_token0,
        VaultError::WithdrawalExceedsTreasury
    );
    require!(
        token1_to_withdraw <= available_token1,
        VaultError::WithdrawalExceedsTreasury
    );

    // Slippage check (before irreversible burn)
    require!(token0_to_withdraw >= min_token0_out, VaultError::SlippageExceeded);
    require!(token1_to_withdraw >= min_token1_out, VaultError::SlippageExceeded);

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
    if token0_to_withdraw > 0 {
        let vault_key = vault.key();
        let seeds = &[seeds::TOKEN0_TREASURY, vault_key.as_ref(), &[vault.token0_treasury_bump]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.token0_treasury.to_account_info(),
                    to: ctx.accounts.user_token0_account.to_account_info(),
                    authority: ctx.accounts.token0_treasury.to_account_info(),
                },
                &[&seeds[..]],
            ),
            token0_to_withdraw,
        )?;
    }

    // Transfer token1 from treasury to user
    if token1_to_withdraw > 0 {
        let vault_key = vault.key();
        let seeds = &[seeds::TOKEN1_TREASURY, vault_key.as_ref(), &[vault.token1_treasury_bump]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.token1_treasury.to_account_info(),
                    to: ctx.accounts.user_token1_account.to_account_info(),
                    authority: ctx.accounts.token1_treasury.to_account_info(),
                },
                &[&seeds[..]],
            ),
            token1_to_withdraw,
        )?;
    }

    // Update vault accounting
    vault.treasury_token0 = vault.treasury_token0.saturating_sub(token0_to_withdraw);
    vault.treasury_token1 = vault.treasury_token1.saturating_sub(token1_to_withdraw);
    vault.total_shares = vault
        .total_shares
        .checked_sub(shares_amount)
        .ok_or(error!(VaultError::MathOverflow))?;

    // Update user deposit record.
    // Zero out the counter — shares are now redeemed by token balance, not counter.
    // saturating_sub keeps it valid even if shares were transferred in from elsewhere.
    user_deposit.shares = user_deposit.shares.saturating_sub(shares_amount);
    user_deposit.updated_at = current_time;

    emit!(WithdrawEvent {
        user: ctx.accounts.user.key(),
        shares_burned: shares_amount,
        token0_withdrawn: token0_to_withdraw,
        token1_withdrawn: token1_to_withdraw,
        withdrawal_value: 0,
    });

    Ok(())
}
