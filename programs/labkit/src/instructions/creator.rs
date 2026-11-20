use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::*;
use crate::errors::LabkitError;
use crate::events::CreatorFeesSplit;
use crate::launchlab::{claim_creator_fee, decode_pool_state, ClaimCreatorFee};
use crate::payout::{bps_of, transfer_signed};
use crate::state::{Config, FeeSplit, Launch};

/// Claims a launch's bonding-curve creator fees and splits what was claimed between the
/// keeper and the recipient.
pub fn claim(ctx: Context<ClaimCreatorFees>) -> Result<()> {
    let before = ctx.accounts.creator_token.amount;
    let base_mint = ctx.accounts.launch.base_mint;
    let bump = [ctx.accounts.launch.creator_bump];
    let seeds: &[&[&[u8]]] = &[&[CREATOR_SEED, base_mint.as_ref(), &bump]];
    {
        let a = &ctx.accounts;
        claim_creator_fee(
            ClaimCreatorFee {
                creator: &a.creator.to_account_info(),
                fee_vault_authority: &a.fee_vault_authority.to_account_info(),
                creator_fee_vault: &a.creator_fee_vault.to_account_info(),
                recipient_token_account: &a.creator_token.to_account_info(),
                quote_mint: &a.quote_mint.to_account_info(),
                token_program: &a.token_program.to_account_info(),
                system_program: &a.system_program.to_account_info(),
                associated_token_program: &a.associated_token_program.to_account_info(),
                launchlab_program: &a.launchlab_program.to_account_info(),
            },
            seeds,
        )?;
    }
    ctx.accounts.creator_token.reload()?;
    let claimed = ctx
        .accounts
        .creator_token
        .amount
        .checked_sub(before)
        .ok_or(LabkitError::Overflow)?;
    let shares = shares(&ctx.accounts.launch.fees, claimed, false)?;

    let a = &ctx.accounts;
    let authority = a.creator.to_account_info();
    for (to, amount) in [(&a.crank_token, shares.crank), (&a.recipient_token, shares.recipient)] {
        transfer_signed(&a.creator_token, to, &authority, &a.quote_mint, &a.token_program, seeds, amount)?;
    }
    emit_split(&a.launch, &a.crank, claimed, &shares)
}

/// Splits whatever sits in a creator PDA's WSOL account. Once the launch trades on
/// Raydium, that is CPMM creator fees: part counts as creator-side and part as
/// platform-side, which goes to the kit and the treasury.
pub fn split_only(ctx: Context<SplitCreatorFees>) -> Result<()> {
    let amount = ctx.accounts.creator_token.amount;
    require!(amount > 0, LabkitError::NothingToSplit);
    let pool = decode_pool_state(&ctx.accounts.pool_state.try_borrow_data()?)?;
    let migrated = pool.status == POOL_STATUS_MIGRATED;
    let shares = shares(&ctx.accounts.launch.fees, amount, migrated)?;

    let a = &ctx.accounts;
    let base_mint = a.launch.base_mint;
    let bump = [a.launch.creator_bump];
    let seeds: &[&[&[u8]]] = &[&[CREATOR_SEED, base_mint.as_ref(), &bump]];
    let authority = a.creator.to_account_info();
    for (to, amount) in [
        (&a.crank_token, shares.crank),
        (&a.recipient_token, shares.recipient),
        (&a.kit_quote, shares.kit),
        (&a.treasury_token, shares.treasury),
    ] {
        transfer_signed(&a.creator_token, to, &authority, &a.quote_mint, &a.token_program, seeds, amount)?;
    }
    let launch = &mut ctx.accounts.launch;
    launch.kit_funded = launch
        .kit_funded
        .checked_add(shares.kit)
        .ok_or(LabkitError::Overflow)?;
    emit_split(&ctx.accounts.launch, &ctx.accounts.crank, 0, &shares)
}

#[derive(Debug, PartialEq)]
struct Shares {
    recipient: u64,
    crank: u64,
    kit: u64,
    treasury: u64,
}

/// On the curve every creator fee is creator-side. On Raydium, `amm_creator_bps` of it
/// is; the rest is platform-side and splits like a platform fee.
fn shares(fees: &FeeSplit, amount: u64, migrated: bool) -> Result<Shares> {
    let creator_side = if migrated {
        bps_of(amount, fees.amm_creator_bps)?
    } else {
        amount
    };
    let platform_side = amount - creator_side;
    let crank = bps_of(creator_side, fees.crank_bps)?;
    let kit = bps_of(platform_side, fees.kit_bps)?;
    Ok(Shares {
        recipient: creator_side - crank,
        crank,
        kit,
        treasury: platform_side - kit,
    })
}

