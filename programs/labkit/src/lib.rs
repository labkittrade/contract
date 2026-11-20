//! LabKit runs Raydium LaunchLab launches whose fees fund a kit.
//!
//! Setup, once per deployment: `initialize`, signed by the program upgrade authority,
//! sets the treasury, the fee splits, and the template for each launch's platform.
//!
//! Each launch:
//! 1. `create_launch`, signed by the base mint keypair, creates the launch's own LaunchLab
//!    platform from the template and fixes the kit and the fee splits. The platform's
//!    fee wallet is the kit PDA `["kit", base_mint]`. Its admin is a PDA LabKit never
//!    signs for again, so the platform can't change.
//! 2. LaunchLab `initialize_v2` on that platform with `creator` set to the PDA
//!    `["creator", base_mint]`, then `register_launch`, which checks the pool.
//!
//! The recipient can allocate the launch's locked supply with `create_creator_vesting`,
//! which signs LaunchLab `create_vesting_account` as the creator PDA.
//!
//! Anyone can then crank:
//! - `claim_kit_fees` claims the platform fees and splits them between the kit and the
//!   treasury.
//! - `claim_creator_fees` claims the curve's creator fees and splits them between the
//!   keeper and the recipient.
//! - `split_creator_fees` splits what sits in the creator PDA's WSOL account. After
//!   graduation that is Raydium creator fees, part of which goes to the kit and the
//!   treasury.
//! - `split_fees` sends referral fees in the fee wallet's ATA to the treasury.
//! - `execute_curve` and `execute_swap` spend the kit's SOL on buys and burn the tokens.
//!   Burn always buys, Floor only under a line below the highest price, and Graduate
//!   only on the curve.
//! - `checkpoint` records Floor's highest price.
//! - `release` pays a Graduate kit to the recipient once the curve has completed.
//!
//! Anyone opens a Stake launch's pool with `open_stake_pool`, once. Holders then
//! `stake` and `unstake` its token. The kit's SOL streams to
//! stakers pro rata, and each claims their share with `claim_stake_rewards`.
//!
//! A Draw launch's pool opens with `open_draw`, and holders stake the same way. Once a
//! day, when the pot is big enough, anyone sends `request_draw`, which asks ORAO VRF for
//! randomness and freezes stakes. `settle_draw` then credits the whole pot to a staker
//! picked at random, weighted by stake, among stakes older than a day. Winners claim
//! with `claim_stake_rewards`. `cancel_draw` drops a request ORAO never answered.
//!
//! There is no other way out of a LabKit PDA.

use anchor_lang::prelude::*;

mod constants;
mod cpmm;
mod draw_tree;
mod errors;
mod events;
mod instructions;
mod launchlab;
mod payout;
mod policy;
mod rewards;
mod state;
mod vrf;

pub use instructions::admin::{AcceptAuthority, Initialize, UpdateConfig};
pub use instructions::claim::ClaimKitFees;
pub use instructions::creator::{ClaimCreatorFees, SplitCreatorFees};
pub use instructions::draw::{CancelDraw, OpenDraw, RequestDraw, SettleDraw};
pub use instructions::kit::{Checkpoint, ExecuteCurve, ExecuteSwap, Release};
pub use instructions::launch::{CreateCreatorVesting, CreateLaunch, RegisterLaunch, SetLaunchRecipient};
pub use instructions::split::SplitFees;
pub use instructions::stake::{ClaimStakeRewards, OpenStakePool, Stake, Unstake};
pub use state::{ConfigParams, LaunchParams};

pub(crate) use instructions::admin::{
    __client_accounts_accept_authority, __client_accounts_initialize,
    __client_accounts_update_config,
};
pub(crate) use instructions::claim::__client_accounts_claim_kit_fees;
pub(crate) use instructions::creator::{
    __client_accounts_claim_creator_fees, __client_accounts_split_creator_fees,
};
pub(crate) use instructions::draw::{
    __client_accounts_cancel_draw, __client_accounts_open_draw, __client_accounts_request_draw,
    __client_accounts_settle_draw,
};
pub(crate) use instructions::kit::{
    __client_accounts_checkpoint, __client_accounts_execute_curve,
    __client_accounts_execute_swap, __client_accounts_release,
};
pub(crate) use instructions::launch::{
    __client_accounts_create_creator_vesting, __client_accounts_create_launch,
    __client_accounts_register_launch, __client_accounts_set_launch_recipient,
};
pub(crate) use instructions::split::__client_accounts_split_fees;
pub(crate) use instructions::stake::{
    __client_accounts_claim_stake_rewards, __client_accounts_open_stake_pool,
    __client_accounts_stake, __client_accounts_unstake,
};

