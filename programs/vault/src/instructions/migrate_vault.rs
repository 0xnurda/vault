use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, TokenAccount};

use crate::errors::VaultError;
use crate::state::{seeds, Vault};

/// One-time migration instruction to upgrade the vault account to the generic
/// multi-pool layout after a program upgrade.
///
/// Strategy (bypasses broken deserialization):
///   1. Verify admin from raw bytes [8..40] — the first field in both layouts.
///   2. Fund rent if the realloc needs more lamports.
///   3. Realloc vault account to Vault::LEN.
///   4. Write a fresh, correctly-typed Vault struct sourced from:
///       - share_mint.supply      → total_shares
///       - token0_treasury.amount → treasury_token0
///       - token1_treasury.amount → treasury_token1
///       - token0/token1 mint keys from treasury accounts
///       - protocol_wallet / pool_id supplied by admin as params
///
/// All position/rebalancing state is reset to zero/false.
/// The instruction is idempotent.
#[derive(Accounts)]
pub struct MigrateVault<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    /// CHECK: We intentionally skip Anchor deserialization because the old
    /// layout is incompatible with the new Vault struct. The PDA address is
    /// verified via seeds + canonical bump. The admin authority is verified
    /// from raw bytes inside the handler.
    #[account(
        mut,
        seeds = [seeds::VAULT, pool_id_account.key().as_ref()],
        bump,
    )]
    pub vault: AccountInfo<'info>,

    /// Share mint PDA — supply read for total_shares
    #[account(
        seeds = [seeds::SHARE_MINT, vault.key().as_ref()],
        bump,
    )]
    pub share_mint: Account<'info, Mint>,

    /// Token0 treasury PDA — amount read for treasury_token0
    #[account(
        seeds = [seeds::TOKEN0_TREASURY, vault.key().as_ref()],
        bump,
    )]
    pub token0_treasury: Account<'info, TokenAccount>,

    /// Token1 treasury PDA — amount read for treasury_token1
    #[account(
        seeds = [seeds::TOKEN1_TREASURY, vault.key().as_ref()],
        bump,
    )]
    pub token1_treasury: Account<'info, TokenAccount>,

    /// The Raydium CLMM pool — its key is vault.pool_id and part of PDA seeds.
    /// CHECK: Key used for PDA derivation; ownership checked by Raydium constraint in initialize.
    pub pool_id_account: AccountInfo<'info>,

    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn handler(
    ctx: Context<MigrateVault>,
    protocol_wallet: Pubkey,
    token0_decimals: u8,
    token1_decimals: u8,
) -> Result<()> {
    // ── Step 1: Verify admin from raw bytes ───────────────────────────────
    let stored_admin = {
        let data = ctx.accounts.vault.try_borrow_data()?;
        require!(data.len() >= 40, VaultError::Unauthorized);
        Pubkey::from(
            <[u8; 32]>::try_from(&data[8..40])
                .map_err(|_| error!(VaultError::Unauthorized))?,
        )
    };
    require!(stored_admin == ctx.accounts.admin.key(), VaultError::Unauthorized);

    // ── Step 2: Save discriminator before realloc zeros everything ────────
    let discriminator = {
        let data = ctx.accounts.vault.try_borrow_data()?;
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[0..8]);
        disc
    };

    // ── Step 3: Fund vault if realloc needs more rent ─────────────────────
    let current_lamports = ctx.accounts.vault.lamports();
    let new_rent_min = ctx.accounts.rent.minimum_balance(Vault::LEN);
    if current_lamports < new_rent_min {
        let extra = new_rent_min.saturating_sub(current_lamports);
        anchor_lang::system_program::transfer(
            CpiContext::new(
                ctx.accounts.system_program.to_account_info(),
                anchor_lang::system_program::Transfer {
                    from: ctx.accounts.admin.to_account_info(),
                    to: ctx.accounts.vault.to_account_info(),
                },
            ),
            extra,
        )?;
    }

    // ── Step 4: Resize to new size (zero-fills new bytes) ────────────────
    ctx.accounts.vault.resize(Vault::LEN)?;

    // ── Step 5: Build fresh Vault from actual on-chain values ─────────────
    let pool_id = ctx.accounts.pool_id_account.key();
    let token0_mint = ctx.accounts.token0_treasury.mint;
    let token1_mint = ctx.accounts.token1_treasury.mint;

    let new_vault = Vault {
        admin: ctx.accounts.admin.key(),
        share_mint: ctx.accounts.share_mint.key(),
        pool_id,
        token0_mint,
        token1_mint,
        token0_treasury: ctx.accounts.token0_treasury.key(),
        token1_treasury: ctx.accounts.token1_treasury.key(),
        protocol_wallet,
        token0_decimals,
        token1_decimals,
        // Derive from actual mint supply / token balances
        total_shares: ctx.accounts.share_mint.supply,
        treasury_token0: ctx.accounts.token0_treasury.amount,
        treasury_token1: ctx.accounts.token1_treasury.amount,
        // Canonical PDA bumps as computed by Anchor
        bump: ctx.bumps.vault,
        token0_treasury_bump: ctx.bumps.token0_treasury,
        token1_treasury_bump: ctx.bumps.token1_treasury,
        share_mint_bump: ctx.bumps.share_mint,
        // No active position after migration
        position_mint: Pubkey::default(),
        has_active_position: false,
        position_token0: 0,
        position_token1: 0,
        position_liquidity: 0,
        position_tick_lower: 0,
        position_tick_upper: 0,
        // Not paused, not rebalancing
        is_paused: false,
        is_rebalancing: false,
        // No pending admin transfer
        pending_admin: Pubkey::default(),
        // Fees start at zero
        accumulated_protocol_fees_token0: 0,
        accumulated_protocol_fees_token1: 0,
        // No active rebalance
        rebalance_started_at: 0,
    };

    // ── Step 6: Serialize and write back ──────────────────────────────────
    let struct_bytes = new_vault
        .try_to_vec()
        .map_err(|_| error!(VaultError::MathOverflow))?;

    let mut data = ctx.accounts.vault.try_borrow_mut_data()?;
    data[0..8].copy_from_slice(&discriminator);
    data[8..8 + struct_bytes.len()].copy_from_slice(&struct_bytes);

    msg!(
        "✅ Vault migrated: admin={}, pool_id={}, shares={}, token0={}, token1={}",
        ctx.accounts.admin.key(),
        pool_id,
        ctx.accounts.share_mint.supply,
        ctx.accounts.token0_treasury.amount,
        ctx.accounts.token1_treasury.amount,
    );

    Ok(())
}
