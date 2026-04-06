use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, TokenAccount};

use crate::errors::VaultError;
use crate::state::{seeds, Vault};

/// One-time migration instruction to upgrade the vault account from the old
/// layout (389 bytes) to the new layout (447 bytes) after a program upgrade.
///
/// Strategy (bypasses broken deserialization):
///   1. Verify admin from raw bytes [8..40] — the first field in both layouts.
///   2. Fund rent if the realloc needs more lamports.
///   3. Realloc vault account to Vault::LEN.
///   4. Write a fresh, correctly-typed Vault struct sourced from:
///       - share_mint.supply  → total_shares
///       - sol_treasury.amount → treasury_sol
///       - usdc_treasury.amount → treasury_usdc
///       - protocol_wallet / sol_price_feed  → supplied by admin as params
///
/// All position/rebalancing state is reset to zero/false — the vault had no
/// active position when this migration is needed.
///
/// The instruction is idempotent: re-running it overwrites state with the
/// same values derived from actual on-chain accounts.
#[derive(Accounts)]
pub struct MigrateVault<'info> {
    /// Admin who authorizes the migration
    #[account(mut)]
    pub admin: Signer<'info>,

    /// CHECK: We intentionally skip Anchor deserialization because the old
    /// layout is incompatible with the new Vault struct.  The PDA address is
    /// verified via seeds + canonical bump.  The admin authority is verified
    /// from raw bytes inside the handler.
    #[account(
        mut,
        seeds = [seeds::VAULT],
        bump,
    )]
    pub vault: AccountInfo<'info>,

    /// Share mint PDA — supply read for total_shares
    #[account(
        seeds = [seeds::SHARE_MINT, vault.key().as_ref()],
        bump,
    )]
    pub share_mint: Account<'info, Mint>,

    /// SOL (wSOL) treasury PDA — amount read for treasury_sol
    #[account(
        seeds = [seeds::SOL_TREASURY, vault.key().as_ref()],
        bump,
    )]
    pub sol_treasury: Account<'info, TokenAccount>,

    /// USDC treasury PDA — amount read for treasury_usdc; mint key stored in vault
    #[account(
        seeds = [seeds::USDC_TREASURY, vault.key().as_ref()],
        bump,
    )]
    pub usdc_treasury: Account<'info, TokenAccount>,

    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn handler(
    ctx: Context<MigrateVault>,
    protocol_wallet: Pubkey,
    sol_price_feed: Pubkey,
) -> Result<()> {
    // ── Step 1: Verify admin from raw bytes ───────────────────────────────
    // In both the old and new layouts, `admin` is the first Pubkey field
    // immediately after the 8-byte discriminator → bytes [8..40].
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
    let new_vault = Vault {
        admin: ctx.accounts.admin.key(),
        share_mint: ctx.accounts.share_mint.key(),
        sol_treasury: ctx.accounts.sol_treasury.key(),
        usdc_treasury: ctx.accounts.usdc_treasury.key(),
        // usdc_mint stored inside the treasury token-account metadata
        usdc_mint: ctx.accounts.usdc_treasury.mint,
        protocol_wallet,
        sol_price_feed,
        // Derive from actual mint supply / token balances
        total_shares: ctx.accounts.share_mint.supply,
        treasury_sol: ctx.accounts.sol_treasury.amount,
        treasury_usdc: ctx.accounts.usdc_treasury.amount,
        // Canonical PDA bumps as computed by Anchor
        bump: ctx.bumps.vault,
        sol_treasury_bump: ctx.bumps.sol_treasury,
        usdc_treasury_bump: ctx.bumps.usdc_treasury,
        share_mint_bump: ctx.bumps.share_mint,
        // No active position after migration
        position_mint: Pubkey::default(),
        has_active_position: false,
        position_sol: 0,
        position_usdc: 0,
        position_liquidity: 0,
        position_tick_lower: 0,
        position_tick_upper: 0,
        position_pool_id: Pubkey::default(),
        // Not paused, not rebalancing
        is_paused: false,
        is_rebalancing: false,
        // No pending admin transfer
        pending_admin: Pubkey::default(),
        // Fees start at zero (pre-migration fees assumed extracted or none)
        accumulated_protocol_fees_sol: 0,
        accumulated_protocol_fees_usdc: 0,
        // Price not set after migration — admin must call update_price
        sol_price_usd: 0,
        last_price_update: 0,
    };

    // ── Step 6: Serialize and write back ──────────────────────────────────
    // Vault::LEN = 8 (disc) + 415 (struct) + 32 (padding zeros) = 455… no,
    // LEN = 447; struct serialises to 415 bytes; padding 32 bytes remain zero.
    let struct_bytes = new_vault
        .try_to_vec()
        .map_err(|_| error!(VaultError::MathOverflow))?;

    let mut data = ctx.accounts.vault.try_borrow_mut_data()?;
    // Restore discriminator (realloc with zero_init zeroed it)
    data[0..8].copy_from_slice(&discriminator);
    // Write struct body immediately after discriminator
    data[8..8 + struct_bytes.len()].copy_from_slice(&struct_bytes);
    // Bytes [8 + struct_bytes.len()..Vault::LEN] remain zero (padding)

    msg!(
        "✅ Vault migrated: admin={}, shares={}, sol={}, usdc={}",
        ctx.accounts.admin.key(),
        ctx.accounts.share_mint.supply,
        ctx.accounts.sol_treasury.amount,
        ctx.accounts.usdc_treasury.amount,
    );

    Ok(())
}