declare_id!("133fUn4mjvLVKoe6oR7BWwAiF4qtFgmkg35BdegqgMG3");

// Contact and source shown by explorers. Left out of builds that only link the crate.
#[cfg(not(feature = "no-entrypoint"))]
solana_security_txt::security_txt! {
    name: "LabKit",
    project_url: "https://github.com/labkittrade/contract",
    contacts: "twitter:labkittrade",
    policy: "https://github.com/labkittrade/contract/blob/HEAD/SECURITY.md",
    source_code: "https://github.com/labkittrade/contract"
}

#[program]
pub mod labkit {
    use super::*;

    pub fn initialize(ctx: Context<Initialize>, params: ConfigParams) -> Result<()> {
        super::instructions::admin::initialize(ctx, params)
    }

    pub fn update_config(ctx: Context<UpdateConfig>, params: ConfigParams) -> Result<()> {
        super::instructions::admin::update(ctx, params)
    }

    pub fn transfer_authority(ctx: Context<UpdateConfig>, new_authority: Pubkey) -> Result<()> {
        super::instructions::admin::transfer_authority(ctx, new_authority)
    }

    pub fn accept_authority(ctx: Context<AcceptAuthority>) -> Result<()> {
        super::instructions::admin::accept_authority(ctx)
    }

    pub fn create_launch(ctx: Context<CreateLaunch>, params: LaunchParams) -> Result<()> {
        super::instructions::launch::create(ctx, params)
    }

    pub fn register_launch(ctx: Context<RegisterLaunch>) -> Result<()> {
        super::instructions::launch::register(ctx)
    }

    pub fn set_launch_recipient(ctx: Context<SetLaunchRecipient>, recipient: Pubkey) -> Result<()> {
        super::instructions::launch::set_recipient(ctx, recipient)
    }

    pub fn create_creator_vesting(
        ctx: Context<CreateCreatorVesting>,
        share_amount: u64,
    ) -> Result<()> {
        super::instructions::launch::create_vesting(ctx, share_amount)
    }

    pub fn claim_kit_fees(ctx: Context<ClaimKitFees>) -> Result<()> {
        super::instructions::claim::handler(ctx)
    }

    pub fn claim_creator_fees(ctx: Context<ClaimCreatorFees>) -> Result<()> {
        super::instructions::creator::claim(ctx)
    }

    pub fn split_creator_fees(ctx: Context<SplitCreatorFees>) -> Result<()> {
        super::instructions::creator::split_only(ctx)
    }

    pub fn split_fees(ctx: Context<SplitFees>) -> Result<()> {
        super::instructions::split::handler(ctx)
    }

    pub fn execute_curve(ctx: Context<ExecuteCurve>) -> Result<()> {
        super::instructions::kit::execute_curve(ctx)
    }

    pub fn execute_swap(ctx: Context<ExecuteSwap>) -> Result<()> {
        super::instructions::kit::execute_swap(ctx)
    }

    pub fn checkpoint<'info>(ctx: Context<'_, '_, '_, 'info, Checkpoint<'info>>) -> Result<()> {
        super::instructions::kit::checkpoint(ctx)
    }

    pub fn release(ctx: Context<Release>) -> Result<()> {
        super::instructions::kit::release(ctx)
    }

    pub fn open_stake_pool(ctx: Context<OpenStakePool>) -> Result<()> {
        super::instructions::stake::open_pool(ctx)
    }

    pub fn stake(ctx: Context<Stake>, amount: u64) -> Result<()> {
        super::instructions::stake::stake(ctx, amount)
    }

    pub fn unstake(ctx: Context<Unstake>, amount: u64) -> Result<()> {
        super::instructions::stake::unstake(ctx, amount)
    }

    pub fn claim_stake_rewards(ctx: Context<ClaimStakeRewards>) -> Result<()> {
        super::instructions::stake::claim_rewards(ctx)
    }

    pub fn open_draw(ctx: Context<OpenDraw>) -> Result<()> {
        super::instructions::draw::open(ctx)
    }

    pub fn request_draw(ctx: Context<RequestDraw>, seed: [u8; 32]) -> Result<()> {
        super::instructions::draw::request(ctx, seed)
    }

    pub fn settle_draw<'info>(ctx: Context<'_, '_, 'info, 'info, SettleDraw<'info>>) -> Result<()> {
        super::instructions::draw::settle(ctx)
    }

    pub fn cancel_draw(ctx: Context<CancelDraw>) -> Result<()> {
        super::instructions::draw::cancel(ctx)
    }
}
