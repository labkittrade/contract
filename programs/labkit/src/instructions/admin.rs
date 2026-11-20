use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::LabkitError;
use crate::events::{AuthorityTransferred, ConfigUpdated};
use crate::program::Labkit;
use crate::state::{Config, ConfigParams};

pub fn initialize(ctx: Context<Initialize>, params: ConfigParams) -> Result<()> {
    let config = &mut ctx.accounts.config;
    config.bump = ctx.bumps.config;
    config.fee_wallet_bump = ctx.bumps.fee_wallet;
    config.authority = ctx.accounts.authority.key();
    config.pending_authority = Pubkey::default();
    apply(config, params)
}

pub fn update(ctx: Context<UpdateConfig>, params: ConfigParams) -> Result<()> {
    apply(&mut ctx.accounts.config, params)
}

/// Pass the default public key to cancel a pending transfer.
pub fn transfer_authority(ctx: Context<UpdateConfig>, new_authority: Pubkey) -> Result<()> {
    ctx.accounts.config.pending_authority = new_authority;
    Ok(())
}

pub fn accept_authority(ctx: Context<AcceptAuthority>) -> Result<()> {
    let config = &mut ctx.accounts.config;
    let previous = config.authority;
    config.authority = ctx.accounts.new_authority.key();
    config.pending_authority = Pubkey::default();
    emit!(AuthorityTransferred {
        previous,
        authority: config.authority,
    });
    Ok(())
}

fn apply(config: &mut Config, params: ConfigParams) -> Result<()> {
    require!(params.treasury != Pubkey::default(), LabkitError::InvalidTreasury);
    let fees = params.fees;
    require!(fees.crank_bps <= MAX_CRANK_BPS, LabkitError::CrankFeeTooHigh);
    require!(
        fees.kit_bps as u64 <= BPS_DENOMINATOR && fees.amm_creator_bps as u64 <= BPS_DENOMINATOR,
        LabkitError::InvalidBps
    );
    let platform = &params.platform;
    require!(
        platform.lp_lock_scale <= RATE_DENOMINATOR
            && platform.fee_rate < RATE_DENOMINATOR
            && platform.creator_fee_rate < RATE_DENOMINATOR
            && platform.web.len() <= MAX_PLATFORM_WEB_LEN
            && platform.img.len() <= MAX_PLATFORM_IMG_LEN
            && platform.nft_wallet != Pubkey::default()
            && platform.cpswap_config != Pubkey::default(),
        LabkitError::InvalidPlatform
    );
    config.treasury = params.treasury;
    config.fees = fees;
    config.platform = params.platform;
    emit!(ConfigUpdated {
        authority: config.authority,
        treasury: config.treasury,
        fees,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(mut)]
    pub authority: Signer<'info>,

    #[account(
        init,
        payer = authority,
        space = 8 + Config::INIT_SPACE,
        seeds = [CONFIG_SEED],
        bump
    )]
    pub config: Account<'info, Config>,

    /// CHECK: PDA that LaunchLab knows as `platform_fee_wallet`. Never initialized.
    #[account(seeds = [FEE_WALLET_SEED], bump)]
    pub fee_wallet: UncheckedAccount<'info>,

    #[account(constraint = program.programdata_address()? == Some(program_data.key()))]
    pub program: Program<'info, Labkit>,

    #[account(
        constraint = program_data.upgrade_authority_address == Some(authority.key())
            @ LabkitError::NotUpgradeAuthority
    )]
    pub program_data: Account<'info, ProgramData>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UpdateConfig<'info> {
    pub authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        has_one = authority,
    )]
    pub config: Account<'info, Config>,
}

#[derive(Accounts)]
pub struct AcceptAuthority<'info> {
    pub new_authority: Signer<'info>,

    #[account(
        mut,
        seeds = [CONFIG_SEED],
        bump = config.bump,
        constraint = config.pending_authority == new_authority.key()
            @ LabkitError::NotPendingAuthority,
    )]
    pub config: Account<'info, Config>,
}
