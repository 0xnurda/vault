use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::Token;
use anchor_spl::token_2022::Token2022;
use anchor_spl::token_interface::{Mint, TokenAccount};
use raydium_clmm_cpi::{
    cpi,
    states::{PersonalPositionState, PoolState},
};

use crate::errors::VaultError;
use crate::events::PositionOpened;
use crate::state::{seeds, validate_position_range, Vault};

#[derive(Accounts)]
#[instruction(tick_lower_index: i32, tick_upper_index: i32, tick_array_lower_start_index: i32, tick_array_upper_start_index: i32)]
pub struct OpenPosition<'info> {
    #[account(mut)]
    pub operator: Signer<'info>,

    #[account(
        mut,
        seeds = [seeds::VAULT, vault.pool_id.as_ref()],
        bump = vault.bump,
        constraint = vault.is_operator(&operator.key()) @ VaultError::Unauthorized,
        constraint = !vault.has_active_position @ VaultError::PositionAlreadyExists,
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

    #[account(
        mut,
        constraint = pool_state.key() == vault.pool_id @ VaultError::InvalidPriceFeed,
    )]
    pub pool_state: AccountLoader<'info, PoolState>,

    #[account(mut)]
    pub position_nft_mint: Signer<'info>,

    /// CHECK: Will be initialized by Raydium
    #[account(mut)]
    pub position_nft_account: UncheckedAccount<'info>,

    /// CHECK: Will be initialized by Raydium
    #[account(mut)]
    pub personal_position: UncheckedAccount<'info>,

    /// CHECK: Validated by Raydium
    #[account(mut)]
    pub tick_array_lower: UncheckedAccount<'info>,

    /// CHECK: Validated by Raydium
    #[account(mut)]
    pub tick_array_upper: UncheckedAccount<'info>,

    #[account(mut)]
    pub token_vault_0: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(mut)]
    pub token_vault_1: Box<InterfaceAccount<'info, TokenAccount>>,

    pub vault_0_mint: Box<InterfaceAccount<'info, Mint>>,
    pub vault_1_mint: Box<InterfaceAccount<'info, Mint>>,

    /// CHECK: Validated by Raydium
    pub tick_array_bitmap: UncheckedAccount<'info>,

    /// CHECK: Validated by address constraint
    #[account(address = raydium_clmm_cpi::id())]
    pub clmm_program: UncheckedAccount<'info>,

    pub rent: Sysvar<'info, Rent>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub token_program_2022: Program<'info, Token2022>,
    pub associated_token_program: Program<'info, AssociatedToken>,
}

const MAX_SLIPPAGE_BPS: u16 = 500;

