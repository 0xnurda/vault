use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, MintTo, Token, TokenAccount, Transfer};
use raydium_clmm_cpi::states::PersonalPositionState;

use crate::constants::{MIN_DEPOSIT_TOKEN0, RAYDIUM_CLMM_PROGRAM_ID};
use crate::errors::VaultError;
use crate::events::DepositToken0Event;
use crate::state::{
    calculate_position_amounts, read_pool_sqrt_price_x64, read_pool_token_mint_0,
    read_pool_tick_current, seeds, sqrt_price_to_price, UserDeposit, Vault,
};

#[derive(Accounts)]
pub struct DepositToken0<'info> {
    #[account(mut)]
    pub user: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
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
        constraint = user_token0_account.owner == user.key(),
        constraint = user_token0_account.mint == vault.token0_mint @ VaultError::InvalidMint,
    )]
    pub user_token0_account: Box<Account<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [seeds::TOKEN0_TREASURY, vault.key().as_ref()],
        bump = vault.token0_treasury_bump,
    )]
    pub token0_treasury: Box<Account<'info, TokenAccount>>,

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

    /// Raydium CLMM pool — price is read on-chain from sqrt_price_x64.
    /// Must be the pool stored in vault.pool_id.
    /// CHECK: ownership verified (Raydium CLMM) + key matches vault.pool_id.
    #[account(
        constraint = raydium_pool.owner == &RAYDIUM_CLMM_PROGRAM_ID @ VaultError::InvalidPriceFeed,
        constraint = raydium_pool.key() == vault.pool_id @ VaultError::InvalidPriceFeed,
    )]
    pub raydium_pool: AccountInfo<'info>,

    /// Raydium personal position PDA: ["position", position_mint].
    /// Validated in handler when has_active_position is true.
    /// Pass any pubkey (e.g. system_program) when there is no active position.
    /// CHECK: key validated in handler via find_program_address when active.
    pub personal_position: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<DepositToken0>, amount: u64) -> Result<()> {
    require!(amount > 0, VaultError::InvalidAmount);
    require!(amount >= MIN_DEPOSIT_TOKEN0, VaultError::DepositTooSmall);

    let vault = &mut ctx.accounts.vault;
    let user_deposit = &mut ctx.accounts.user_deposit;
    let current_time = Clock::get()?.unix_timestamp;

    require!(!vault.is_paused, VaultError::VaultPaused);
    require!(!vault.is_rebalancing, VaultError::RebalancingInProgress);

    // Read pool state: price + tick + token ordering
    let (token0_price_in_token1, sqrt_price_x64, tick_current, token0_is_pool_token0) = {
        let pool_data = ctx.accounts.raydium_pool.try_borrow_data()?;
        let sqrt_price_x64 = read_pool_sqrt_price_x64(&pool_data)
            .ok_or(error!(VaultError::InvalidPriceFeed))?;
        let pool_token_mint_0 = read_pool_token_mint_0(&pool_data)
            .ok_or(error!(VaultError::InvalidPriceFeed))?;
        let tick_current = read_pool_tick_current(&pool_data)
            .ok_or(error!(VaultError::InvalidPriceFeed))?;
        let token0_is_pool_token0 = pool_token_mint_0 == vault.token0_mint;
        let price = sqrt_price_to_price(
            sqrt_price_x64,
            token0_is_pool_token0,
            vault.token0_decimals,
            vault.token1_decimals,
        )
        .ok_or(error!(VaultError::InvalidPriceFeed))?;
        (price, sqrt_price_x64, tick_current, token0_is_pool_token0)
    };
    require!(token0_price_in_token1 > 0, VaultError::InvalidPriceFeed);

    // Compute real-time position amounts (prevents dilution from stale stored values)
    let (pos_token0, pos_token1) = if vault.has_active_position && vault.position_liquidity > 0 {
        let (expected_pda, _) = Pubkey::find_program_address(
            &[b"position", vault.position_mint.as_ref()],
            &raydium_clmm_cpi::id(),
        );
        require!(
            ctx.accounts.personal_position.key() == expected_pda,
            VaultError::InvalidPosition
        );
        let pos_data = ctx.accounts.personal_position.try_borrow_data()?;
        let pos = PersonalPositionState::try_deserialize(&mut &pos_data[..])?;
        drop(pos_data);
        calculate_position_amounts(
            sqrt_price_x64,
            tick_current,
            pos.tick_lower_index,
            pos.tick_upper_index,
            pos.liquidity,
            token0_is_pool_token0,
        )
    } else {
        (0u64, 0u64)
    };

    // TVL in token1 units using real-time position amounts
    let current_tvl = vault.calculate_tvl_with_position(token0_price_in_token1, pos_token0, pos_token1);

    // Deposit value in token1 units
    let deposit_value = vault.token0_to_token1(amount, token0_price_in_token1);
    require!(deposit_value > 0, VaultError::InvalidAmount);

    // Shares to mint
    let shares_to_mint = vault.calculate_shares_to_mint(deposit_value, current_tvl)?;
    require!(shares_to_mint > 0, VaultError::InvalidAmount);

    // Transfer token0 from user to treasury
    token::transfer(
        CpiContext::new(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.user_token0_account.to_account_info(),
                to: ctx.accounts.token0_treasury.to_account_info(),
                authority: ctx.accounts.user.to_account_info(),
            },
        ),
        amount,
    )?;

    // Mint shares to user
    let vault_key = vault.key();
    let mint_seeds = &[seeds::SHARE_MINT, vault_key.as_ref(), &[vault.share_mint_bump]];
    token::mint_to(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            MintTo {
                mint: ctx.accounts.share_mint.to_account_info(),
                to: ctx.accounts.user_share_account.to_account_info(),
                authority: ctx.accounts.share_mint.to_account_info(),
            },
            &[&mint_seeds[..]],
        ),
        shares_to_mint,
    )?;

    // Update vault state
    vault.treasury_token0 = vault.treasury_token0
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
    user_deposit.total_deposited_token0 = user_deposit.total_deposited_token0
        .checked_add(amount)
        .ok_or(error!(VaultError::MathOverflow))?;
    user_deposit.updated_at = current_time;

    let new_tvl = current_tvl.checked_add(deposit_value).unwrap_or(current_tvl);

    emit!(DepositToken0Event {
        user: ctx.accounts.user.key(),
        amount,
        deposit_value,
        shares_minted: shares_to_mint,
        total_shares: vault.total_shares,
        tvl: new_tvl,
        token0_price: token0_price_in_token1,
    });

    Ok(())
}
