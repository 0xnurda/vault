use anchor_lang::prelude::*;
use anchor_spl::memo::Memo;
use anchor_spl::token::Token;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_clmm_cpi::{
    cpi,
    states::{AmmConfig, PoolState},
};

use crate::errors::VaultError;
use crate::events::SwapEvent;
use crate::state::{
    reference_sqrt_price, seeds, swap_min_out_floor, value_in_token1,
    Vault, MAX_SWAP_VOLUME_BPS, SWAP_COOLDOWN_SECS, SWAP_WINDOW_SECS,
};

/// Swap direction enum
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq)]
pub enum SwapDirection {
    /// Swap token0 → token1
    Token0ToToken1,
    /// Swap token1 → token0
    Token1ToToken0,
}

#[derive(Accounts)]
pub struct SwapInTreasury<'info> {
    #[account(mut)]
    pub admin: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
        constraint = vault.is_operator(&admin.key()) @ VaultError::Unauthorized,
    )]
    pub vault: Box<Account<'info, Vault>>,

    #[account(
        mut,
        seeds = [seeds::TOKEN0_TREASURY, vault.key().as_ref()],
        bump = vault.token0_treasury_bump,
    )]
    pub token0_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        seeds = [seeds::TOKEN1_TREASURY, vault.key().as_ref()],
        bump = vault.token1_treasury_bump,
    )]
    pub token1_treasury: Box<InterfaceAccount<'info, TokenAccount>>,

    pub amm_config: Box<Account<'info, AmmConfig>>,

    /// Must be the vault's own pool — otherwise an operator could route the swap
    /// through a self-created pool with a fake price and drain the treasury (C-1).
    #[account(
        mut,
        constraint = pool_state.key() == vault.pool_id @ VaultError::InvalidPriceFeed,
    )]
    pub pool_state: AccountLoader<'info, PoolState>,

    #[account(mut)]
    pub input_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub output_vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: Raydium CLMM ObservationState. Bound to the pool in the handler via
    /// `pool.observation_key == observation_state.key()` (C-1); the swap CPI writes
    /// to it. Read as raw bytes for the TWAP floor (current 100-slot oracle layout).
    #[account(mut)]
    pub observation_state: UncheckedAccount<'info>,

    pub input_vault_mint: Box<InterfaceAccount<'info, Mint>>,
    pub output_vault_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: Validated by address constraint
    #[account(address = raydium_clmm_cpi::id())]
    pub clmm_program: UncheckedAccount<'info>,

    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub memo_program: Program<'info, Memo>,
}

