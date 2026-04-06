use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, MintTo, Token, TokenAccount, Transfer};

use crate::constants::{MIN_DEPOSIT_SOL, RAYDIUM_CLMM_PROGRAM_ID, WSOL_MINT};
use crate::errors::VaultError;
use crate::events::DepositSolEvent;
use crate::state::{read_pool_sqrt_price_x64, read_pool_token_mint_0, seeds, sqrt_price_to_sol_usd,
    UserDeposit, Vault};

#[derive(Accounts)]
pub struct DepositSol<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
    )]
    pub vault: Box<Account<'info, Vault>>,

    #[account(
        init_if_needed,
        payer = user,
        space = UserDeposit::LEN,
        seeds = [seeds::USER_DEPOSIT, vault.key().as_ref(), user.key().as_ref()],
        bump,
    )]
    pub user_deposit: Box<Account<'info, UserDeposit>>,

    #[account(
        mut,
        constraint = user_wsol_account.owner == user.key(),
        constraint = user_wsol_account.mint == wsol_mint.key(),
    )]
    pub user_wsol_account: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Box<Account<'info, TokenAccount>>,

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

    pub wsol_mint: Box<Account<'info, Mint>>,

    /// Raydium CLMM SOL/USDC pool — price is read on-chain from sqrt_price_x64.
    /// Must be the pool stored in vault.sol_price_feed (set by admin via update_price).
    /// CHECK: ownership verified (Raydium CLMM) + key matches vault.sol_price_feed.
    #[account(
        constraint = raydium_pool.owner == &RAYDIUM_CLMM_PROGRAM_ID @ VaultError::InvalidPriceFeed,
        constraint = raydium_pool.key() == vault.sol_price_feed @ VaultError::InvalidPriceFeed,
    )]
    pub raydium_pool: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<DepositSol>, amount: u64) -> Result<()> {
    require!(amount > 0, VaultError::InvalidAmount);
    require!(amount >= MIN_DEPOSIT_SOL, VaultError::DepositTooSmall);

    let vault = &mut ctx.accounts.vault;
    let user_deposit = &mut ctx.accounts.user_deposit;
    let current_time = Clock::get()?.unix_timestamp;

    require!(!vault.is_paused, VaultError::VaultPaused);
    require!(!vault.is_rebalancing, VaultError::RebalancingInProgress);

    // Read SOL price live from Raydium CLMM pool (sqrt_price_x64 → USD with 6 decimals)
    let sol_price_usd = {
        let pool_data = ctx.accounts.raydium_pool.try_borrow_data()?;
        let sqrt_price_x64 = read_pool_sqrt_price_x64(&pool_data)
            .ok_or(error!(VaultError::InvalidPriceFeed))?;
        let token_mint_0 = read_pool_token_mint_0(&pool_data)
            .ok_or(error!(VaultError::InvalidPriceFeed))?;
        let sol_is_token0 = token_mint_0 == WSOL_MINT;
        sqrt_price_to_sol_usd(sqrt_price_x64, sol_is_token0)
            .ok_or(error!(VaultError::InvalidSolPrice))?
    };
    require!(sol_price_usd > 0, VaultError::InvalidSolPrice);

    // Calculate TVL on-chain
    let current_tvl = vault.calculate_tvl(sol_price_usd);

    // Calculate deposit value in USD
    let deposit_value_usd = vault.sol_to_usd(amount, sol_price_usd);
    require!(deposit_value_usd > 0, VaultError::InvalidAmount);

    // Calculate shares to mint
    let shares_to_mint = vault.calculate_shares_to_mint(deposit_value_usd, current_tvl)?;
    require!(shares_to_mint > 0, VaultError::InvalidAmount);

    // Transfer wSOL from user to treasury
    let cpi_accounts = Transfer {
        from: ctx.accounts.user_wsol_account.to_account_info(),
        to: ctx.accounts.sol_treasury.to_account_info(),
        authority: ctx.accounts.user.to_account_info(),
    };
    token::transfer(
        CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts),
        amount,
    )?;

    // Mint shares to user
    let vault_key = vault.key();
    let seeds = &[seeds::SHARE_MINT, vault_key.as_ref(), &[vault.share_mint_bump]];
    let cpi_accounts = MintTo {
        mint: ctx.accounts.share_mint.to_account_info(),
        to: ctx.accounts.user_share_account.to_account_info(),
        authority: ctx.accounts.share_mint.to_account_info(),
    };
    token::mint_to(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            cpi_accounts,
            &[&seeds[..]],
        ),
        shares_to_mint,
    )?;

    // Update vault state
    vault.treasury_sol = vault.treasury_sol
        .checked_add(amount)
        .ok_or(error!(VaultError::MathOverflow))?;
    vault.total_shares = vault.total_shares
        .checked_add(shares_to_mint)
        .ok_or(error!(VaultError::MathOverflow))?;

    // Update user deposit record
    if user_deposit.created_at == 0 {
        user_deposit.user = ctx.accounts.user.key();
        user_deposit.vault = vault.key();
        user_deposit.created_at = current_time;
        user_deposit.bump = ctx.bumps.user_deposit;
    }
    user_deposit.shares = user_deposit.shares
        .checked_add(shares_to_mint)
        .ok_or(error!(VaultError::MathOverflow))?;
    user_deposit.total_deposited_sol = user_deposit.total_deposited_sol
        .checked_add(amount)
        .ok_or(error!(VaultError::MathOverflow))?;
    user_deposit.updated_at = current_time;

    let new_tvl = current_tvl.checked_add(deposit_value_usd).unwrap_or(current_tvl);

    emit!(DepositSolEvent {
        user: ctx.accounts.user.key(),
        amount,
        deposit_value_usd,
        shares_minted: shares_to_mint,
        total_shares: vault.total_shares,
        tvl_usd: new_tvl,
        sol_price_usd,
    });

    Ok(())
}
