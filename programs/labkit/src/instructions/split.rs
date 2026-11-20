use anchor_lang::prelude::*;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::*;
use crate::errors::LabkitError;
use crate::events::FeesSplit;
use crate::payout::transfer_signed;
use crate::state::Config;

/// Sends everything in the fee wallet's ATA to the treasury. Frontends route referral
/// fees there.
pub fn handler(ctx: Context<SplitFees>) -> Result<()> {
    let a = &ctx.accounts;
    let amount = a.fee_wallet_token.amount;
    require!(amount > 0, LabkitError::NothingToSplit);
    let bump = [a.config.fee_wallet_bump];
    transfer_signed(
        &a.fee_wallet_token,
        &a.treasury_token,
        &a.fee_wallet.to_account_info(),
        &a.mint,
        &a.token_program,
        &[&[FEE_WALLET_SEED, &bump]],
        amount,
    )?;
    emit!(FeesSplit {
        mint: a.mint.key(),
        treasury_amount: amount,
        slot: Clock::get()?.slot,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct SplitFees<'info> {
    pub crank: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Account<'info, Config>,

    /// CHECK: LabKit's fee wallet PDA. Signs the transfer out of its ATA.
    #[account(seeds = [FEE_WALLET_SEED], bump = config.fee_wallet_bump)]
    pub fee_wallet: UncheckedAccount<'info>,

    #[account(mint::token_program = token_program)]
    pub mint: InterfaceAccount<'info, Mint>,

    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = fee_wallet,
        associated_token::token_program = token_program,
    )]
    pub fee_wallet_token: InterfaceAccount<'info, TokenAccount>,

    #[account(
        mut,
        token::mint = mint,
        token::token_program = token_program,
        constraint = treasury_token.owner == config.treasury @ LabkitError::TreasuryMismatch,
    )]
    pub treasury_token: InterfaceAccount<'info, TokenAccount>,

    pub token_program: Interface<'info, TokenInterface>,
}
