use anchor_lang::prelude::*;

pub const CONFIG_SEED: &[u8] = b"config";
/// Receives referral fees that frontends route to LabKit. Never initialized; it only
/// owns the token accounts those fees land in.
pub const FEE_WALLET_SEED: &[u8] = b"fee_wallet";
pub const LAUNCH_SEED: &[u8] = b"launch";
/// `[CREATOR_SEED, base_mint]` is the LaunchLab `creator` of a launch. Never initialized.
pub const CREATOR_SEED: &[u8] = b"creator";
/// `[PLATFORM_ADMIN_SEED, base_mint]` is the admin of the launch's own LaunchLab platform.
/// LabKit never signs anything else as it, so the platform can't be changed.
pub const PLATFORM_ADMIN_SEED: &[u8] = b"platform_admin";
/// `[KIT_SEED, base_mint]` is the platform's fee wallet and holds the kit's SOL as WSOL.
/// Never initialized.
pub const KIT_SEED: &[u8] = b"kit";
/// `[STAKE_POOL_SEED, base_mint]` is a Stake launch's pool. It owns the staked tokens.
pub const STAKE_POOL_SEED: &[u8] = b"stake_pool";
/// `[STAKE_SEED, base_mint, owner]` is one wallet's stake in a launch.
pub const STAKE_SEED: &[u8] = b"stake";
/// `[DRAW_SEED, base_mint]` is a Draw launch's schedule and pending request.
pub const DRAW_SEED: &[u8] = b"draw";
/// `[DRAW_TREE_SEED, base_mint]` holds every Draw staker's stake in a Fenwick tree.
pub const DRAW_TREE_SEED: &[u8] = b"draw_tree";

pub const BPS_DENOMINATOR: u64 = 10_000;
/// Ceiling on the keeper's share of creator-side fees.
pub const MAX_CRANK_BPS: u16 = 500;

pub const KIT_BURN: u8 = 0;
pub const KIT_DRAW: u8 = 1;
pub const KIT_FLOOR: u8 = 2;
pub const KIT_GRADUATE: u8 = 3;
pub const KIT_STAKE: u8 = 4;

/// A Stake kit pays what it receives to stakers evenly over this window, restarted by
/// each new deposit, so staking just before a fee claim earns only a sliver of it.
pub const STAKE_STREAM_SECONDS: i64 = 3 * 24 * 60 * 60;

/// Least time between two draws. The `local-timing` feature is for local testing only.
#[cfg(not(feature = "local-timing"))]
pub const DRAW_INTERVAL_SECONDS: i64 = 24 * 60 * 60;
#[cfg(feature = "local-timing")]
pub const DRAW_INTERVAL_SECONDS: i64 = 20;
/// A stake added less than this long before a draw is requested can't win it, so
/// staking just before a draw doesn't pay.
#[cfg(not(feature = "local-timing"))]
pub const DRAW_MIN_STAKE_AGE_SECONDS: i64 = 24 * 60 * 60;
#[cfg(feature = "local-timing")]
pub const DRAW_MIN_STAKE_AGE_SECONDS: i64 = 10;
/// After this long without randomness, anyone may cancel the request and unfreeze stakes.
#[cfg(not(feature = "local-timing"))]
pub const DRAW_TIMEOUT_SECONDS: i64 = 60 * 60;
#[cfg(feature = "local-timing")]
pub const DRAW_TIMEOUT_SECONDS: i64 = 30;
/// A draw waits until the pot is at least this.
pub const DRAW_MIN_POT_LAMPORTS: u64 = 50_000_000;
/// Ceiling on what the pot refunds the keeper for one request.
pub const DRAW_MAX_COST_LAMPORTS: u64 = 10_000_000;
/// Picks drawn from one randomness before an all-ineligible draw rolls the pot over.
pub const DRAW_PICKS: usize = 8;

