use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::LabkitError;
use crate::launchlab::{invoke, key_at, u64_at};

pub struct CpmmPoolView {
    pub amm_config: Pubkey,
    pub creator: Pubkey,
    pub vaults: [Pubkey; 2],
    pub mints: [Pubkey; 2],
    pub observation: Pubkey,
    /// Protocol, fund and creator fees booked per side. They sit in the vaults until
    /// collected, so they are not liquidity.
    pub booked: [u64; 2],
    pub creator_fee_enabled: bool,
}

pub fn decode_cpmm_pool(account: &AccountInfo) -> Result<CpmmPoolView> {
    require!(account.owner == &CPMM_PROGRAM_ID, LabkitError::BadAmmPool);
    let data = account.try_borrow_data()?;
    require!(
        data.len() == CPMM_POOL_LEN && data[..8] == POOL_STATE_DISC,
        LabkitError::BadAmmPool
    );
    let booked = |side: usize| -> Result<u64> {
        [
            CPMM_POOL_PROTOCOL_FEES_0_OFFSET,
            CPMM_POOL_FUND_FEES_0_OFFSET,
            CPMM_POOL_CREATOR_FEES_0_OFFSET,
        ]
        .iter()
        .try_fold(0u64, |sum, offset| sum.checked_add(u64_at(&data, offset + 8 * side)))
        .ok_or_else(|| error!(LabkitError::Overflow))
    };
    Ok(CpmmPoolView {
        amm_config: key_at(&data, CPMM_POOL_AMM_CONFIG_OFFSET),
        creator: key_at(&data, CPMM_POOL_CREATOR_OFFSET),
        vaults: [
            key_at(&data, CPMM_POOL_VAULT_0_OFFSET),
            key_at(&data, CPMM_POOL_VAULT_1_OFFSET),
        ],
        mints: [
            key_at(&data, CPMM_POOL_MINT_0_OFFSET),
            key_at(&data, CPMM_POOL_MINT_1_OFFSET),
        ],
        observation: key_at(&data, CPMM_POOL_OBSERVATION_OFFSET),
        booked: [booked(0)?, booked(1)?],
        creator_fee_enabled: data[CPMM_POOL_ENABLE_CREATOR_FEE_OFFSET] != 0,
    })
}

/// Trade fee plus, when the pool charges it, the creator fee, on the 1_000_000 denominator.
pub fn cpmm_fee_rate(amm_config: &AccountInfo, pool: &CpmmPoolView) -> Result<u64> {
    require!(
        amm_config.owner == &CPMM_PROGRAM_ID && amm_config.key() == pool.amm_config,
        LabkitError::BadAmmPool
    );
    let data = amm_config.try_borrow_data()?;
    require!(
        data.len() >= CPMM_CONFIG_CREATOR_FEE_RATE_OFFSET + 8,
        LabkitError::BadAmmPool
    );
    let creator = if pool.creator_fee_enabled {
        u64_at(&data, CPMM_CONFIG_CREATOR_FEE_RATE_OFFSET)
    } else {
        0
    };
    u64_at(&data, CPMM_CONFIG_TRADE_FEE_RATE_OFFSET)
        .checked_add(creator)
        .ok_or_else(|| error!(LabkitError::Overflow))
}

pub struct SwapBaseInput<'a, 'info> {
    pub payer: &'a AccountInfo<'info>,
    pub authority: &'a AccountInfo<'info>,
    pub amm_config: &'a AccountInfo<'info>,
    pub pool_state: &'a AccountInfo<'info>,
    pub input_token_account: &'a AccountInfo<'info>,
    pub output_token_account: &'a AccountInfo<'info>,
    pub input_vault: &'a AccountInfo<'info>,
    pub output_vault: &'a AccountInfo<'info>,
    pub input_token_program: &'a AccountInfo<'info>,
    pub output_token_program: &'a AccountInfo<'info>,
    pub input_token_mint: &'a AccountInfo<'info>,
    pub output_token_mint: &'a AccountInfo<'info>,
    pub observation_state: &'a AccountInfo<'info>,
    pub cpmm_program: &'a AccountInfo<'info>,
}

pub fn swap_base_input(
    a: SwapBaseInput,
    amount_in: u64,
    minimum_amount_out: u64,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let mut data = SWAP_BASE_INPUT.to_vec();
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&minimum_amount_out.to_le_bytes());
    invoke(
        a.cpmm_program,
        data,
        &[
            (a.payer, true, false),
            (a.authority, false, false),
            (a.amm_config, false, false),
            (a.pool_state, false, true),
            (a.input_token_account, false, true),
            (a.output_token_account, false, true),
            (a.input_vault, false, true),
            (a.output_vault, false, true),
            (a.input_token_program, false, false),
            (a.output_token_program, false, false),
            (a.input_token_mint, false, false),
            (a.output_token_mint, false, false),
            (a.observation_state, false, true),
        ],
        signer_seeds,
    )
}
