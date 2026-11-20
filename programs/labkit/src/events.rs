use anchor_lang::prelude::*;

use crate::state::FeeSplit;

#[event]
pub struct ConfigUpdated {
    pub authority: Pubkey,
    pub treasury: Pubkey,
    pub fees: FeeSplit,
}

#[event]
pub struct AuthorityTransferred {
    pub previous: Pubkey,
    pub authority: Pubkey,
}

/// Referral fees moved from the fee wallet to the treasury.
#[event]
pub struct FeesSplit {
    pub mint: Pubkey,
    pub treasury_amount: u64,
    pub slot: u64,
}

#[event]
pub struct LaunchCreated {
    pub base_mint: Pubkey,
    pub platform_config: Pubkey,
    pub recipient: Pubkey,
    pub kit: u8,
    pub drawdown_bps: u16,
    pub fees: FeeSplit,
}

#[event]
pub struct LaunchRegistered {
    pub base_mint: Pubkey,
    pub pool_state: Pubkey,
}

#[event]
pub struct LaunchRecipientChanged {
    pub base_mint: Pubkey,
    pub previous: Pubkey,
    pub recipient: Pubkey,
}

#[event]
pub struct CreatorVestingCreated {
    pub base_mint: Pubkey,
    pub beneficiary: Pubkey,
    pub share_amount: u64,
}

/// A launch's platform fees, claimed and split between its kit and the treasury.
#[event]
pub struct KitFeesClaimed {
    pub base_mint: Pubkey,
    pub claimed: u64,
    pub kit_amount: u64,
    pub treasury_amount: u64,
    pub crank: Pubkey,
    pub slot: u64,
}

#[event]
pub struct CreatorFeesSplit {
    pub base_mint: Pubkey,
    /// Claimed from LaunchLab in this instruction. Zero for a plain `split_creator_fees`.
    pub claimed: u64,
    pub recipient_amount: u64,
    pub crank_amount: u64,
    /// Nonzero only for Raydium creator fees, after graduation.
    pub kit_amount: u64,
    pub treasury_amount: u64,
    pub crank: Pubkey,
    pub slot: u64,
}

/// 0 is the bonding curve, 1 is Raydium CPMM.
#[event]
pub struct KitExecuted {
    pub base_mint: Pubkey,
    pub kit: u8,
    pub venue: u8,
    pub lamports: u64,
    pub burned: u64,
    pub price_q64: u128,
    pub crank: Pubkey,
    pub slot: u64,
}

#[event]
pub struct FloorCheckpoint {
    pub base_mint: Pubkey,
    pub price_q64: u128,
    pub high_q64: u128,
    pub slot: u64,
}

#[event]
pub struct Staked {
    pub base_mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
    /// The owner's stake afterwards.
    pub position: u64,
    pub total_staked: u64,
    pub slot: u64,
}

#[event]
pub struct Unstaked {
    pub base_mint: Pubkey,
    pub owner: Pubkey,
    pub amount: u64,
    /// The owner's stake afterwards.
    pub position: u64,
    pub total_staked: u64,
    pub slot: u64,
}

#[event]
pub struct StakeRewardsClaimed {
    pub base_mint: Pubkey,
    pub owner: Pubkey,
    pub lamports: u64,
    pub slot: u64,
}

#[event]
pub struct DrawRequested {
    pub base_mint: Pubkey,
    pub seed: [u8; 32],
    /// Lamports the pot refunded the keeper for the VRF fee and rent.
    pub cost: u64,
    pub slot: u64,
}

#[event]
pub struct DrawSettled {
    pub base_mint: Pubkey,
    pub round: u64,
    /// Default when every pick was ineligible and the pot rolled over.
    pub winner: Pubkey,
    pub prize: u64,
    pub slot: u64,
}

#[event]
pub struct DrawCancelled {
    pub base_mint: Pubkey,
    pub seed: [u8; 32],
    pub slot: u64,
}

#[event]
pub struct GraduateReleased {
    pub base_mint: Pubkey,
    pub recipient: Pubkey,
    pub lamports: u64,
    pub slot: u64,
}
