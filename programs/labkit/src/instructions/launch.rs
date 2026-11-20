use anchor_lang::prelude::*;
use anchor_lang::system_program::{transfer, Transfer};

use crate::constants::*;
use crate::errors::LabkitError;
use crate::events::{CreatorVestingCreated, LaunchCreated, LaunchRecipientChanged, LaunchRegistered};
use crate::launchlab::{
    create_platform_config, create_vesting_account, decode_pool_state, CreatePlatformConfig,
    CreateVestingAccount,
};
use crate::policy::{price_q64, validate_kit};
use crate::state::{Config, Launch, LaunchParams};

/// Creates the launch and its own LaunchLab platform from the config's template. The
/// platform's fee wallet is the kit PDA and its admin is a PDA LabKit never signs for
/// again, so the platform can't change. Signed by the base mint keypair, which LaunchLab
/// `initialize_v2` then uses in the next transaction.
pub fn create(ctx: Context<CreateLaunch>, params: LaunchParams) -> Result<()> {
    require!(params.recipient != Pubkey::default(), LabkitError::InvalidRecipient);
    validate_kit(params.kit, params.drawdown_bps)?;

    let a = &ctx.accounts;
    let base_mint = a.base_mint.key();
    let bump = [ctx.bumps.platform_admin];
    let seeds: &[&[&[u8]]] = &[&[PLATFORM_ADMIN_SEED, base_mint.as_ref(), &bump]];
    let system_program = a.system_program.to_account_info();
    let admin = a.platform_admin.to_account_info();
    let payer = a.payer.to_account_info();

    let before = admin.lamports();
    let rent = Rent::get()?.minimum_balance(PLATFORM_CONFIG_SPACE);
    transfer(
        CpiContext::new(
            system_program.clone(),
            Transfer {
                from: payer.clone(),
                to: admin.clone(),
            },
        ),
        rent,
    )?;
    create_platform_config(
        CreatePlatformConfig {
            platform_admin: &admin,
            platform_fee_wallet: &a.kit.to_account_info(),
            platform_nft_wallet: &a.nft_wallet.to_account_info(),
            platform_config: &a.platform_config.to_account_info(),
            cpswap_config: &a.cpswap_config.to_account_info(),
            system_program: &system_program,
            transfer_fee_extension_authority: &a.treasury.to_account_info(),
            platform_vesting_wallet: &a.treasury.to_account_info(),
            launchlab_program: &a.launchlab_program.to_account_info(),
        },
        &a.config.platform,
        seeds,
    )?;
    let unspent = admin.lamports().saturating_sub(before);
    if unspent > 0 {
        transfer(
            CpiContext::new_with_signer(
                system_program,
                Transfer {
                    from: admin,
                    to: payer,
                },
                seeds,
            ),
            unspent,
        )?;
    }

    let fees = ctx.accounts.config.fees;
    let launch = &mut ctx.accounts.launch;
    launch.bump = ctx.bumps.launch;
    launch.creator_bump = ctx.bumps.creator;
    launch.kit_bump = ctx.bumps.kit;
    launch.admin_bump = ctx.bumps.platform_admin;
    launch.kit = params.kit;
    launch.drawdown_bps = params.drawdown_bps;
    launch.fees = fees;
    launch.base_mint = base_mint;
    launch.platform_config = ctx.accounts.platform_config.key();
    launch.recipient = params.recipient;
    emit!(LaunchCreated {
        base_mint,
        platform_config: launch.platform_config,
        recipient: launch.recipient,
        kit: launch.kit,
        drawdown_bps: launch.drawdown_bps,
        fees,
    });
    Ok(())
}

