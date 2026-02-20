use anchor_lang::prelude::*;

pub mod errors;
pub mod events;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("CikLi2FgfnAoDDepVRe8WA7SsEHvpaJeZv5WpbvvQKCw");

#[program]
pub mod smart_wallet {
    use super::*;

    // ============ USER INSTRUCTIONS ============

    /// Create a new smart wallet with personal treasury accounts
    pub fn create_wallet(ctx: Context<CreateWallet>) -> Result<()> {
        instructions::create_wallet::handler(ctx)
    }

    /// Set or remove delegate (backend operator)
    pub fn set_delegate(ctx: Context<SetDelegate>, new_delegate: Pubkey) -> Result<()> {
        instructions::set_delegate::handler(ctx, new_delegate)
    }

    /// Withdraw SOL or USDC from treasury back to user (owner only)
    pub fn withdraw(ctx: Context<Withdraw>, amount: u64, is_sol: bool) -> Result<()> {
        instructions::withdraw::handler(ctx, amount, is_sol)
    }

    /// Close smart wallet and return rent to owner (owner only)
    pub fn close_wallet(ctx: Context<CloseWallet>) -> Result<()> {
        instructions::close_wallet::handler(ctx)
    }

    /// Pause or unpause the wallet (owner only)
    pub fn set_paused(ctx: Context<SetPaused>, paused: bool) -> Result<()> {
        instructions::set_paused::handler(ctx, paused)
    }

    // ============ AUTOMATED MANAGEMENT (delegate or owner) ============

    /// Pull tokens from user's account into treasury via approve/delegate
    pub fn fund_treasury(ctx: Context<FundTreasury>, amount: u64, is_sol: bool) -> Result<()> {
        instructions::deposit::handler(ctx, amount, is_sol)
    }

    /// Swap SOL/USDC within treasury for rebalancing
    pub fn swap_in_treasury<'a, 'b, 'c: 'info, 'info>(
        ctx: Context<'a, 'b, 'c, 'info, SwapInTreasury<'info>>,
        amount_in: u64,
        minimum_amount_out: u64,
        direction: SwapDirection,
    ) -> Result<()> {
        instructions::swap_in_treasury::handler(ctx, amount_in, minimum_amount_out, direction)
    }

    /// Open a new Raydium CLMM position
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

    /// Close the active CLMM position
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

    /// Decrease liquidity in the active position
    pub fn decrease_liquidity(
        ctx: Context<DecreaseLiquidity>,
        liquidity: u128,
        amount_0_min: u64,
        amount_1_min: u64,
    ) -> Result<()> {
        instructions::decrease_liquidity::handler(ctx, liquidity, amount_0_min, amount_1_min)
    }

    /// Collect accumulated trading fees
    pub fn collect_fees(ctx: Context<CollectFees>) -> Result<()> {
        instructions::collect_fees::handler(ctx)
    }
}
