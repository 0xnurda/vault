use anchor_lang::prelude::*;
use anchor_spl::token::{Mint, TokenAccount};

use crate::constants::RAYDIUM_CLMM_PROGRAM_ID;
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

    /// token0 mint — decimals read from here, NOT from handler args (audit #3).
    #[account(
        constraint = token0_mint.key() == token0_treasury.mint @ VaultError::InvalidMint,
    )]
    pub token0_mint: Account<'info, Mint>,

    /// token1 mint — decimals read from here, NOT from handler args (audit #3).
    #[account(
        constraint = token1_mint.key() == token1_treasury.mint @ VaultError::InvalidMint,
    )]
    pub token1_mint: Account<'info, Mint>,

    /// The Raydium CLMM pool — validated to be owned by Raydium (audit #3).
    /// CHECK: Ownership validated against RAYDIUM_CLMM_PROGRAM_ID.
    #[account(
        constraint = pool_id_account.owner == &RAYDIUM_CLMM_PROGRAM_ID @ VaultError::InvalidPriceFeed,
    )]
    pub pool_id_account: AccountInfo<'info>,

    /// Raydium personal position PDA, used ONLY to verify there is no live
    /// liquidity before wiping position state (audit C1). When the old vault has
    /// has_active_position = true, this must be the real position and have
    /// liquidity == 0. Pass system_program as a dummy when there is no position.
    /// CHECK: validated in handler against the old layout's position_mint.
    pub personal_position: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn handler(
    ctx: Context<MigrateVault>,
    protocol_wallet: Pubkey,
) -> Result<()> {
    // ── Idempotency: skip if already migrated (audit #3) ─────────────────
    // A migrated vault is exactly Vault::LEN bytes with a valid pool_id at
    // the known offset (8 disc + 32 admin + 32 share_mint = 72 → pool_id at 72..104).
    {
        let data = ctx.accounts.vault.try_borrow_data()?;
        if data.len() == Vault::LEN && data.len() >= 104 {
            let stored_pool_id = Pubkey::from(
                <[u8; 32]>::try_from(&data[72..104]).unwrap_or([0u8; 32]),
            );
            if stored_pool_id == ctx.accounts.pool_id_account.key() {
                msg!("Vault already migrated to current layout — skipping.");
                return Ok(());
            }
        }
    }

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

    // ── C1: refuse to migrate while LIQUIDITY is live ─────────────────────
    // Migration zeroes position_mint/liquidity. If a position still holds
    // liquidity on Raydium, the vault would forget its NFT and the funds would
    // be unreachable (close_position/withdraw_from_position need position_mint).
    // We gate on position_liquidity (the real danger), not has_active_position:
    // a stuck flag with zero liquidity (already-drained position) is safe to
    // migrate, but any non-zero liquidity must be closed first.
    //
    // Old-layout offsets (stable — operator was appended at the end):
    //   ... position_mint[294..326], has_active_position[326],
    //   position_token0[327..335], position_token1[335..343],
    //   position_liquidity(u128)[343..359]
    {
        let data = ctx.accounts.vault.try_borrow_data()?;
        if data.len() >= 359 {
            let liq_bytes: [u8; 16] = data[343..359].try_into()
                .map_err(|_| error!(VaultError::InvalidPosition))?;
            let position_liquidity = u128::from_le_bytes(liq_bytes);
            require!(position_liquidity == 0, VaultError::PositionAlreadyExists);
        }
    }

    // ── C1 guard: refuse to wipe a LIVE position ──────────────────────────
    // Migration zeroes position state. If the old vault has an active position
    // with real liquidity, wiping position_mint would orphan the NFT + funds on
    // Raydium forever. Read has_active_position + position_mint from the old
    // layout (offsets stable across both layouts) and, if active, require the
    // real on-chain position to be empty (liquidity == 0).
    // Old/new layout offsets: position_mint at 294..326, has_active_position at 326.
    {
        let data = ctx.accounts.vault.try_borrow_data()?;
        if data.len() > 326 && data[326] == 1 {
            let position_mint = Pubkey::from(
                <[u8; 32]>::try_from(&data[294..326])
                    .map_err(|_| error!(VaultError::InvalidPosition))?,
            );
            drop(data);

            // The passed personal_position must be the real PDA for this position.
            let (expected_pda, _) = Pubkey::find_program_address(
                &[b"position", position_mint.as_ref()],
                &RAYDIUM_CLMM_PROGRAM_ID,
            );
            require!(
                ctx.accounts.personal_position.key() == expected_pda,
                VaultError::InvalidPosition
            );

            // Read real liquidity from PersonalPositionState: bump(1) + nft_mint(32)
            // + pool_id(32) + tick_lower(4) + tick_upper(4) = 73 (+8 disc) → liquidity at 81.
            let pos_data = ctx.accounts.personal_position.try_borrow_data()?;
            require!(pos_data.len() >= 97, VaultError::InvalidPosition);
            let liquidity = u128::from_le_bytes(
                <[u8; 16]>::try_from(&pos_data[81..97])
                    .map_err(|_| error!(VaultError::InvalidPosition))?,
            );
            require!(liquidity == 0, VaultError::PositionAlreadyExists);
        }
    }

    // ── Step 2: Save discriminator + old total_shares before realloc ──────
    // total_shares lives at a stable offset (264..272) in both layouts, before
    // any changed field. Preserve it directly so the phantom DEAD_SHARES stay in
    // the count (audit L2) — recomputing from share_mint.supply would drop them
    // and nudge the share price.
    let (discriminator, old_total_shares) = {
        let data = ctx.accounts.vault.try_borrow_data()?;
        let mut disc = [0u8; 8];
        disc.copy_from_slice(&data[0..8]);
        let ts = if data.len() >= 272 {
            u64::from_le_bytes(<[u8; 8]>::try_from(&data[264..272]).unwrap_or([0u8; 8]))
        } else {
            0
        };
        (disc, ts)
    };
    // Fall back to mint supply if the old counter is somehow zero.
    let preserved_total_shares = if old_total_shares > 0 {
        old_total_shares
    } else {
        ctx.accounts.share_mint.supply
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
    // Read decimals from mint accounts — NOT from args (audit #3 fix)
    let token0_decimals = ctx.accounts.token0_mint.decimals;
    let token1_decimals = ctx.accounts.token1_mint.decimals;

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
        total_shares: preserved_total_shares,
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
        // Default operator = admin; rotate later via set_operator
        operator: ctx.accounts.admin.key(),
        // Swap rate-limit window starts fresh (audit H1)
        swap_window_start: 0,
        swap_volume_in_window: 0,
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