/// ORAO VRF, the same program on devnet and mainnet.
pub const ORAO_VRF_PROGRAM_ID: Pubkey = pubkey!("VRFzZoJdhFWL8rkvu87LpKM3RbcVezpMEc6X5GVDr7y");
pub const ORAO_NETWORK_STATE_SEED: &[u8] = b"orao-vrf-network-configuration";
pub const ORAO_RANDOMNESS_SEED: &[u8] = b"orao-vrf-randomness-request";
/// ORAO IDL `request_v2`
pub const ORAO_REQUEST_V2: [u8; 8] = [38, 151, 209, 6, 195, 102, 28, 217];
/// ORAO `RandomnessV2`: discriminator, then the `RequestAccount` enum. `Fulfilled` is
/// variant 1: client, seed, then the 64 bytes of randomness.
pub const ORAO_RANDOMNESS_V2_DISC: [u8; 8] = [139, 239, 184, 215, 227, 86, 191, 226];
pub const ORAO_FULFILLED_VARIANT: u8 = 1;
pub const ORAO_RANDOMNESS_OFFSET: usize = 8 + 1 + 32 + 32;
/// A fulfilled request shrinks to this size and returns the rest of its rent to the client,
/// which is the request's payer.
pub const ORAO_FULFILLED_LEN: usize = ORAO_RANDOMNESS_OFFSET + 64;

pub const MIN_DRAWDOWN_BPS: u16 = 100;
pub const MAX_DRAWDOWN_BPS: u16 = 9_000;
/// Floor's high rises at most this much per `HIGH_RISE_SECONDS` since it last rose, and
/// a longer wait earns no more, so a price pumped for one transaction can't set it.
pub const HIGH_RISE_BPS: u64 = 1_000;
#[cfg(not(feature = "local-timing"))]
pub const HIGH_RISE_SECONDS: i64 = 60 * 60;
#[cfg(feature = "local-timing")]
pub const HIGH_RISE_SECONDS: i64 = 2;
/// Smallest kit buy, unless a smaller one completes the curve.
pub const MIN_EXECUTE_LAMPORTS: u64 = 10_000_000;
/// One buy spends at most this share of the quote reserve. That keeps its price impact
/// near 2%, under the round-trip fees a sandwich around it would pay.
pub const MAX_SPEND_OF_RESERVE_BPS: u64 = 100;
/// Tolerance on top of fees for the minimum a kit buy must receive. The quote is taken in
/// the same instruction, so this only covers rounding.
pub const KIT_SLIPPAGE_BPS: u64 = 100;

pub const PLATFORM_NAME: &str = "LabKit";
pub const MAX_PLATFORM_WEB_LEN: usize = 64;
pub const MAX_PLATFORM_IMG_LEN: usize = 200;

pub const WSOL_MINT: Pubkey = pubkey!("So11111111111111111111111111111111111111112");

#[cfg(not(feature = "devnet"))]
pub const LAUNCHLAB_PROGRAM_ID: Pubkey = pubkey!("LanMV9sAd7wArD4vJFi2qDdfnVhFxYSUg6eADduJ3uj");
#[cfg(feature = "devnet")]
pub const LAUNCHLAB_PROGRAM_ID: Pubkey = pubkey!("DRay6fNdQ5J82H7xV6uq2aV3mNrUZ1J4PgSKsWgptcm6");

#[cfg(not(feature = "devnet"))]
pub const CPMM_PROGRAM_ID: Pubkey = pubkey!("CPMMoo8L3F4NbTegBCKVNunggL7H1ZpdTHKxQB5qKP1C");
#[cfg(feature = "devnet")]
pub const CPMM_PROGRAM_ID: Pubkey = pubkey!("DRaycpLY18LhpbydsBWbVJtxpNv9oXPgjRSfpF2bWpYb");

/// LaunchLab and CPMM rates use a 1_000_000 denominator.
pub const RATE_DENOMINATOR: u64 = 1_000_000;

/// LaunchLab IDL `claim_platform_fee_from_vault`
pub const CLAIM_PLATFORM_FEE_FROM_VAULT: [u8; 8] = [117, 241, 198, 168, 248, 218, 80, 29];
/// LaunchLab IDL `claim_creator_fee`
pub const CLAIM_CREATOR_FEE: [u8; 8] = [26, 97, 138, 203, 132, 171, 141, 252];
/// LaunchLab IDL `create_vesting_account`
pub const CREATE_VESTING_ACCOUNT: [u8; 8] = [129, 178, 2, 13, 217, 172, 230, 218];
/// LaunchLab IDL `create_platform_config`
pub const CREATE_PLATFORM_CONFIG: [u8; 8] = [176, 90, 196, 175, 253, 113, 220, 20];
/// LaunchLab IDL `buy_exact_in`
pub const BUY_EXACT_IN: [u8; 8] = [250, 234, 13, 123, 213, 156, 19, 236];
/// CPMM IDL `swap_base_input`
pub const SWAP_BASE_INPUT: [u8; 8] = [143, 190, 90, 218, 196, 30, 51, 222];

