use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, MintTo, Token, TokenAccount, Transfer};

use crate::errors::VaultError;
use crate::state::{seeds, UserDeposit, Vault};

#[derive(Accounts)]
pub struct DepositSol<'info> {
    /// User making the deposit
    #[account(mut)]
    pub user: Signer<'info>,

    /// Vault state
    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump = vault.bump,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// User's deposit record (created if not exists)
    #[account(
        init_if_needed,
        payer = user,
        space = UserDeposit::LEN,
        seeds = [seeds::USER_DEPOSIT, vault.key().as_ref(), user.key().as_ref()],
        bump,
    )]
    pub user_deposit: Box<Account<'info, UserDeposit>>,

    /// User's wSOL token account (source)
    #[account(
        mut,
        constraint = user_wsol_account.owner == user.key(),
        constraint = user_wsol_account.mint == wsol_mint.key(),
    )]
    pub user_wsol_account: Box<Account<'info, TokenAccount>>,

    /// SOL treasury (destination)
    #[account(
        mut,
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump = vault.sol_treasury_bump,
    )]
    pub sol_treasury: Box<Account<'info, TokenAccount>>,

    /// Share mint
    #[account(
        mut,
        seeds = [seeds::SHARE_MINT, vault.key().as_ref()],
        bump = vault.share_mint_bump,
    )]
    pub share_mint: Box<Account<'info, Mint>>,

    /// User's share token account (will receive shares)
    #[account(
        mut,
        constraint = user_share_account.owner == user.key(),
        constraint = user_share_account.mint == share_mint.key(),
    )]
    pub user_share_account: Box<Account<'info, TokenAccount>>,

    /// Wrapped SOL mint
    pub wsol_mint: Box<Account<'info, Mint>>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<DepositSol>, amount: u64) -> Result<()> {
    require!(amount > 0, VaultError::InvalidAmount);

    let vault = &mut ctx.accounts.vault;
    let user_deposit = &mut ctx.accounts.user_deposit;

    // Check TVL is recent (within 10 minutes)
    let current_time = Clock::get()?.unix_timestamp;
    require!(
        vault.sol_price_usd > 0 && current_time - vault.last_tvl_update < 600,
        VaultError::StaleTvl
    );

    // Calculate deposit value in USD
    let deposit_value_usd = vault.sol_to_usd(amount);
    require!(deposit_value_usd > 0, VaultError::InvalidAmount);

    // Calculate shares to mint
    let shares_to_mint = vault.calculate_shares_to_mint(deposit_value_usd);
    require!(shares_to_mint > 0, VaultError::InvalidAmount);

    // Transfer wSOL from user to treasury
    let cpi_accounts = Transfer {
        from: ctx.accounts.user_wsol_account.to_account_info(),
        to: ctx.accounts.sol_treasury.to_account_info(),
        authority: ctx.accounts.user.to_account_info(),
    };
    let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
    token::transfer(cpi_ctx, amount)?;

    // Mint shares to user
    let vault_key = vault.key();
    let seeds = &[
        seeds::SHARE_MINT,
        vault_key.as_ref(),
        &[vault.share_mint_bump],
    ];
    let signer_seeds = &[&seeds[..]];

    let cpi_accounts = MintTo {
        mint: ctx.accounts.share_mint.to_account_info(),
        to: ctx.accounts.user_share_account.to_account_info(),
        authority: ctx.accounts.share_mint.to_account_info(),
    };
    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        cpi_accounts,
        signer_seeds,
    );
    token::mint_to(cpi_ctx, shares_to_mint)?;

    // Update vault state
    vault.treasury_sol = vault.treasury_sol.checked_add(amount).unwrap();
    vault.total_shares = vault.total_shares.checked_add(shares_to_mint).unwrap();
    vault.tvl_usd = vault.tvl_usd.checked_add(deposit_value_usd).unwrap();

    // Update user deposit record
    if user_deposit.created_at == 0 {
        user_deposit.user = ctx.accounts.user.key();
        user_deposit.vault = vault.key();
        user_deposit.created_at = current_time;
        user_deposit.bump = ctx.bumps.user_deposit;
    }
    user_deposit.shares = user_deposit.shares.checked_add(shares_to_mint).unwrap();
    user_deposit.total_deposited_sol = user_deposit
        .total_deposited_sol
        .checked_add(amount)
        .unwrap();
    user_deposit.updated_at = current_time;

    msg!(
        "Deposited {} lamports SOL (${} USD)",
        amount,
        deposit_value_usd
    );
    msg!("Minted {} shares", shares_to_mint);
    msg!("New total shares: {}", vault.total_shares);
    msg!("New TVL: ${}", vault.tvl_usd);

    Ok(())
}
