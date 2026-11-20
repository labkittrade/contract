use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;

use crate::constants::*;
use crate::errors::LabkitError;
use crate::state::PlatformTemplate;

/// LaunchLab validates its own PDAs (fee vault authorities and fee vaults) and that
/// the signing wallet matches the platform config or the vault's creator.
pub struct ClaimPlatformFeeFromVault<'a, 'info> {
    pub platform_fee_wallet: &'a AccountInfo<'info>,
    pub fee_vault_authority: &'a AccountInfo<'info>,
    pub platform_config: &'a AccountInfo<'info>,
    pub platform_fee_vault: &'a AccountInfo<'info>,
    pub recipient_token_account: &'a AccountInfo<'info>,
    pub quote_mint: &'a AccountInfo<'info>,
    pub token_program: &'a AccountInfo<'info>,
    pub system_program: &'a AccountInfo<'info>,
    pub associated_token_program: &'a AccountInfo<'info>,
    pub launchlab_program: &'a AccountInfo<'info>,
}

pub struct ClaimCreatorFee<'a, 'info> {
    pub creator: &'a AccountInfo<'info>,
    pub fee_vault_authority: &'a AccountInfo<'info>,
    pub creator_fee_vault: &'a AccountInfo<'info>,
    pub recipient_token_account: &'a AccountInfo<'info>,
    pub quote_mint: &'a AccountInfo<'info>,
    pub token_program: &'a AccountInfo<'info>,
    pub system_program: &'a AccountInfo<'info>,
    pub associated_token_program: &'a AccountInfo<'info>,
    pub launchlab_program: &'a AccountInfo<'info>,
}

/// Drains the platform's per-quote-mint fee vault into the fee wallet's ATA.
/// LaunchLab reverts when the vault is empty.
pub fn claim_platform_fee_from_vault(
    a: ClaimPlatformFeeFromVault,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    invoke(
        a.launchlab_program,
        CLAIM_PLATFORM_FEE_FROM_VAULT.to_vec(),
        &[
            (a.platform_fee_wallet, true, true),
            (a.fee_vault_authority, false, false),
            (a.platform_config, false, false),
            (a.platform_fee_vault, false, true),
            (a.recipient_token_account, false, true),
            (a.quote_mint, false, false),
            (a.token_program, false, false),
            (a.system_program, false, false),
            (a.associated_token_program, false, false),
        ],
        signer_seeds,
    )
}

/// Drains a creator's per-quote-mint fee vault into the creator's ATA.
/// LaunchLab reverts when the vault is empty.
pub fn claim_creator_fee(a: ClaimCreatorFee, signer_seeds: &[&[&[u8]]]) -> Result<()> {
    invoke(
        a.launchlab_program,
        CLAIM_CREATOR_FEE.to_vec(),
        &[
            (a.creator, true, true),
            (a.fee_vault_authority, false, false),
            (a.creator_fee_vault, false, true),
            (a.recipient_token_account, false, true),
            (a.quote_mint, false, false),
            (a.token_program, false, false),
            (a.system_program, false, false),
            (a.associated_token_program, false, false),
        ],
        signer_seeds,
    )
}

pub struct CreateVestingAccount<'a, 'info> {
    pub creator: &'a AccountInfo<'info>,
    pub beneficiary: &'a AccountInfo<'info>,
    pub pool_state: &'a AccountInfo<'info>,
    pub vesting_record: &'a AccountInfo<'info>,
    pub system_program: &'a AccountInfo<'info>,
    pub launchlab_program: &'a AccountInfo<'info>,
}

/// Allocates `share_amount` of the launch's locked supply to `beneficiary`. The
/// creator signs and pays the vesting record's rent.
pub fn create_vesting_account(
    a: CreateVestingAccount,
    share_amount: u64,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let mut data = CREATE_VESTING_ACCOUNT.to_vec();
    data.extend_from_slice(&share_amount.to_le_bytes());
    invoke(
        a.launchlab_program,
        data,
        &[
            (a.creator, true, true),
            (a.beneficiary, false, true),
            (a.pool_state, false, true),
            (a.vesting_record, false, true),
            (a.system_program, false, false),
        ],
        signer_seeds,
    )
}

pub struct CreatePlatformConfig<'a, 'info> {
    pub platform_admin: &'a AccountInfo<'info>,
    pub platform_fee_wallet: &'a AccountInfo<'info>,
    pub platform_nft_wallet: &'a AccountInfo<'info>,
    pub platform_config: &'a AccountInfo<'info>,
    pub cpswap_config: &'a AccountInfo<'info>,
    pub system_program: &'a AccountInfo<'info>,
    pub transfer_fee_extension_authority: &'a AccountInfo<'info>,
    pub platform_vesting_wallet: &'a AccountInfo<'info>,
    pub launchlab_program: &'a AccountInfo<'info>,
}

