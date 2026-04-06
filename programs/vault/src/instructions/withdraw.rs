use anchor_lang::prelude::*;
use anchor_spl::token::{self, Burn, Mint, Token, TokenAccount, Transfer};

use crate::errors::VaultError;
use crate::events::WithdrawEvent;
use crate::state::{seeds, UserDeposit, Vault};

/// Withdraw burns ALL of the user's shares and returns their proportional share
/// of the **total** vault TVL:
///
///   total_user_sol  = treasury_sol  - protocol_fees_sol  + position_sol
///   total_user_usdc = treasury_usdc - protocol_fees_usdc + position_usdc
///   sol_out  = total_user_sol  × (user_shares / total_shares)
///   usdc_out = total_user_usdc × (user_shares / total_shares)
///
/// Payout comes from treasury only (admin keeps a buffer there).
/// If treasury cannot cover the full entitlement the instruction fails with
/// `WithdrawalExceedsTreasury`; the admin must call `decrease_liquidity`
/// (or `close_position`) first to replenish the treasury, then the user retries.
///
/// **position_sol/usdc are NOT reduced here.**  Those fields represent the
/// TOTAL vault position (shared by all remaining holders).  The treasury buffer
/// "fronts" the position portion of the withdrawn user's entitlement; when
/// close_position eventually returns all position funds to treasury the maths
/// resolves correctly for remaining shareholders.
///
/// No Pyth price feed is needed — pure token ratios are used.
#[derive(Accounts)]
pub struct Withdraw<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT],
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
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [seeds::USDC_TREASURY, vault.key().as_ref()],
        bump = vault.usdc_treasury_bump,
    )]
    pub usdc_treasury: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = user_wsol_account.owner == user.key(),
        constraint = user_wsol_account.mint == sol_treasury.mint @ VaultError::InvalidMint,
    )]
    pub user_wsol_account: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        constraint = user_usdc_account.owner == user.key(),
        constraint = user_usdc_account.mint == usdc_treasury.mint @ VaultError::InvalidMint,
    )]
    pub user_usdc_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<Withdraw>) -> Result<()> {
    let vault = &mut ctx.accounts.vault;
    let user_deposit = &mut ctx.accounts.user_deposit;
    let current_time = Clock::get()?.unix_timestamp;

    require!(!vault.is_paused, VaultError::VaultPaused);
    require!(!vault.is_rebalancing, VaultError::RebalancingInProgress);

    let shares_amount = user_deposit.shares;
    require!(shares_amount > 0, VaultError::InsufficientShares);
    require!(
        ctx.accounts.user_share_account.amount >= shares_amount,
        VaultError::InsufficientShares
    );

    let total_shares = vault.total_shares;
    require!(total_shares > 0, VaultError::InsufficientShares);

    // ── Total user-accessible funds = treasury + CLMM position − protocol fees ──
    //
    // position_sol / position_usdc = amounts originally deposited into the open
    // Raydium position (updated at open_position / increase_liquidity /
    // decrease_liquidity; zeroed at close_position).
    //
    // accumulated_protocol_fees belong to the protocol and are excluded.
    let total_user_sol = vault
        .treasury_sol
        .saturating_sub(vault.accumulated_protocol_fees_sol)
        .saturating_add(vault.position_sol);

    let total_user_usdc = vault
        .treasury_usdc
        .saturating_sub(vault.accumulated_protocol_fees_usdc)
        .saturating_add(vault.position_usdc);

    // ── User's proportional entitlement ──────────────────────────────────────
    let sol_to_withdraw = (total_user_sol as u128)
        .checked_mul(shares_amount as u128)
        .and_then(|v| v.checked_div(total_shares as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or(error!(VaultError::MathOverflow))?;

    let usdc_to_withdraw = (total_user_usdc as u128)
        .checked_mul(shares_amount as u128)
        .and_then(|v| v.checked_div(total_shares as u128))
        .and_then(|v| u64::try_from(v).ok())
        .ok_or(error!(VaultError::MathOverflow))?;

    // ── Treasury availability check ───────────────────────────────────────────
    //
    // We can only physically pay from treasury.  If the buffer is insufficient
    // (because most TVL is locked in a Raydium position) the admin must first
    // call decrease_liquidity to replenish treasury, then the user retries.
    let available_sol = ctx
        .accounts
        .sol_treasury
        .amount
        .saturating_sub(vault.accumulated_protocol_fees_sol);

    let available_usdc = ctx
        .accounts
        .usdc_treasury
        .amount
        .saturating_sub(vault.accumulated_protocol_fees_usdc);

    require!(
        sol_to_withdraw <= available_sol,
        VaultError::WithdrawalExceedsTreasury
    );
    require!(
        usdc_to_withdraw <= available_usdc,
        VaultError::WithdrawalExceedsTreasury
    );

    // ── Burn shares ───────────────────────────────────────────────────────────
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

    // ── Transfer SOL (wSOL) from treasury to user ─────────────────────────────
    if sol_to_withdraw > 0 {
        let vault_key = vault.key();
        let seeds = &[seeds::SOL_TREASURY, vault_key.as_ref(), &[vault.sol_treasury_bump]];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.sol_treasury.to_account_info(),
                    to: ctx.accounts.user_wsol_account.to_account_info(),
                    authority: ctx.accounts.sol_treasury.to_account_info(),
                },
                &[&seeds[..]],
            ),
            sol_to_withdraw,
        )?;
    }

    // ── Transfer USDC from treasury to user ───────────────────────────────────
    if usdc_to_withdraw > 0 {
        let vault_key = vault.key();
        let seeds = &[
            seeds::USDC_TREASURY,
            vault_key.as_ref(),
            &[vault.usdc_treasury_bump],
        ];
        token::transfer(
            CpiContext::new_with_signer(
                ctx.accounts.token_program.to_account_info(),
                Transfer {
                    from: ctx.accounts.usdc_treasury.to_account_info(),
                    to: ctx.accounts.user_usdc_account.to_account_info(),
                    authority: ctx.accounts.usdc_treasury.to_account_info(),
                },
                &[&seeds[..]],
            ),
            usdc_to_withdraw,
        )?;
    }

    // ── Update vault accounting ───────────────────────────────────────────────
    //
    // Only treasury and total_shares change.
    // position_sol / position_usdc are intentionally NOT reduced here because
    // they represent the full Raydium position shared by ALL holders.
    // The treasury buffer "fronts" the position portion of this withdrawal;
    // when close_position later returns all position funds the maths resolves
    // correctly:
    //
    //   Example: treasury=2, position=8, shares=1000
    //   User withdraws 10% (100 shares) → entitled to 1 SOL → treasury: 2→1
    //   position_sol stays 8.  900 remaining shares × (1+8)/900 = 0.01 SOL ✓
    //   close_position returns 8 SOL → treasury=9, position=0
    //   900 shares × 9/900 = 0.01 SOL ✓  (total paid out = 1+9 = 10 = original)
    vault.treasury_sol = vault.treasury_sol.saturating_sub(sol_to_withdraw);
    vault.treasury_usdc = vault.treasury_usdc.saturating_sub(usdc_to_withdraw);
    vault.total_shares = vault
        .total_shares
        .checked_sub(shares_amount)
        .ok_or(error!(VaultError::MathOverflow))?;

    // ── Update user deposit record ────────────────────────────────────────────
    user_deposit.shares = user_deposit
        .shares
        .checked_sub(shares_amount)
        .ok_or(error!(VaultError::MathOverflow))?;
    user_deposit.updated_at = current_time;

    emit!(WithdrawEvent {
        user: ctx.accounts.user.key(),
        shares_burned: shares_amount,
        sol_withdrawn: sol_to_withdraw,
        usdc_withdrawn: usdc_to_withdraw,
        withdrawal_value_usd: 0, // calculated off-chain using oracle price
    });

    Ok(())
}
