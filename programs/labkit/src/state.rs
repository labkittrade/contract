use anchor_lang::prelude::*;

#[account]
#[derive(InitSpace)]
pub struct Config {
    pub bump: u8,
    pub fee_wallet_bump: u8,
    pub authority: Pubkey,
    /// Set by `transfer_authority`, cleared by `accept_authority`. Default means none.
    pub pending_authority: Pubkey,
    /// Wallet whose token accounts receive the treasury share.
    pub treasury: Pubkey,
    /// Copied into each launch when it is created.
    pub fees: FeeSplit,
    /// The LaunchLab platform each new launch gets.
    pub platform: PlatformTemplate,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, InitSpace)]
pub struct FeeSplit {
    /// Keeper share of creator-side fees, in bps. The recipient gets the rest.
    pub crank_bps: u16,
    /// Kit share of platform-side fees, in bps. The treasury gets the rest.
    pub kit_bps: u16,
    /// Share of Raydium creator fees that counts as creator-side after graduation, in
    /// bps. The rest counts as platform-side.
    pub amm_creator_bps: u16,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, InitSpace)]
pub struct PlatformTemplate {
    /// Platform fee on every curve trade, on LaunchLab's 1_000_000 denominator.
    pub fee_rate: u64,
    /// Creator fee on every curve trade, on LaunchLab's 1_000_000 denominator.
    pub creator_fee_rate: u64,
    /// Share of graduated liquidity locked to `nft_wallet`, on LaunchLab's denominator.
    /// The rest is burned.
    pub lp_lock_scale: u64,
    pub nft_wallet: Pubkey,
    /// The CPMM `AmmConfig` launches graduate into. Its creator fee applies after graduation.
    pub cpswap_config: Pubkey,
    #[max_len(64)]
    pub web: String,
    #[max_len(200)]
    pub img: String,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct ConfigParams {
    pub treasury: Pubkey,
    pub fees: FeeSplit,
    pub platform: PlatformTemplate,
}

/// A launch whose LaunchLab `creator` is `[CREATOR_SEED, base_mint]` and whose platform
/// is its own, administered by `[PLATFORM_ADMIN_SEED, base_mint]`.
#[account]
#[derive(InitSpace)]
pub struct Launch {
    pub bump: u8,
    pub creator_bump: u8,
    pub kit_bump: u8,
    pub admin_bump: u8,
    /// Chosen at creation and never changed.
    pub kit: u8,
    /// Set once the LaunchLab pool exists on the launch's platform.
    pub registered: bool,
    /// Floor only: how far under the high the price must fall before the kit buys.
    pub drawdown_bps: u16,
    /// Fixed at creation so a config change never reprices an existing launch.
    pub fees: FeeSplit,
    pub base_mint: Pubkey,
    pub pool_state: Pubkey,
    pub platform_config: Pubkey,
    /// Receives the creator's share. Only this wallet can change it.
    pub recipient: Pubkey,
    /// Floor only: the highest price seen, in lamports per raw token, Q64.64. It starts at
    /// the curve's opening price and rises no faster than `HIGH_RISE_BPS` allows.
    pub high_q64: u128,
    /// Floor only: when `high_q64` last rose, in Unix seconds.
    pub high_at: i64,
    /// Lamports routed to the kit.
    pub kit_funded: u64,
    /// Lamports the kit spent on buys.
    pub spent: u64,
    /// Raw tokens the kit bought and burned.
    pub burned: u64,
    /// Graduate only: lamports paid to the recipient after the curve completed.
    pub released: u64,
    pub executions: u64,
    pub last_execution_slot: u64,
}

/// A Stake launch's pool. Its token account `ATA(base_mint, pool)` holds the staked
/// tokens. Rewards stay in the kit's WSOL account until claimed.
#[account]
#[derive(InitSpace)]
pub struct StakePool {
    pub bump: u8,
    pub base_mint: Pubkey,
    /// Raw tokens staked.
    pub total_staked: u64,
    /// Lamports streamed per staked raw token since the pool opened, Q64.64.
    pub reward_per_token_q64: u128,
    /// Lamports received and not yet streamed.
    pub streaming: u64,
    /// When `streaming` is fully paid out, in Unix seconds.
    pub stream_end: i64,
    pub last_update: i64,
    /// Lamports streamed to stakers.
    pub distributed: u64,
    /// Lamports stakers have claimed.
    pub claimed: u64,
}

/// One wallet's stake in a launch.
#[account]
#[derive(InitSpace)]
pub struct StakePosition {
    pub bump: u8,
    pub owner: Pubkey,
    pub base_mint: Pubkey,
    /// Raw tokens staked.
    pub amount: u64,
    /// The pool's `reward_per_token_q64` when `earned` was last brought up to date.
    pub reward_per_token_q64: u128,
    /// Lamports earned and not yet claimed. Draw: prizes won.
    pub earned: u64,
    /// Lamports claimed.
    pub claimed: u64,
    /// Draw only: the position's 1-based index in the draw tree, zero until the first stake.
    pub slot: u32,
    /// When the stake last grew, in Unix seconds.
    pub staked_at: i64,
}

/// A Draw launch's schedule. Its stakers are in the launch's `StakePool`, and their stakes
/// in the `DrawTree`, so a random point in the total stake finds its owner in O(log n).
#[account]
#[derive(InitSpace)]
pub struct Draw {
    pub bump: u8,
    pub base_mint: Pubkey,
    /// Draws settled, won or rolled over.
    pub round: u64,
    /// Set from the VRF request until it is settled or cancelled. Stakes are frozen meanwhile.
    pub pending: bool,
    /// The ORAO request seed of the pending or last draw.
    pub seed: [u8; 32],
    pub requested_at: i64,
    /// When the last draw was settled, or the pool opened.
    pub last_draw: i64,
    pub last_winner: Pubkey,
    pub last_prize: u64,
    /// Lamports won in all draws.
    pub won: u64,
}

/// Header of the draw tree account. Fenwick entries follow it as little-endian `u64`s,
/// one per staker, and the account grows by one entry for each new staker.
#[account]
#[derive(InitSpace)]
pub struct DrawTree {
    pub bump: u8,
    /// Set while a draw waits for randomness.
    pub frozen: bool,
    /// Entries in the tree.
    pub len: u32,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct LaunchParams {
    pub recipient: Pubkey,
    pub kit: u8,
    /// Floor only, zero otherwise.
    pub drawdown_bps: u16,
}
