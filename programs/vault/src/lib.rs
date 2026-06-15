use anchor_lang::prelude::*;

pub mod state;
pub mod instructions;
pub mod errors;
pub mod events;
pub mod constants;

use instructions::*;

declare_id!("6Cu6y74sFv2R2qnHS4wHAPCVcnSxYMQRCZ84chG2HRr9");

#[program]
pub mod vault {
    use super::*;

    // ============ ADMIN INSTRUCTIONS ============

    /// Initialize vault with treasury PDAs, share mint, admin, protocol wallet.
    /// pool: Raydium CLMM pool whose key becomes vault.pool_id (immutable PDA seed).
    /// admin: vault admin pubkey — separate from payer (deployer key) so you can
    ///        deploy from one cold keypair and operate from a different hot/multisig key.
    pub fn initialize(
        ctx: Context<Initialize>,
        admin: Pubkey,
        protocol_wallet: Pubkey,
    ) -> Result<()> {
        instructions::initialize::handler(ctx, admin, protocol_wallet)
    }

    /// Pause or unpause the vault (user deposits/withdrawals)
    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        instructions::set_paused::handler(ctx, paused)
    }

    /// Step 1: Propose a new admin (current admin only)
    pub fn transfer_admin(ctx: Context<TransferAdmin>, new_admin: Pubkey) -> Result<()> {
        instructions::transfer_admin::handler(ctx, new_admin)
    }

    /// Step 2: New admin accepts the transfer
    pub fn accept_admin(ctx: Context<AcceptAdmin>) -> Result<()> {
        instructions::accept_admin::handler(ctx)
    }

    /// Set the hot operator key for automated ops (admin only).
    pub fn set_operator(ctx: Context<SetOperator>, new_operator: Pubkey) -> Result<()> {
        instructions::set_operator::handler(ctx, new_operator)
    }

    /// Extract accumulated protocol fees (10% of collected fees) to protocol_wallet
    pub fn extract_protocol_fee(ctx: Context<ExtractProtocolFee>) -> Result<()> {
        instructions::extract_protocol_fee::handler(ctx)
    }

    /// Sweep claimed LM reward tokens to protocol_wallet (audit M-4).
    pub fn extract_rewards(ctx: Context<ExtractRewards>) -> Result<()> {
        instructions::extract_rewards::handler(ctx)
    }

    /// Emergency: cancel a stuck rebalance (if open_position fails after close_position).
    pub fn cancel_rebalance(ctx: Context<CancelRebalance>) -> Result<()> {
        instructions::cancel_rebalance::handler(ctx)
    }

    /// One-time migration: upgrades vault account layout after program upgrade.
    /// Decimals are now read from mint accounts — no longer accepted as args (audit #3).
    pub fn migrate_vault(
        ctx: Context<MigrateVault>,
        protocol_wallet: Pubkey,
    ) -> Result<()> {
        instructions::migrate_vault::handler(ctx, protocol_wallet)
    }

    /// Sync vault.position_token0/token1 with real on-chain CLMM amounts.
    pub fn sync_position_value(ctx: Context<SyncPositionValue>) -> Result<()> {
        instructions::sync_position_value::sync_handler(ctx)
    }

    /// Swap tokens within treasury via Raydium CLMM CPI (for rebalancing)
    pub fn swap_in_treasury<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, SwapInTreasury<'info>>,
        amount_in: u64,
        minimum_amount_out: u64,
        direction: SwapDirection,
    ) -> Result<()> {
        instructions::swap_in_treasury::handler(ctx, amount_in, minimum_amount_out, direction)
    }

    // ============ POSITION MANAGEMENT ============

    /// Open a new CLMM position with funds from treasury.
    pub fn open_position<'a, 'b, 'c: 'info, 'info>(
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
        instructions::open_position::handler(
            ctx,
            tick_lower_index,
            tick_upper_index,
            tick_array_lower_start_index,
            tick_array_upper_start_index,
            liquidity,
            amount_0_max,
            amount_1_max,
            slippage_bps,
        )
    }

    /// Close the active CLMM position and return funds to treasury.
    pub fn close_position<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, ClosePosition<'info>>,
        amount_0_min: u64,
        amount_1_min: u64,
    ) -> Result<()> {
        instructions::close_position::handler(ctx, amount_0_min, amount_1_min)
    }

    /// Increase liquidity in the active position
    pub fn increase_liquidity(
        ctx: Context<IncreaseLiquidity>,
        liquidity: u128,
        amount_0_max: u64,
        amount_1_max: u64,
        slippage_bps: u16,
    ) -> Result<()> {
        instructions::increase_liquidity::handler(ctx, liquidity, amount_0_max, amount_1_max, slippage_bps)
    }

    /// Decrease liquidity from the active position
    pub fn decrease_liquidity<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, DecreaseLiquidity<'info>>,
        liquidity: u128,
        amount_0_min: u64,
        amount_1_min: u64,
    ) -> Result<()> {
        instructions::decrease_liquidity::handler(ctx, liquidity, amount_0_min, amount_1_min)
    }

    /// Collect accumulated trading fees from the position.
    pub fn collect_fees<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, CollectFees<'info>>,
    ) -> Result<()> {
        instructions::collect_fees::handler(ctx)
    }

    // ============ USER INSTRUCTIONS ============

    /// Deposit token0 into vault (price read live from Raydium pool).
    /// min_shares_out: revert if fewer shares would be minted (deposit slippage, H-3).
    pub fn deposit_token0(ctx: Context<DepositToken0>, amount: u64, min_shares_out: u64) -> Result<()> {
        instructions::deposit_token0::handler(ctx, amount, min_shares_out)
    }

    /// Deposit token1 into vault (price read live from Raydium pool).
    /// min_shares_out: revert if fewer shares would be minted (deposit slippage, H-3).
    pub fn deposit_token1(ctx: Context<DepositToken1>, amount: u64, min_shares_out: u64) -> Result<()> {
        instructions::deposit_token1::handler(ctx, amount, min_shares_out)
    }

    /// Full withdrawal from treasury (burn ALL shares, receive token0/token1 pro-rata).
    /// Emergency mode: if `is_rebalancing` has been true for > 3600s, users may
    /// still withdraw from whatever is in the treasury.
    pub fn withdraw(ctx: Context<Withdraw>, min_token0_out: u64, min_token1_out: u64) -> Result<()> {
        instructions::withdraw::handler(ctx, min_token0_out, min_token1_out)
    }

    /// Atomic withdrawal when a Raydium position is active.
    /// Kamino-style two-CPI flow:
    ///   1. Collect uncollected fees (10% → protocol, 90% → TVL).
    ///   2. Remove user's pro-rata liquidity and transfer all proceeds.
    pub fn withdraw_from_position<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, WithdrawFromPosition<'info>>,
        min_token0_out: u64,
        min_token1_out: u64,
        shares_to_withdraw: u64,
    ) -> Result<()> {
        instructions::withdraw_from_position::handler(ctx, min_token0_out, min_token1_out, shares_to_withdraw)
    }
}
