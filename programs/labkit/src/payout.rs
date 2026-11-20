use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::constants::BPS_DENOMINATOR;
use crate::errors::LabkitError;

pub(crate) fn bps_of(amount: u64, bps: u16) -> Result<u64> {
    let share = (amount as u128)
        .checked_mul(bps as u128)
        .ok_or(LabkitError::Overflow)?
        / BPS_DENOMINATOR as u128;
    u64::try_from(share).map_err(|_| error!(LabkitError::Overflow))
}

/// Transfers out of a token account owned by a LabKit PDA. Zero amounts are skipped.
pub(crate) fn transfer_signed<'info>(
    from: &InterfaceAccount<'info, TokenAccount>,
    to: &InterfaceAccount<'info, TokenAccount>,
    authority: &AccountInfo<'info>,
    mint: &InterfaceAccount<'info, Mint>,
    token_program: &Interface<'info, TokenInterface>,
    seeds: &[&[&[u8]]],
    amount: u64,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    transfer_checked(
        CpiContext::new_with_signer(
            token_program.to_account_info(),
            TransferChecked {
                from: from.to_account_info(),
                mint: mint.to_account_info(),
                to: to.to_account_info(),
                authority: authority.clone(),
            },
            seeds,
        ),
        amount,
        mint.decimals,
    )
}
