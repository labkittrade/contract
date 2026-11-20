//! ORAO VRF, called by hand from its IDL so LabKit doesn't depend on its crate.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke;

use crate::constants::*;
use crate::errors::LabkitError;

/// ORAO `request_v2`. The payer pays the fee and the request account's rent.
pub fn request<'info>(
    vrf: &AccountInfo<'info>,
    payer: &AccountInfo<'info>,
    network_state: &AccountInfo<'info>,
    treasury: &AccountInfo<'info>,
    randomness: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    seed: [u8; 32],
) -> Result<()> {
    let mut data = ORAO_REQUEST_V2.to_vec();
    data.extend_from_slice(&seed);
    let ix = Instruction {
        program_id: ORAO_VRF_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(payer.key(), true),
            AccountMeta::new(network_state.key(), false),
            AccountMeta::new(treasury.key(), false),
            AccountMeta::new(randomness.key(), false),
            AccountMeta::new_readonly(system_program.key(), false),
        ],
        data,
    };
    invoke(
        &ix,
        &[
            payer.clone(),
            network_state.clone(),
            treasury.clone(),
            randomness.clone(),
            system_program.clone(),
            vrf.clone(),
        ],
    )?;
    Ok(())
}

pub fn randomness_address(seed: &[u8; 32]) -> Pubkey {
    Pubkey::find_program_address(&[ORAO_RANDOMNESS_SEED, seed], &ORAO_VRF_PROGRAM_ID).0
}

/// The request's randomness once ORAO has fulfilled it, `None` while it is pending or the
/// account doesn't exist.
pub fn fulfilled(account: &AccountInfo, seed: &[u8; 32]) -> Result<Option<[u8; 64]>> {
    require_keys_eq!(account.key(), randomness_address(seed), LabkitError::BadCandidate);
    if account.owner != &ORAO_VRF_PROGRAM_ID {
        return Ok(None);
    }
    let data = account.try_borrow_data()?;
    if data.len() < ORAO_RANDOMNESS_OFFSET + 64
        || data[..8] != ORAO_RANDOMNESS_V2_DISC
        || data[8] != ORAO_FULFILLED_VARIANT
    {
        return Ok(None);
    }
    Ok(Some(data[ORAO_RANDOMNESS_OFFSET..ORAO_RANDOMNESS_OFFSET + 64].try_into().unwrap()))
}