/// Sent after LaunchLab `initialize_v2`. Anyone can send it: it only records the pool
/// once it is a SOL pool on the launch's platform with the creator PDA as `creator`.
/// The base mint must be SPL Token, since a Token-2022 transfer fee would leave staked
/// tokens short and kit buys under their minimum, and Raydium creator fees must be in
/// SOL, since only WSOL leaves the creator PDA.
pub fn register(ctx: Context<RegisterLaunch>) -> Result<()> {
    let launch = &mut ctx.accounts.launch;
    require!(!launch.registered, LabkitError::AlreadyRegistered);
    let pool = decode_pool_state(&ctx.accounts.pool_state.try_borrow_data()?)?;
    require_keys_eq!(pool.base_mint, launch.base_mint, LabkitError::BadPoolState);
    require_keys_eq!(pool.quote_mint, WSOL_MINT, LabkitError::QuoteNotSol);
    require!(pool.token_program_flag == 0, LabkitError::UnsupportedBaseToken);
    require!(
        pool.amm_creator_fee_on == AMM_CREATOR_FEE_ON_QUOTE,
        LabkitError::CreatorFeeNotQuote
    );
    require_keys_eq!(
        pool.creator,
        ctx.accounts.creator.key(),
        LabkitError::PoolCreatorMismatch
    );
    require_keys_eq!(
        pool.platform_config,
        launch.platform_config,
        LabkitError::PlatformMismatch
    );
    if launch.kit == KIT_FLOOR {
        launch.high_q64 = price_q64(pool.virtual_quote, pool.virtual_base)?;
        launch.high_at = Clock::get()?.unix_timestamp;
    }
    launch.pool_state = ctx.accounts.pool_state.key();
    launch.registered = true;
    emit!(LaunchRegistered {
        base_mint: launch.base_mint,
        pool_state: launch.pool_state,
    });
    Ok(())
}

pub fn set_recipient(ctx: Context<SetLaunchRecipient>, recipient: Pubkey) -> Result<()> {
    require!(recipient != Pubkey::default(), LabkitError::InvalidRecipient);
    let launch = &mut ctx.accounts.launch;
    let previous = launch.recipient;
    launch.recipient = recipient;
    emit!(LaunchRecipientChanged {
        base_mint: launch.base_mint,
        previous,
        recipient,
    });
    Ok(())
}