fn emit_split(launch: &Launch, crank: &Signer, claimed: u64, shares: &Shares) -> Result<()> {
    emit!(CreatorFeesSplit {
        base_mint: launch.base_mint,
        claimed,
        recipient_amount: shares.recipient,
        crank_amount: shares.crank,
        kit_amount: shares.kit,
        treasury_amount: shares.treasury,
        crank: crank.key(),
        slot: Clock::get()?.slot,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct ClaimCreatorFees<'info> {
    /// Pays for the creator PDA's WSOL account the first time.
    #[account(mut)]
    pub crank: Signer<'info>,

    #[account(seeds = [LAUNCH_SEED, launch.base_mint.as_ref()], bump = launch.bump)]
    pub launch: Box<Account<'info, Launch>>,

    /// CHECK: The launch's creator PDA. LaunchLab requires it writable and signing.
    #[account(
        mut,
        seeds = [CREATOR_SEED, launch.base_mint.as_ref()],
        bump = launch.creator_bump,
    )]
    pub creator: UncheckedAccount<'info>,

    #[account(address = WSOL_MINT, mint::token_program = token_program)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    /// Created here so LaunchLab's `init_if_needed` never asks the creator PDA for rent.
    #[account(
        init_if_needed,
        payer = crank,
        associated_token::mint = quote_mint,
        associated_token::authority = creator,
        associated_token::token_program = token_program,
    )]
    pub creator_token: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = quote_mint,
        token::token_program = token_program,
        constraint = recipient_token.owner == launch.recipient @ LabkitError::RecipientMismatch,
    )]
    pub recipient_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Any WSOL account. The crank picks where its reward goes.
    #[account(mut, token::mint = quote_mint, token::token_program = token_program)]
    pub crank_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: LaunchLab PDA `[creator, quote_mint]`, checked by LaunchLab.
    #[account(mut)]
    pub creator_fee_vault: UncheckedAccount<'info>,

    /// CHECK: LaunchLab PDA `["creator_fee_vault_auth_seed"]`, checked by LaunchLab.
    pub fee_vault_authority: UncheckedAccount<'info>,

    /// CHECK: Address-checked LaunchLab program.
    #[account(address = LAUNCHLAB_PROGRAM_ID)]
    pub launchlab_program: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SplitCreatorFees<'info> {
    /// Pays for the kit's WSOL account if a Raydium split comes before any platform claim.
    #[account(mut)]
    pub crank: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        mut,
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        has_one = pool_state @ LabkitError::PoolMismatch,
        constraint = launch.registered @ LabkitError::NotRegistered,
    )]
    pub launch: Box<Account<'info, Launch>>,

    /// CHECK: The launch's LaunchLab `PoolState`, pinned by `has_one`.
    pub pool_state: UncheckedAccount<'info>,

    /// CHECK: The launch's creator PDA. Signs the transfers out of its WSOL account.
    #[account(seeds = [CREATOR_SEED, launch.base_mint.as_ref()], bump = launch.creator_bump)]
    pub creator: UncheckedAccount<'info>,

    /// CHECK: The launch's kit PDA, which owns `kit_quote`.
    #[account(seeds = [KIT_SEED, launch.base_mint.as_ref()], bump = launch.kit_bump)]
    pub kit: UncheckedAccount<'info>,

    #[account(address = WSOL_MINT, mint::token_program = token_program)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        associated_token::mint = quote_mint,
        associated_token::authority = creator,
        associated_token::token_program = token_program,
    )]
    pub creator_token: Box<InterfaceAccount<'info, TokenAccount>>,

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
        constraint = recipient_token.owner == launch.recipient @ LabkitError::RecipientMismatch,
    )]
    pub recipient_token: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        token::mint = quote_mint,
        token::token_program = token_program,
        constraint = treasury_token.owner == config.treasury @ LabkitError::TreasuryMismatch,
    )]
    pub treasury_token: Box<InterfaceAccount<'info, TokenAccount>>,

    /// Any WSOL account. The crank picks where its reward goes.
    #[account(mut, token::mint = quote_mint, token::token_program = token_program)]
    pub crank_token: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEES: FeeSplit = FeeSplit {
        crank_bps: 500,
        kit_bps: 7_000,
        amm_creator_bps: 5_000,
    };

    #[test]
    fn curve_creator_fees_go_95_to_the_dev_and_5_to_the_keeper() {
        let s = shares(&FEES, 1_000_000, false).unwrap();
        assert_eq!(
            s,
            Shares {
                recipient: 950_000,
                crank: 50_000,
                kit: 0,
                treasury: 0
            }
        );
    }

    #[test]
    fn raydium_creator_fees_split_half_like_a_creator_fee_and_half_like_a_platform_fee() {
        let s = shares(&FEES, 1_000_000, true).unwrap();
        assert_eq!(
            s,
            Shares {
                recipient: 475_000,
                crank: 25_000,
                kit: 350_000,
                treasury: 150_000
            }
        );
        let odd = shares(&FEES, 999, true).unwrap();
        assert_eq!(odd.recipient + odd.crank + odd.kit + odd.treasury, 999);
    }
}