/// LaunchLab `VestingRecord`: discriminator, epoch, pool, beneficiary, claimed,
/// share, `[u64; 8]` padding. The creator pays its rent.
pub const VESTING_RECORD_SPACE: usize = 8 + 8 + 32 + 32 + 8 + 8 + 64;
/// LaunchLab `PlatformConfig` with its discriminator. The platform admin pays its rent.
pub const PLATFORM_CONFIG_SPACE: usize = 944;

/// LaunchLab `GlobalConfig`, Borsh layout.
pub const GLOBAL_TRADE_FEE_RATE_OFFSET: usize = 27;

/// LaunchLab `PlatformConfig`, Borsh layout.
pub const PLATFORM_CONFIG_DISC: [u8; 8] = [160, 78, 128, 0, 248, 83, 230, 160];
pub const PLATFORM_FEE_RATE_OFFSET: usize = 104;
pub const PLATFORM_CPSWAP_CONFIG_OFFSET: usize = 688;
pub const PLATFORM_CREATOR_FEE_RATE_OFFSET: usize = 720;

/// LaunchLab `PoolState`, Borsh layout. CPMM's `PoolState` shares the discriminator.
pub const POOL_STATE_DISC: [u8; 8] = [247, 237, 227, 245, 215, 195, 222, 70];
pub const POOL_STATUS_OFFSET: usize = 17;
pub const POOL_TOTAL_BASE_SELL_OFFSET: usize = 29;
pub const POOL_VIRTUAL_BASE_OFFSET: usize = 37;
pub const POOL_VIRTUAL_QUOTE_OFFSET: usize = 45;
pub const POOL_REAL_BASE_OFFSET: usize = 53;
pub const POOL_REAL_QUOTE_OFFSET: usize = 61;
pub const POOL_GLOBAL_CONFIG_OFFSET: usize = 141;
pub const POOL_PLATFORM_CONFIG_OFFSET: usize = 173;
pub const POOL_BASE_MINT_OFFSET: usize = 205;
pub const POOL_QUOTE_MINT_OFFSET: usize = 237;
pub const POOL_CREATOR_OFFSET: usize = 333;
/// Bit 0 set means the base mint is Token-2022.
pub const POOL_TOKEN_PROGRAM_FLAG_OFFSET: usize = 365;
/// `AmmCreatorFeeOn`: 0 takes Raydium creator fees in the quote token only.
pub const POOL_AMM_CREATOR_FEE_ON_OFFSET: usize = 366;
pub const AMM_CREATOR_FEE_ON_QUOTE: u8 = 0;
/// `PoolState.status` once trading moves to CPMM.
pub const POOL_STATUS_TRADING: u8 = 0;
pub const POOL_STATUS_MIGRATED: u8 = 2;

/// CPMM accounts are zero-copy and packed (raydium-cp-swap `states`).
pub const CPMM_POOL_LEN: usize = 637;
pub const CPMM_POOL_AMM_CONFIG_OFFSET: usize = 8;
pub const CPMM_POOL_CREATOR_OFFSET: usize = 40;
pub const CPMM_POOL_VAULT_0_OFFSET: usize = 72;
pub const CPMM_POOL_VAULT_1_OFFSET: usize = 104;
pub const CPMM_POOL_MINT_0_OFFSET: usize = 168;
pub const CPMM_POOL_MINT_1_OFFSET: usize = 200;
pub const CPMM_POOL_OBSERVATION_OFFSET: usize = 296;
pub const CPMM_POOL_PROTOCOL_FEES_0_OFFSET: usize = 341;
pub const CPMM_POOL_FUND_FEES_0_OFFSET: usize = 357;
pub const CPMM_POOL_ENABLE_CREATOR_FEE_OFFSET: usize = 390;
pub const CPMM_POOL_CREATOR_FEES_0_OFFSET: usize = 397;
pub const CPMM_CONFIG_TRADE_FEE_RATE_OFFSET: usize = 12;
pub const CPMM_CONFIG_CREATOR_FEE_RATE_OFFSET: usize = 108;