/// Signs LaunchLab `create_vesting_account` as the creator PDA. The recipient lends the
/// PDA the vesting record's rent and gets back whatever LaunchLab did not spend.
pub fn create_vesting(ctx: Context<CreateCreatorVesting>, share_amount: u64) -> Result<()> {
    let a = &ctx.accounts;
    let base_mint = a.launch.base_mint;
    let bump = [a.launch.creator_bump];
    let seeds: &[&[&[u8]]] = &[&[CREATOR_SEED, base_mint.as_ref(), &bump]];
    let system_program = a.system_program.to_account_info();
    let creator = a.creator.to_account_info();
    let recipient = a.recipient.to_account_info();

    let before = creator.lamports();
    transfer(
        CpiContext::new(
            system_program.clone(),
            Transfer {
                from: recipient.clone(),
                to: creator.clone(),
            },
        ),
        Rent::get()?.minimum_balance(VESTING_RECORD_SPACE),
    )?;
    create_vesting_account(
        CreateVestingAccount {
            creator: &creator,
            beneficiary: &a.beneficiary.to_account_info(),
            pool_state: &a.pool_state.to_account_info(),
            vesting_record: &a.vesting_record.to_account_info(),
            system_program: &system_program,
            launchlab_program: &a.launchlab_program.to_account_info(),
        },
        share_amount,
        seeds,
    )?;
    let unspent = creator
        .lamports()
        .checked_sub(before)
        .ok_or(LabkitError::Overflow)?;
    if unspent > 0 {
        transfer(
            CpiContext::new_with_signer(
                system_program,
                Transfer {
                    from: creator,
                    to: recipient,
                },
                seeds,
            ),
            unspent,
        )?;
    }
    emit!(CreatorVestingCreated {
        base_mint,
        beneficiary: a.beneficiary.key(),
        share_amount,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct CreateLaunch<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    /// CHECK: The launch's base mint keypair. LaunchLab creates the mint in `initialize_v2`.
    pub base_mint: Signer<'info>,

    #[account(seeds = [CONFIG_SEED], bump = config.bump)]
    pub config: Box<Account<'info, Config>>,

    #[account(
        init,
        payer = payer,
        space = 8 + Launch::INIT_SPACE,
        seeds = [LAUNCH_SEED, base_mint.key().as_ref()],
        bump
    )]
    pub launch: Box<Account<'info, Launch>>,

    /// CHECK: The launch's LaunchLab `creator`. Never initialized.
    #[account(seeds = [CREATOR_SEED, base_mint.key().as_ref()], bump)]
    pub creator: UncheckedAccount<'info>,

    /// CHECK: The platform's fee wallet and the kit's vault. Never initialized.
    #[account(seeds = [KIT_SEED, base_mint.key().as_ref()], bump)]
    pub kit: UncheckedAccount<'info>,

    /// CHECK: The platform's admin. Holds the platform config's rent for one CPI.
    #[account(mut, seeds = [PLATFORM_ADMIN_SEED, base_mint.key().as_ref()], bump)]
    pub platform_admin: UncheckedAccount<'info>,

    /// CHECK: LaunchLab PDA `["platform_config", platform_admin]`, created by LaunchLab.
    #[account(mut)]
    pub platform_config: UncheckedAccount<'info>,

    /// CHECK: Pinned to the template.
    #[account(address = config.platform.nft_wallet)]
    pub nft_wallet: UncheckedAccount<'info>,

    /// CHECK: Pinned to the template. LaunchLab checks it is a CPMM config.
    #[account(address = config.platform.cpswap_config)]
    pub cpswap_config: UncheckedAccount<'info>,

    /// CHECK: The treasury, named as the platform's vesting wallet and transfer fee authority.
    #[account(address = config.treasury)]
    pub treasury: UncheckedAccount<'info>,

    /// CHECK: Address-checked LaunchLab program.
    #[account(address = LAUNCHLAB_PROGRAM_ID)]
    pub launchlab_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RegisterLaunch<'info> {
    #[account(mut, seeds = [LAUNCH_SEED, launch.base_mint.as_ref()], bump = launch.bump)]
    pub launch: Account<'info, Launch>,

    /// CHECK: The launch's LaunchLab `creator`. Never initialized.
    #[account(seeds = [CREATOR_SEED, launch.base_mint.as_ref()], bump = launch.creator_bump)]
    pub creator: UncheckedAccount<'info>,

    /// CHECK: LaunchLab `PoolState`, decoded and checked in the handler.
    #[account(owner = LAUNCHLAB_PROGRAM_ID)]
    pub pool_state: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct SetLaunchRecipient<'info> {
    pub recipient: Signer<'info>,

    #[account(
        mut,
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        has_one = recipient,
    )]
    pub launch: Account<'info, Launch>,
}

#[derive(Accounts)]
pub struct CreateCreatorVesting<'info> {
    #[account(mut)]
    pub recipient: Signer<'info>,

    #[account(
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        has_one = recipient,
        has_one = pool_state @ LabkitError::PoolMismatch,
        constraint = launch.registered @ LabkitError::NotRegistered,
    )]
    pub launch: Account<'info, Launch>,

    /// CHECK: The launch's creator PDA. Signs for LaunchLab and pays the record's rent.
    #[account(
        mut,
        seeds = [CREATOR_SEED, launch.base_mint.as_ref()],
        bump = launch.creator_bump,
    )]
    pub creator: UncheckedAccount<'info>,

    /// CHECK: Chosen by the recipient. LaunchLab fixes it on the vesting record.
    #[account(mut)]
    pub beneficiary: UncheckedAccount<'info>,

    /// CHECK: The launch's LaunchLab `PoolState`, pinned by `has_one`.
    #[account(mut)]
    pub pool_state: UncheckedAccount<'info>,

    /// CHECK: LaunchLab PDA `["pool_vesting", pool_state, beneficiary]`, created by LaunchLab.
    #[account(mut)]
    pub vesting_record: UncheckedAccount<'info>,

    /// CHECK: Address-checked LaunchLab program.
    #[account(address = LAUNCHLAB_PROGRAM_ID)]
    pub launchlab_program: UncheckedAccount<'info>,

    pub system_program: Program<'info, System>,
}
