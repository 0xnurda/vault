use anchor_lang::prelude::*;

pub mod state;
pub mod instructions;
pub mod errors;
pub mod events;

use instructions::*;

declare_id!("6wktAqahNmWdF14B4UQYam7bskj1fUcMQQXaE2jmTYNz");

#[program]
pub mod vault {
    use super::*;

    // ============ ADMIN INSTRUCTIONS ============

    /// Initialize vault with treasury PDAs and share mint
    pub fn initialize(ctx: Context<Initialize>) -> Result<()> {
        instructions::initialize::handler(ctx)
    }

    /// Pause or unpause the vault
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

    /// Update TVL (called by backend periodically)
    pub fn update_tvl(ctx: Context<UpdateTvl>, tvl_usd: u64, sol_price: u64) -> Result<()> {
        instructions::update_tvl::handler(ctx, tvl_usd, sol_price)
    }

    /// Withdraw funds from treasury to admin wallet for Raydium management
    pub fn withdraw_to_manage(
        ctx: Context<WithdrawToManage>,
        sol_amount: u64,
        usdc_amount: u64,
    ) -> Result<()> {
        instructions::withdraw_to_manage::handler(ctx, sol_amount, usdc_amount)
    }

    /// Return funds to treasury after rebalance
    pub fn return_from_manage(
        ctx: Context<ReturnFromManage>,
        sol_amount: u64,
        usdc_amount: u64,
    ) -> Result<()> {
        instructions::return_from_manage::handler(ctx, sol_amount, usdc_amount)
    }

    /// Swap tokens within treasury via Raydium CLMM CPI
    /// This allows rebalancing without moving funds to admin wallet
    pub fn swap_in_treasury<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, SwapInTreasury<'info>>,
        amount_in: u64,
        minimum_amount_out: u64,
        direction: SwapDirection,
    ) -> Result<()> {
        instructions::swap_in_treasury::handler(ctx, amount_in, minimum_amount_out, direction)
    }

    // ============ POSITION MANAGEMENT ============

    /// Open a new CLMM position with funds from treasury
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

    /// Close the active CLMM position and return funds to treasury
    pub fn close_position(ctx: Context<ClosePosition>, amount_0_min: u64, amount_1_min: u64) -> Result<()> {
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
    pub fn decrease_liquidity(
        ctx: Context<DecreaseLiquidity>,
        liquidity: u128,
        amount_0_min: u64,
        amount_1_min: u64,
    ) -> Result<()> {
        instructions::decrease_liquidity::handler(ctx, liquidity, amount_0_min, amount_1_min)
    }

    /// Collect accumulated trading fees from the position
    pub fn collect_fees(ctx: Context<CollectFees>) -> Result<()> {
        instructions::collect_fees::handler(ctx)
    }

    // ============ USER INSTRUCTIONS ============

    /// Deposit SOL into vault
    pub fn deposit_sol(ctx: Context<DepositSol>, amount: u64) -> Result<()> {
        instructions::deposit_sol::handler(ctx, amount)
    }

    /// Deposit USDC into vault
    pub fn deposit_usdc(ctx: Context<DepositUsdc>, amount: u64) -> Result<()> {
        instructions::deposit_usdc::handler(ctx, amount)
    }

    /// Full withdrawal from vault (burn ALL shares, receive SOL/USDC)
    pub fn withdraw(ctx: Context<Withdraw>) -> Result<()> {
        instructions::withdraw::handler(ctx)
    }
}