pub fn handler<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, SwapInTreasury<'info>>,
    amount_in: u64,
    minimum_amount_out: u64,
    direction: SwapDirection,
) -> Result<()> {
    require!(amount_in > 0, VaultError::InvalidAmount);
    require!(minimum_amount_out > 0, VaultError::InvalidAmount);

    let vault = &ctx.accounts.vault;

    // ── C-1: bind oracle + amm_config + pool vaults to the vault's real pool ──
    // pool_state is already constrained == vault.pool_id (accounts struct). Now
    // verify the observation, amm_config and the Raydium token vaults are THIS
    // pool's — so an operator cannot supply a self-made pool's oracle/vaults and
    // bypass the floor/volume-cap (which are computed from the oracle).
    {
        let pool = ctx.accounts.pool_state.load()?;
        require!(
            pool.observation_key == ctx.accounts.observation_state.key(),
            VaultError::InvalidPriceFeed
        );
        require!(
            pool.amm_config == ctx.accounts.amm_config.key(),
            VaultError::InvalidPriceFeed
        );
        // input/output_vault must be the pool's own token vaults (either order).
        let (tv0, tv1) = (pool.token_vault_0, pool.token_vault_1);
        let inv = ctx.accounts.input_vault.key();
        let outv = ctx.accounts.output_vault.key();
        require!(
            (inv == tv0 && outv == tv1) || (inv == tv1 && outv == tv0),
            VaultError::InvalidMint
        );
    }

    // Treasury swap is safe during an active position:
    // token0_treasury / token1_treasury are separate accounts from the Raydium
    // position — swapping only affects treasury balances, not the locked position.

    // ── TWAP-floor + rate-limit: neutralize operator drain (audit #4, H1) ──────
    // The contract derives its OWN minimum output from a ≥30-second-old observation
    // (manipulation-resistant) and rejects a swap below that floor. It also caps
    // cumulative swap volume per window, so a compromised operator cannot bleed the
    // treasury via repeated near-floor self-sandwich swaps.
    let (new_window_start, new_window_volume) = {
        let now = Clock::get()?.unix_timestamp;
        // Fail-safe: a vault holding real funds must NOT swap without an oracle
        // reference. We are swapping treasury funds here, so require history.
        // (Borrow is scoped to this block and released before the swap CPI.)
        let ref_sqrt = {
            let obs_data = ctx.accounts.observation_state.try_borrow_data()?;
            reference_sqrt_price(&obs_data).ok_or(error!(VaultError::OracleUnavailable))?
        };

        // Direction from the validated `direction` + vault mints (audit [D]).
        let pool = ctx.accounts.pool_state.load()?;
        let pool_token0 = pool.token_mint_0;
        drop(pool);
        let vault_token0_is_pool_token0 = vault.token0_mint == pool_token0;
        let input_is_pool_token0 = match direction {
            SwapDirection::Token0ToToken1 => vault_token0_is_pool_token0,
            SwapDirection::Token1ToToken0 => !vault_token0_is_pool_token0,
        };

        // 1) Per-swap floor
        let floor = swap_min_out_floor(ref_sqrt, amount_in, input_is_pool_token0)
            .ok_or(error!(VaultError::MathOverflow))?;
        require!(minimum_amount_out >= floor, VaultError::SlippageExceeded);

        // 2) Per-window volume cap (in pool-token1 units)
        let swap_value = value_in_token1(ref_sqrt, amount_in, input_is_pool_token0)
            .ok_or(error!(VaultError::MathOverflow))?;

        // Treasury value in pool-token1 units = pool_token1_amount + pool_token0_amount × P
        let (pool_token0_treasury, pool_token1_treasury) = if vault_token0_is_pool_token0 {
            (vault.treasury_token0, vault.treasury_token1)
        } else {
            (vault.treasury_token1, vault.treasury_token0)
        };
        let treasury_value = (pool_token1_treasury as u128).saturating_add(
            value_in_token1(ref_sqrt, pool_token0_treasury, true).unwrap_or(0),
        );
        let cap = treasury_value
            .checked_mul(MAX_SWAP_VOLUME_BPS)
            .and_then(|v| v.checked_div(10_000))
            .unwrap_or(0);

        // 3) Cooldown between swaps (audit M-3). last_swap_at == 0 (never swapped)
        // passes naturally since now - 0 is far larger than the cooldown.
        require!(
            now.saturating_sub(vault.last_swap_at) >= SWAP_COOLDOWN_SECS,
            VaultError::SwapCooldownActive
        );

        // Reset the window if it has elapsed, else accumulate.
        let (mut window_start, mut window_volume) =
            (vault.swap_window_start, vault.swap_volume_in_window as u128);
        if now.saturating_sub(window_start) >= SWAP_WINDOW_SECS {
            window_start = now;
            window_volume = 0;
        }
        window_volume = window_volume.saturating_add(swap_value);
        require!(window_volume <= cap, VaultError::SwapVolumeExceeded);

        (window_start, u64::try_from(window_volume).unwrap_or(u64::MAX))
    };

    let (input_treasury, output_treasury) = match direction {
        SwapDirection::Token0ToToken1 => {
            require!(
                ctx.accounts.token0_treasury.amount >= amount_in,
                VaultError::InsufficientTreasuryBalance
            );
            // Validate that the caller passed Raydium vaults matching our token ordering.
            // pool.token_vault_N may be vault.token0 or vault.token1 depending on the pool —
            // this check ensures the script isn't hard-coded for a specific pool layout.
            require!(
                ctx.accounts.input_vault.mint == vault.token0_mint,
                VaultError::InvalidMint
            );
            require!(
                ctx.accounts.output_vault.mint == vault.token1_mint,
                VaultError::InvalidMint
            );
            (&ctx.accounts.token0_treasury, &ctx.accounts.token1_treasury)
        }
        SwapDirection::Token1ToToken0 => {
            require!(
                ctx.accounts.token1_treasury.amount >= amount_in,
                VaultError::InsufficientTreasuryBalance
            );
            require!(
                ctx.accounts.input_vault.mint == vault.token1_mint,
                VaultError::InvalidMint
            );
            require!(
                ctx.accounts.output_vault.mint == vault.token0_mint,
                VaultError::InvalidMint
            );
            (&ctx.accounts.token1_treasury, &ctx.accounts.token0_treasury)
        }
    };

    let vault_key = vault.key();
    let (treasury_seed, treasury_bump): (&[u8], u8) = match direction {
        SwapDirection::Token0ToToken1 => (seeds::TOKEN0_TREASURY, vault.token0_treasury_bump),
        SwapDirection::Token1ToToken0 => (seeds::TOKEN1_TREASURY, vault.token1_treasury_bump),
    };

    let signer_seeds: &[&[&[u8]]] = &[&[
        treasury_seed,
        vault_key.as_ref(),
        &[treasury_bump],
    ]];

    let cpi_accounts = cpi::accounts::SwapSingleV2 {
        payer: input_treasury.to_account_info(),
        amm_config: ctx.accounts.amm_config.to_account_info(),
        pool_state: ctx.accounts.pool_state.to_account_info(),
        input_token_account: input_treasury.to_account_info(),
        output_token_account: output_treasury.to_account_info(),
        input_vault: ctx.accounts.input_vault.to_account_info(),
        output_vault: ctx.accounts.output_vault.to_account_info(),
        observation_state: ctx.accounts.observation_state.to_account_info(),
        token_program: ctx.accounts.token_program.to_account_info(),
        token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
        memo_program: ctx.accounts.memo_program.to_account_info(),
        input_vault_mint: ctx.accounts.input_vault_mint.to_account_info(),
        output_vault_mint: ctx.accounts.output_vault_mint.to_account_info(),
    };

    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.clmm_program.to_account_info(),
        cpi_accounts,
        signer_seeds,
    )
    .with_remaining_accounts(ctx.remaining_accounts.to_vec());

    cpi::swap_v2(cpi_ctx, amount_in, minimum_amount_out, 0, true)?;

    ctx.accounts.token0_treasury.reload()?;
    ctx.accounts.token1_treasury.reload()?;

    let vault = &mut ctx.accounts.vault.as_mut();
    vault.treasury_token0 = ctx.accounts.token0_treasury.amount;
    vault.treasury_token1 = ctx.accounts.token1_treasury.amount;
    // Persist the swap rate-limit window (audit H1) and cooldown stamp (audit M-3).
    vault.swap_window_start = new_window_start;
    vault.swap_volume_in_window = new_window_volume;
    vault.last_swap_at = Clock::get()?.unix_timestamp;

    emit!(SwapEvent {
        amount_in,
        direction: if direction == SwapDirection::Token0ToToken1 {
            "TOKEN0->TOKEN1".to_string()
        } else {
            "TOKEN1->TOKEN0".to_string()
        },
        treasury_token0: vault.treasury_token0,
        treasury_token1: vault.treasury_token1,
    });

    Ok(())
}