pub fn handler<'a, 'b, 'c: 'info, 'info>(
    ctx: Context<'a, 'b, 'c, 'info, OpenPosition<'info>>,
    tick_lower_index: i32,
    tick_upper_index: i32,
    tick_array_lower_start_index: i32,
    tick_array_upper_start_index: i32,
    liquidity: u128,
    amount_0_max: u64,
    amount_1_max: u64,
    slippage_bps: u16,
) -> Result<()> {
    require!(tick_lower_index < tick_upper_index, VaultError::InvalidTickRange);
    require!(liquidity > 0 || amount_0_max > 0, VaultError::InvalidAmount);
    require!(slippage_bps <= MAX_SLIPPAGE_BPS, VaultError::SlippageTooHigh);

    // ── M3: bound the position range (malicious-operator guardrail) ───────────
    {
        let pool = ctx.accounts.pool_state.load()?;
        validate_position_range(
            tick_lower_index,
            tick_upper_index,
            pool.tick_current,
            pool.tick_spacing as i32,
        )?;
    }

    let vault = &ctx.accounts.vault;

    require!(
        ctx.accounts.token0_treasury.amount >= amount_0_max,
        VaultError::InsufficientTreasuryBalance
    );
    require!(
        ctx.accounts.token1_treasury.amount >= amount_1_max,
        VaultError::InsufficientTreasuryBalance
    );

    let pool_id = vault.pool_id;
    let vault_seeds: &[&[&[u8]]] = &[&[seeds::VAULT, pool_id.as_ref(), &[vault.bump]]];

    let token0_before = ctx.accounts.token0_treasury.amount;
    let token1_before = ctx.accounts.token1_treasury.amount;

    let token0_treasury_seeds: &[&[u8]] = &[
        seeds::TOKEN0_TREASURY,
        &ctx.accounts.vault.key().to_bytes(),
        &[vault.token0_treasury_bump],
    ];
    // ── AUDIT NOTE (A2): delegate = `operator` here (vs `vault` in increase_liquidity). ──
    // This asymmetry is REQUIRED by Raydium, not a bug. `open_position_with_token22_nft`
    // takes a `payer` (= operator) which both funds the rent for the new NFT/position
    // accounts AND is the authority Raydium uses to pull token0/token1 out of the
    // treasuries. So the treasuries must `approve` the payer (operator) as SPL delegate.
    // `increase_liquidity_v2` has no `payer`; its transfer authority is `nft_owner`
    // (= vault), so there the delegate is the vault PDA.
    // SAFETY: the approve is bounded by amount_{0,1}_max and the matching `revoke`
    // below ALWAYS runs — on CPI success AND failure (see the no-`?` pattern) — so the
    // allowance is 0 after this instruction; operator never holds a standing delegation.
    // → Flagged for external-auditor review: confirm Raydium open requires payer-authority.
    anchor_spl::token_interface::approve(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token_interface::Approve {
                to: ctx.accounts.token0_treasury.to_account_info(),
                delegate: ctx.accounts.operator.to_account_info(),
                authority: ctx.accounts.token0_treasury.to_account_info(),
            },
            &[token0_treasury_seeds],
        ),
        amount_0_max,
    )?;

    let token1_treasury_seeds: &[&[u8]] = &[
        seeds::TOKEN1_TREASURY,
        &ctx.accounts.vault.key().to_bytes(),
        &[vault.token1_treasury_bump],
    ];
    anchor_spl::token_interface::approve(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token_interface::Approve {
                to: ctx.accounts.token1_treasury.to_account_info(),
                delegate: ctx.accounts.operator.to_account_info(),
                authority: ctx.accounts.token1_treasury.to_account_info(),
            },
            &[token1_treasury_seeds],
        ),
        amount_1_max,
    )?;

    let cpi_accounts = cpi::accounts::OpenPositionWithToken22Nft {
        payer: ctx.accounts.operator.to_account_info(),
        position_nft_owner: ctx.accounts.vault.to_account_info(),
        position_nft_mint: ctx.accounts.position_nft_mint.to_account_info(),
        position_nft_account: ctx.accounts.position_nft_account.to_account_info(),
        pool_state: ctx.accounts.pool_state.to_account_info(),
        protocol_position: ctx.accounts.personal_position.to_account_info(),
        tick_array_lower: ctx.accounts.tick_array_lower.to_account_info(),
        tick_array_upper: ctx.accounts.tick_array_upper.to_account_info(),
        personal_position: ctx.accounts.personal_position.to_account_info(),
        token_account_0: ctx.accounts.token0_treasury.to_account_info(),
        token_account_1: ctx.accounts.token1_treasury.to_account_info(),
        token_vault_0: ctx.accounts.token_vault_0.to_account_info(),
        token_vault_1: ctx.accounts.token_vault_1.to_account_info(),
        rent: ctx.accounts.rent.to_account_info(),
        system_program: ctx.accounts.system_program.to_account_info(),
        token_program: ctx.accounts.token_program.to_account_info(),
        associated_token_program: ctx.accounts.associated_token_program.to_account_info(),
        token_program_2022: ctx.accounts.token_program_2022.to_account_info(),
        vault_0_mint: ctx.accounts.vault_0_mint.to_account_info(),
        vault_1_mint: ctx.accounts.vault_1_mint.to_account_info(),
    };

    let cpi_ctx = CpiContext::new_with_signer(
        ctx.accounts.clmm_program.to_account_info(),
        cpi_accounts,
        vault_seeds,
    )
    .with_remaining_accounts(vec![ctx.accounts.tick_array_bitmap.to_account_info()]);

    // Save CPI result WITHOUT `?` — we must ALWAYS revoke even if CPI fails,
    // otherwise operator retains a live SPL delegate on the treasury accounts.
    let open_result = cpi::open_position_with_token22_nft(
        cpi_ctx,
        tick_lower_index,
        tick_upper_index,
        tick_array_lower_start_index,
        tick_array_upper_start_index,
        liquidity,
        amount_0_max,
        amount_1_max,
        true,
        Some(false),
    );

    // ALWAYS revoke — both on success and on failure.
    anchor_spl::token_interface::revoke(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token_interface::Revoke {
                source: ctx.accounts.token0_treasury.to_account_info(),
                authority: ctx.accounts.token0_treasury.to_account_info(),
            },
            &[token0_treasury_seeds],
        ),
    )?;
    anchor_spl::token_interface::revoke(
        CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            anchor_spl::token_interface::Revoke {
                source: ctx.accounts.token1_treasury.to_account_info(),
                authority: ctx.accounts.token1_treasury.to_account_info(),
            },
            &[token1_treasury_seeds],
        ),
    )?;

    // Now propagate the CPI result.
    open_result?;

    ctx.accounts.token0_treasury.reload()?;
    ctx.accounts.token1_treasury.reload()?;

    let token0_used = token0_before.saturating_sub(ctx.accounts.token0_treasury.amount);
    let token1_used = token1_before.saturating_sub(ctx.accounts.token1_treasury.amount);

    let vault = &mut ctx.accounts.vault;
    vault.has_active_position = true;
    vault.position_mint = ctx.accounts.position_nft_mint.key();
    vault.position_tick_lower = tick_lower_index;
    vault.position_tick_upper = tick_upper_index;
    let position_data = ctx.accounts.personal_position.try_borrow_data()?;
    let personal_pos = PersonalPositionState::try_deserialize(&mut &position_data[..])?;
    vault.position_liquidity = personal_pos.liquidity;
    vault.position_token0 = token0_used;
    vault.position_token1 = token1_used;
    vault.treasury_token0 = ctx.accounts.token0_treasury.amount;
    vault.treasury_token1 = ctx.accounts.token1_treasury.amount;
    vault.is_rebalancing = false;
    vault.rebalance_started_at = 0;

    emit!(PositionOpened {
        position_mint: ctx.accounts.position_nft_mint.key(),
        pool_id: vault.pool_id,
        tick_lower: tick_lower_index,
        tick_upper: tick_upper_index,
        liquidity: vault.position_liquidity,
        token0_used: vault.position_token0,
        token1_used: vault.position_token1,
    });

    Ok(())
}
