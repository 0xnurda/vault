use anchor_lang::prelude::*;

pub mod state;
pub mod instructions;
pub mod errors;
pub mod events;
pub mod constants;

use instructions::*;

declare_id!("DSNXFCkn4y6FSzfDmkZhNp3r23KdCvZ1fQMKRVXXZEMy");

#[program]
pub mod vault {
    use super::*;

    // ============ ADMIN INSTRUCTIONS ============

    /// Initialize vault with treasury PDAs, share mint, protocol wallet, and Pyth price feed
    pub fn initialize(
        ctx: Context<Initialize>,
        protocol_wallet: Pubkey,
        sol_price_feed: Pubkey,
    ) -> Result<()> {
        instructions::initialize::handler(ctx, protocol_wallet, sol_price_feed)
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

    /// Extract accumulated protocol fees (10% of collected fees) to protocol_wallet
    pub fn extract_protocol_fee(ctx: Context<ExtractProtocolFee>) -> Result<()> {
        instructions::extract_protocol_fee::handler(ctx)
    }

    /// Emergency: cancel a stuck rebalance (if open_position fails after close_position).
    /// Resets is_rebalancing = false so users can withdraw. Admin only.
    pub fn cancel_rebalance(ctx: Context<CancelRebalance>) -> Result<()> {
        instructions::cancel_rebalance::handler(ctx)
    }

    /// One-time migration: upgrades vault account layout after program upgrade.
    /// Reallocs to Vault::LEN, preserves discriminator, sources state from
    /// actual on-chain token accounts. Admin only.
    pub fn migrate_vault(
        ctx: Context<MigrateVault>,
        protocol_wallet: Pubkey,
        sol_price_feed: Pubkey,
    ) -> Result<()> {
        instructions::migrate_vault::handler(ctx, protocol_wallet, sol_price_feed)
    }

    /// Set Raydium CLMM pool as SOL price source (admin-only, called once after upgrade).
    /// raydium_pool: SOL/USDC Raydium CLMM pool address
    pub fn sync_position_value(ctx: Context<SyncPositionValue>) -> Result<()> {
        instructions::sync_position_value::sync_handler(ctx)
    }

    /// Update price feed address stored in vault
    pub fn update_price(ctx: Context<UpdatePrice>, raydium_pool: Pubkey) -> Result<()> {
        instructions::update_price::handler(ctx, raydium_pool)
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
    /// Also clears is_rebalancing flag (end of rebalance cycle).
    pub fn open_position<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, OpenPosition<'info>>,
        tick_lower_index: i32,
        tick_upper_index: i32,
        tick_array_lower_start_index: i32,
        tick_array_upper_start_index: i32,
        liquidity: u128,
        amount_0_max: u64,
        amount_1_max: u64,
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
        )
    }

    /// Close the active CLMM position and return funds to treasury.
    /// Sets is_rebalancing = true (blocks user deposits/withdrawals until open_position).
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
    ) -> Result<()> {
        instructions::increase_liquidity::handler(ctx, liquidity, amount_0_max, amount_1_max)
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
    /// 10% of fees → accumulated_protocol_fees. 90% stays in treasury (user profit).
    pub fn collect_fees(ctx: Context<CollectFees>) -> Result<()> {
        instructions::collect_fees::handler(ctx)
    }

    // ============ USER INSTRUCTIONS ============

    /// Deposit SOL into vault (price read live from Raydium pool)
    pub fn deposit_sol(ctx: Context<DepositSol>, amount: u64) -> Result<()> {
        instructions::deposit_sol::handler(ctx, amount)
    }

    /// Deposit USDC into vault (price read live from Raydium pool)
    pub fn deposit_usdc(ctx: Context<DepositUsdc>, amount: u64) -> Result<()> {
        instructions::deposit_usdc::handler(ctx, amount)
    }

    /// Full withdrawal from vault (burn ALL shares, receive SOL/USDC pro-rata)
    pub fn withdraw(ctx: Context<Withdraw>) -> Result<()> {
        instructions::withdraw::handler(ctx)
    }
}
