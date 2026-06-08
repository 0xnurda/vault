use anchor_lang::prelude::*;
use raydium_clmm_cpi::states::{PersonalPositionState, PoolState};

use crate::errors::VaultError;
use crate::state::{calculate_position_amounts, seeds, Vault};

/// Sync vault.position_token0 / position_token1 with real on-chain amounts.
///
/// position_token0/token1 are set at open_position / increase_liquidity time and
/// become stale as the CLMM pool price moves (CLMM auto-rebalances token mix).
/// This instruction reads the actual amounts from the Raydium position and
/// updates the vault state so TVL calculations and share pricing are accurate.
///
/// Should be called by the admin bot periodically (e.g. before each deposit or
/// on a cron every few minutes).
/// Anyone can call sync_position_value — it only reads on-chain state and
/// updates bookkeeping.  Making it permissionless lets users sync before
/// calling withdraw so that entitlement math uses accurate values.
#[derive(Accounts)]
pub struct SyncPositionValue<'info> {
    // No signer requirement — permissionless read + bookkeeping update.
    /// CHECK: not used, kept for future compatibility
    pub caller: UncheckedAccount<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
        constraint = vault.has_active_position @ VaultError::NoActivePosition,
        constraint = vault.pool_id == pool_state.key() @ VaultError::InvalidPriceFeed,
    )]
    pub vault: Box<Account<'info, Vault>>,

    /// Raydium CLMM pool — provides current sqrt_price_x64 and tick_current.
    pub pool_state: AccountLoader<'info, PoolState>,

    /// Raydium personal position PDA: ["position", position_mint].
    /// CHECK: key validated against vault.position_mint below.
    pub personal_position: UncheckedAccount<'info>,
}

pub fn sync_handler(ctx: Context<SyncPositionValue>) -> Result<()> {
    let vault = &mut ctx.accounts.vault;

    // ── Validate personal_position PDA ───────────────────────────────────────
    let (expected_pda, _) = Pubkey::find_program_address(
        &[b"position", vault.position_mint.as_ref()],
        &raydium_clmm_cpi::id(),
    );
    require!(
        ctx.accounts.personal_position.key() == expected_pda,
        VaultError::InvalidPosition
    );

    // ── Read position state ───────────────────────────────────────────────────
    let position_data = ctx.accounts.personal_position.try_borrow_data()?;
    let pos = PersonalPositionState::try_deserialize(&mut &position_data[..])?;
    let liquidity = pos.liquidity;
    let tick_lower = pos.tick_lower_index;
    let tick_upper = pos.tick_upper_index;
    drop(position_data);

    if liquidity == 0 {
        vault.position_token0 = 0;
        vault.position_token1 = 0;
        vault.position_liquidity = 0;
        return Ok(());
    }

    // ── Read pool state ───────────────────────────────────────────────────────
    let pool = ctx.accounts.pool_state.load()?;
    let sqrt_price_x64 = pool.sqrt_price_x64;
    let tick_current = pool.tick_current;
    let token0_is_pool_token0 = pool.token_mint_0 == vault.token0_mint;
    drop(pool);

    // ── Calculate real amounts using shared CLMM math ────────────────────────
    let (pos_token0, pos_token1) = calculate_position_amounts(
        sqrt_price_x64,
        tick_current,
        tick_lower,
        tick_upper,
        liquidity,
        token0_is_pool_token0,
    );

    // ── Update vault ──────────────────────────────────────────────────────────
    vault.position_token0 = pos_token0;
    vault.position_token1 = pos_token1;
    vault.position_liquidity = liquidity;

    msg!(
        "sync_position_value: token0={} token1={} liquidity={} tick_current={}",
        vault.position_token0,
        vault.position_token1,
        liquidity,
        tick_current,
    );

    Ok(())
}
