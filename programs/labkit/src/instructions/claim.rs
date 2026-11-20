use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::*;
use crate::errors::LabkitError;
use crate::events::KitFeesClaimed;
use crate::launchlab::{claim_platform_fee_from_vault, ClaimPlatformFeeFromVault};
use crate::payout::{bps_of, transfer_signed};
use crate::state::{Config, Launch};

/// Claims the launch's platform fee vault into the kit's WSOL account, keeps the kit's
/// share there, and sends the rest to the treasury.
pub fn handler(ctx: Context<ClaimKitFees>) -> Result<()> {
    let before = ctx.accounts.kit_quote.amount;
    let base_mint = ctx.accounts.launch.base_mint;
    let bump = [ctx.accounts.launch.kit_bump];
    let seeds: &[&[&[u8]]] = &[&[KIT_SEED, base_mint.as_ref(), &bump]];
    {
        let a = &ctx.accounts;
        claim_platform_fee_from_vault(
            ClaimPlatformFeeFromVault {
                platform_fee_wallet: &a.kit.to_account_info(),
                fee_vault_authority: &a.fee_vault_authority.to_account_info(),
                platform_config: &a.platform_config.to_account_info(),
                platform_fee_vault: &a.platform_fee_vault.to_account_info(),
                recipient_token_account: &a.kit_quote.to_account_info(),
                quote_mint: &a.quote_mint.to_account_info(),
                token_program: &a.token_program.to_account_info(),
                system_program: &a.system_program.to_account_info(),
                associated_token_program: &a.associated_token_program.to_account_info(),
                launchlab_program: &a.launchlab_program.to_account_info(),
            },
            seeds,
        )?;
    }
    ctx.accounts.kit_quote.reload()?;
    let claimed = ctx
        .accounts
        .kit_quote
        .amount
        .checked_sub(before)
        .ok_or(LabkitError::Overflow)?;
    let kit_amount = bps_of(claimed, ctx.accounts.launch.fees.kit_bps)?;
    let treasury_amount = claimed - kit_amount;

    let a = &ctx.accounts;
    transfer_signed(
        &a.kit_quote,
        &a.treasury_token,
        &a.kit.to_account_info(),
        &a.quote_mint,
        &a.token_program,
        seeds,
        treasury_amount,
    )?;
    let launch = &mut ctx.accounts.launch;
    launch.kit_funded = launch
        .kit_funded
        .checked_add(kit_amount)
        .ok_or(LabkitError::Overflow)?;
    emit!(KitFeesClaimed {
        base_mint,
        claimed,
        kit_amount,
        treasury_amount,
        crank: ctx.accounts.crank.key(),
        slot: Clock::get()?.slot,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct ClaimKitFees<'info> {
    /// Pays for the kit's WSOL account the first time.
    #[account(mut)]
    pub crank: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        mut,
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        has_one = platform_config @ LabkitError::PlatformMismatch,
    )]
    pub launch: Box<Account<'info, Launch>>,

    /// CHECK: The platform's fee wallet. LaunchLab requires it writable and signing.
    #[account(mut, seeds = [KIT_SEED, launch.base_mint.as_ref()], bump = launch.kit_bump)]
    pub kit: UncheckedAccount<'info>,

    #[account(address = WSOL_MINT, mint::token_program = token_program)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Created here so LaunchLab's `init_if_needed` never asks the kit PDA for rent.
    #[account(
        init_if_needed,
        payer = crank,
        associated_token::mint = quote_mint,
        associated_token::authority = kit,
        associated_token::token_program = token_program,
    )]
    pub kit_quote: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = quote_mint,
        token::token_program = token_program,
        constraint = treasury_token.owner == config.treasury @ LabkitError::TreasuryMismatch,
    )]
    pub treasury_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: The launch's LaunchLab `PlatformConfig`, pinned by `has_one`.
    pub platform_config: UncheckedAccount<'info>,

    /// CHECK: LaunchLab PDA `[platform_config, quote_mint]`, checked by LaunchLab.
    #[account(mut)]
    pub platform_fee_vault: UncheckedAccount<'info>,

    /// CHECK: LaunchLab PDA `["platform_fee_vault_auth_seed"]`, checked by LaunchLab.
    pub fee_vault_authority: UncheckedAccount<'info>,

    /// CHECK: Address-checked LaunchLab program.
    #[account(address = LAUNCHLAB_PROGRAM_ID)]
    pub launchlab_program: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}