/// Creates a platform from the template. The admin signs and pays the config's rent.
pub fn create_platform_config(
    a: CreatePlatformConfig,
    template: &PlatformTemplate,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let mut data = CREATE_PLATFORM_CONFIG.to_vec();
    for value in [
        template.lp_lock_scale,
        0,
        RATE_DENOMINATOR - template.lp_lock_scale,
        template.fee_rate,
    ] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    for text in [PLATFORM_NAME, template.web.as_str(), template.img.as_str()] {
        data.extend_from_slice(&(text.len() as u32).to_le_bytes());
        data.extend_from_slice(text.as_bytes());
    }
    data.extend_from_slice(&template.creator_fee_rate.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    invoke(
        a.launchlab_program,
        data,
        &[
            (a.platform_admin, true, true),
            (a.platform_fee_wallet, false, false),
            (a.platform_nft_wallet, false, false),
            (a.platform_config, false, true),
            (a.cpswap_config, false, false),
            (a.system_program, false, false),
            (a.transfer_fee_extension_authority, false, false),
            (a.platform_vesting_wallet, false, false),
        ],
        signer_seeds,
    )
}

pub struct BuyExactIn<'a, 'info> {
    pub payer: &'a AccountInfo<'info>,
    pub authority: &'a AccountInfo<'info>,
    pub global_config: &'a AccountInfo<'info>,
    pub platform_config: &'a AccountInfo<'info>,
    pub pool_state: &'a AccountInfo<'info>,
    pub user_base_token: &'a AccountInfo<'info>,
    pub user_quote_token: &'a AccountInfo<'info>,
    pub base_vault: &'a AccountInfo<'info>,
    pub quote_vault: &'a AccountInfo<'info>,
    pub base_token_mint: &'a AccountInfo<'info>,
    pub quote_token_mint: &'a AccountInfo<'info>,
    pub base_token_program: &'a AccountInfo<'info>,
    pub quote_token_program: &'a AccountInfo<'info>,
    pub event_authority: &'a AccountInfo<'info>,
    pub launchlab_program: &'a AccountInfo<'info>,
    pub system_program: &'a AccountInfo<'info>,
    pub platform_fee_vault: &'a AccountInfo<'info>,
    pub creator_fee_vault: &'a AccountInfo<'info>,
}

/// A curve buy with no share fee. The fee vaults are LaunchLab's remaining accounts.
pub fn buy_exact_in(
    a: BuyExactIn,
    amount_in: u64,
    minimum_amount_out: u64,
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let mut data = BUY_EXACT_IN.to_vec();
    for value in [amount_in, minimum_amount_out, 0] {
        data.extend_from_slice(&value.to_le_bytes());
    }
    invoke(
        a.launchlab_program,
        data,
        &[
            (a.payer, true, true),
            (a.authority, false, false),
            (a.global_config, false, false),
            (a.platform_config, false, false),
            (a.pool_state, false, true),
            (a.user_base_token, false, true),
            (a.user_quote_token, false, true),
            (a.base_vault, false, true),
            (a.quote_vault, false, true),
            (a.base_token_mint, false, false),
            (a.quote_token_mint, false, false),
            (a.base_token_program, false, false),
            (a.quote_token_program, false, false),
            (a.event_authority, false, false),
            (a.launchlab_program, false, false),
            (a.system_program, false, false),
            (a.platform_fee_vault, false, true),
            (a.creator_fee_vault, false, true),
        ],
        signer_seeds,
    )
}

pub struct PoolView {
    pub status: u8,
    pub total_base_sell: u64,
    pub virtual_base: u64,
    pub virtual_quote: u64,
    pub real_base: u64,
    pub real_quote: u64,
    pub global_config: Pubkey,
    pub platform_config: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub creator: Pubkey,
    pub token_program_flag: u8,
    pub amm_creator_fee_on: u8,
}

impl PoolView {
    /// `(quote, base)` reserves the constant-product curve prices against.
    pub fn reserves(&self) -> Result<(u64, u64)> {
        let quote = self
            .virtual_quote
            .checked_add(self.real_quote)
            .ok_or(LabkitError::Overflow)?;
        let base = self
            .virtual_base
            .checked_sub(self.real_base)
            .ok_or(LabkitError::Overflow)?;
        Ok((quote, base))
    }

    /// Tokens still for sale on the curve.
    pub fn base_left(&self) -> u64 {
        self.total_base_sell.saturating_sub(self.real_base)
    }
}

pub fn decode_pool_state(data: &[u8]) -> Result<PoolView> {
    require!(
        data.len() > POOL_AMM_CREATOR_FEE_ON_OFFSET && data[..8] == POOL_STATE_DISC,
        LabkitError::BadPoolState
    );
    Ok(PoolView {
        status: data[POOL_STATUS_OFFSET],
        total_base_sell: u64_at(data, POOL_TOTAL_BASE_SELL_OFFSET),
        virtual_base: u64_at(data, POOL_VIRTUAL_BASE_OFFSET),
        virtual_quote: u64_at(data, POOL_VIRTUAL_QUOTE_OFFSET),
        real_base: u64_at(data, POOL_REAL_BASE_OFFSET),
        real_quote: u64_at(data, POOL_REAL_QUOTE_OFFSET),
        global_config: key_at(data, POOL_GLOBAL_CONFIG_OFFSET),
        platform_config: key_at(data, POOL_PLATFORM_CONFIG_OFFSET),
        base_mint: key_at(data, POOL_BASE_MINT_OFFSET),
        quote_mint: key_at(data, POOL_QUOTE_MINT_OFFSET),
        creator: key_at(data, POOL_CREATOR_OFFSET),
        token_program_flag: data[POOL_TOKEN_PROGRAM_FLAG_OFFSET],
        amm_creator_fee_on: data[POOL_AMM_CREATOR_FEE_ON_OFFSET],
    })
}

/// The CPMM `AmmConfig` the platform's launches graduate into.
pub fn platform_cpswap_config(platform_config: &[u8]) -> Result<Pubkey> {
    require!(
        platform_config.len() >= PLATFORM_CPSWAP_CONFIG_OFFSET + 32
            && platform_config[..8] == PLATFORM_CONFIG_DISC,
        LabkitError::PlatformMismatch
    );
    Ok(key_at(platform_config, PLATFORM_CPSWAP_CONFIG_OFFSET))
}

/// Every fee a curve buy pays on the launch's platform, on the 1_000_000 denominator.
pub fn curve_fee_rate(global_config: &[u8], platform_config: &[u8]) -> Result<u64> {
    require!(
        global_config.len() >= GLOBAL_TRADE_FEE_RATE_OFFSET + 8,
        LabkitError::BadPoolState
    );
    require!(
        platform_config.len() >= PLATFORM_CREATOR_FEE_RATE_OFFSET + 8
            && platform_config[..8] == PLATFORM_CONFIG_DISC,
        LabkitError::PlatformMismatch
    );
    [
        u64_at(global_config, GLOBAL_TRADE_FEE_RATE_OFFSET),
        u64_at(platform_config, PLATFORM_FEE_RATE_OFFSET),
        u64_at(platform_config, PLATFORM_CREATOR_FEE_RATE_OFFSET),
    ]
    .iter()
    .try_fold(0u64, |sum, rate| sum.checked_add(*rate))
    .ok_or_else(|| error!(LabkitError::Overflow))
}

pub(crate) fn key_at(data: &[u8], offset: usize) -> Pubkey {
    let mut key = [0u8; 32];
    key.copy_from_slice(&data[offset..offset + 32]);
    Pubkey::new_from_array(key)
}

pub(crate) fn u64_at(data: &[u8], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&data[offset..offset + 8]);
    u64::from_le_bytes(bytes)
}

/// Each entry is `(account, is_signer, is_writable)`, in the callee's account order.
pub(crate) fn invoke<'info>(
    program: &AccountInfo<'info>,
    data: Vec<u8>,
    accounts: &[(&AccountInfo<'info>, bool, bool)],
    signer_seeds: &[&[&[u8]]],
) -> Result<()> {
    let metas = accounts
        .iter()
        .map(|(account, signer, writable)| {
            if *writable {
                AccountMeta::new(account.key(), *signer)
            } else {
                AccountMeta::new_readonly(account.key(), *signer)
            }
        })
        .collect();
    let mut infos: Vec<AccountInfo<'info>> = accounts.iter().map(|(a, _, _)| (*a).clone()).collect();
    infos.push(program.clone());
    invoke_signed(
        &Instruction {
            program_id: program.key(),
            accounts: metas,
            data,
        },
        &infos,
        signer_seeds,
    )?;
    Ok(())
}
