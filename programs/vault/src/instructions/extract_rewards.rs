use anchor_lang::prelude::*;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

use crate::errors::VaultError;
use crate::events::RewardsExtracted;
use crate::state::{seeds, Vault};

/// Sweep LM reward tokens that accrued to the vault (audit M-4) to the protocol
/// wallet. Rewards are claimed from the Raydium position into a vault-PDA-owned
/// ATA during `collect_fees` / `close_position` (decrease_liquidity_v2 transfers
/// pending rewards). They are NOT part of depositor TVL, so this admin-only
/// instruction realizes them as protocol revenue instead of leaving them as
/// unaccounted "stuck" value in the vault.
#[derive(Accounts)]
pub struct ExtractRewards<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
        constraint = vault.admin == admin.key() @ VaultError::Unauthorized,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// The reward token mint. Must NOT be either pool token — that would let this
    /// path drain principal/fees rather than rewards.
    #[account(
        constraint = reward_mint.key() != vault.token0_mint @ VaultError::InvalidMint,
        constraint = reward_mint.key() != vault.token1_mint @ VaultError::InvalidMint,
    )]
    pub reward_mint: Box<Account<'info, Mint>>,

    /// Vault-owned ATA holding the claimed reward tokens (authority = vault PDA).
    #[account(
        mut,
        constraint = vault_reward_account.owner == vault.key() @ VaultError::Unauthorized,
        constraint = vault_reward_account.mint == reward_mint.key() @ VaultError::InvalidMint,
    )]
    pub vault_reward_account: Box<Account<'info, TokenAccount>>,

    /// Destination — owned by the protocol wallet.
    #[account(
        mut,
        constraint = protocol_reward_account.owner == vault.protocol_wallet @ VaultError::Unauthorized,
        constraint = protocol_reward_account.mint == reward_mint.key() @ VaultError::InvalidMint,
    )]
    pub protocol_reward_account: Box<Account<'info, TokenAccount>>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<ExtractRewards>) -> Result<()> {
    let amount = ctx.accounts.vault_reward_account.amount;
    require!(amount > 0, VaultError::NoRewardsToExtract);

    let vault = &ctx.accounts.vault;
    let pool_id = vault.pool_id;
    let signer: &[&[&[u8]]] = &[&[seeds::VAULT, pool_id.as_ref(), &[vault.bump]]];

    token::transfer(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault_reward_account.to_account_info(),
                to: ctx.accounts.protocol_reward_account.to_account_info(),
                authority: ctx.accounts.vault.to_account_info(),
            },
            signer,
        ),
        amount,
    )?;

    emit!(RewardsExtracted {
        reward_mint: ctx.accounts.reward_mint.key(),
        amount,
        protocol_wallet: vault.protocol_wallet,
    });

    Ok(())
}
