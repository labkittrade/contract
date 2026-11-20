use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token_interface::{burn, Burn, Mint, TokenAccount, TokenInterface};

use crate::constants::*;
use crate::cpmm::{cpmm_fee_rate, decode_cpmm_pool, swap_base_input, CpmmPoolView, SwapBaseInput};
use crate::errors::LabkitError;
use crate::events::{FloorCheckpoint, GraduateReleased, KitExecuted};
use crate::launchlab::{
    buy_exact_in, curve_fee_rate, decode_pool_state, platform_cpswap_config, u64_at, BuyExactIn,
};
use crate::payout::transfer_signed;
use crate::policy::{floor_line, min_out, plan_buy, price_q64, raise_high, Market};
use crate::state::Launch;

const VENUE_CURVE: u8 = 0;
const VENUE_CPMM: u8 = 1;
const TOKEN_AMOUNT_OFFSET: usize = 64;

/// Buys on the bonding curve with the kit's WSOL and burns what it bought. Burn and
/// Graduate spend whatever the kit holds, Floor only while the price is under its line.
pub fn execute_curve(ctx: Context<ExecuteCurve>) -> Result<()> {
    let kit = ctx.accounts.launch.kit;
    require!(
        matches!(kit, KIT_BURN | KIT_FLOOR | KIT_GRADUATE),
        LabkitError::WrongKit
    );
    once_per_slot(&ctx.accounts.launch)?;
    let pool = decode_pool_state(&ctx.accounts.pool_state.try_borrow_data()?)?;
    require!(pool.status == POOL_STATUS_TRADING, LabkitError::CurveNotTrading);
    let global_config = &ctx.accounts.global_config;
    require!(
        global_config.owner == &LAUNCHLAB_PROGRAM_ID && global_config.key() == pool.global_config,
        LabkitError::BadPoolState
    );
    let fee_rate = curve_fee_rate(
        &global_config.try_borrow_data()?,
        &ctx.accounts.platform_config.try_borrow_data()?,
    )?;
    let (quote, base) = pool.reserves()?;
    let market = Market {
        quote,
        base,
        fee_rate,
        base_left: Some(pool.base_left()),
    };
    let price = price_q64(quote, base)?;
    let floor = note_price(&mut ctx.accounts.launch, price)?;
    let spend = plan_buy(&market, ctx.accounts.kit_quote.amount, floor)?;
    let minimum = min_out(&market, spend)?;

    let quote_before = ctx.accounts.kit_quote.amount;
    let a = &ctx.accounts;
    let base_mint = a.launch.base_mint;
    let bump = [a.launch.kit_bump];
    let seeds: &[&[&[u8]]] = &[&[KIT_SEED, base_mint.as_ref(), &bump]];
    buy_exact_in(
        BuyExactIn {
            payer: &a.kit.to_account_info(),
            authority: &a.launchlab_authority.to_account_info(),
            global_config: &a.global_config.to_account_info(),
            platform_config: &a.platform_config.to_account_info(),
            pool_state: &a.pool_state.to_account_info(),
            user_base_token: &a.kit_base.to_account_info(),
            user_quote_token: &a.kit_quote.to_account_info(),
            base_vault: &a.base_vault.to_account_info(),
            quote_vault: &a.quote_vault.to_account_info(),
            base_token_mint: &a.base_mint.to_account_info(),
            quote_token_mint: &a.quote_mint.to_account_info(),
            base_token_program: &a.token_program.to_account_info(),
            quote_token_program: &a.token_program.to_account_info(),
            event_authority: &a.event_authority.to_account_info(),
            launchlab_program: &a.launchlab_program.to_account_info(),
            system_program: &a.system_program.to_account_info(),
            platform_fee_vault: &a.platform_fee_vault.to_account_info(),
            creator_fee_vault: &a.creator_fee_vault.to_account_info(),
        },
        spend,
        minimum,
        seeds,
    )?;

    let a = &mut *ctx.accounts;
    a.kit_quote.reload()?;
    a.kit_base.reload()?;
    finish(
        &mut a.launch,
        Bought {
            kit: &a.kit,
            kit_base: &a.kit_base,
            base_mint: &a.base_mint,
            token_program: &a.token_program,
            crank: a.crank.key(),
        },
        quote_before.saturating_sub(a.kit_quote.amount),
        spend,
        VENUE_CURVE,
        price,
    )
}

/// Buys on the launch's Raydium CPMM pool and burns what it bought. Burn always,
/// Floor only under its line. Graduate never buys on Raydium.
pub fn execute_swap(ctx: Context<ExecuteSwap>) -> Result<()> {
    let kit = ctx.accounts.launch.kit;
    require!(matches!(kit, KIT_BURN | KIT_FLOOR), LabkitError::WrongKit);
    once_per_slot(&ctx.accounts.launch)?;
    let pool = decode_pool_state(&ctx.accounts.pool_state.try_borrow_data()?)?;
    require!(pool.status == POOL_STATUS_MIGRATED, LabkitError::NotMigrated);
    let a = &ctx.accounts;
    let view = decode_cpmm_pool(&a.cpmm_pool)?;
    require_keys_eq!(view.observation, a.observation.key(), LabkitError::BadAmmPool);
    let market = amm_market(
        &view,
        &a.creator.key(),
        &a.launch.base_mint,
        &a.platform_config,
        &a.quote_vault,
        &a.base_vault,
        cpmm_fee_rate(&a.amm_config, &view)?,
    )?;
    let price = price_q64(market.quote, market.base)?;
    let floor = note_price(&mut ctx.accounts.launch, price)?;
    let spend = plan_buy(&market, ctx.accounts.kit_quote.amount, floor)?;
    let minimum = min_out(&market, spend)?;

    let quote_before = ctx.accounts.kit_quote.amount;
    let a = &ctx.accounts;
    let base_mint = a.launch.base_mint;
    let bump = [a.launch.kit_bump];
    let seeds: &[&[&[u8]]] = &[&[KIT_SEED, base_mint.as_ref(), &bump]];
    swap_base_input(
        SwapBaseInput {
            payer: &a.kit.to_account_info(),
            authority: &a.cpmm_authority.to_account_info(),
            amm_config: &a.amm_config.to_account_info(),
            pool_state: &a.cpmm_pool.to_account_info(),
            input_token_account: &a.kit_quote.to_account_info(),
            output_token_account: &a.kit_base.to_account_info(),
            input_vault: &a.quote_vault.to_account_info(),
            output_vault: &a.base_vault.to_account_info(),
            input_token_program: &a.token_program.to_account_info(),
            output_token_program: &a.token_program.to_account_info(),
            input_token_mint: &a.quote_mint.to_account_info(),
            output_token_mint: &a.base_mint.to_account_info(),
            observation_state: &a.observation.to_account_info(),
            cpmm_program: &a.cpmm_program.to_account_info(),
        },
        spend,
        minimum,
        seeds,
    )?;

    let a = &mut *ctx.accounts;
    a.kit_quote.reload()?;
    a.kit_base.reload()?;
    finish(
        &mut a.launch,
        Bought {
            kit: &a.kit,
            kit_base: &a.kit_base,
            base_mint: &a.base_mint,
            token_program: &a.token_program,
            crank: a.crank.key(),
        },
        quote_before.saturating_sub(a.kit_quote.amount),
        spend,
        VENUE_CPMM,
        price,
    )
}

/// Raises Floor's high toward the current price, as far as `raise_high` allows. On
/// Raydium, the remaining accounts are the CPMM pool, its AMM config, its WSOL vault and
/// its token vault.
pub fn checkpoint<'info>(ctx: Context<'_, '_, '_, 'info, Checkpoint<'info>>) -> Result<()> {
    require!(ctx.accounts.launch.kit == KIT_FLOOR, LabkitError::WrongKit);
    let pool = decode_pool_state(&ctx.accounts.pool_state.try_borrow_data()?)?;
    let price = match pool.status {
        POOL_STATUS_TRADING => {
            require!(ctx.remaining_accounts.is_empty(), LabkitError::BadAmmPool);
            let (quote, base) = pool.reserves()?;
            price_q64(quote, base)?
        }
        POOL_STATUS_MIGRATED => {
            let [cpmm_pool, amm_config, quote_vault, base_vault] = ctx.remaining_accounts else {
                return err!(LabkitError::BadAmmPool);
            };
            let view = decode_cpmm_pool(cpmm_pool)?;
            let fee_rate = cpmm_fee_rate(amm_config, &view)?;
            let market = amm_market(
                &view,
                &ctx.accounts.creator.key(),
                &ctx.accounts.launch.base_mint,
                &ctx.accounts.platform_config,
                quote_vault,
                base_vault,
                fee_rate,
            )?;
            price_q64(market.quote, market.base)?
        }
        _ => return err!(LabkitError::NotMigrated),
    };
    let launch = &mut ctx.accounts.launch;
    note_price(launch, price)?;
    emit!(FloorCheckpoint {
        base_mint: launch.base_mint,
        price_q64: price,
        high_q64: launch.high_q64,
        slot: Clock::get()?.slot,
    });
    Ok(())
}

/// Graduate only: once the curve has completed, pays the kit's WSOL to the recipient.
pub fn release(ctx: Context<Release>) -> Result<()> {
    require!(ctx.accounts.launch.kit == KIT_GRADUATE, LabkitError::WrongKit);
    let pool = decode_pool_state(&ctx.accounts.pool_state.try_borrow_data()?)?;
    require!(pool.status != POOL_STATUS_TRADING, LabkitError::NotGraduated);
    let amount = ctx.accounts.kit_quote.amount;
    require!(amount > 0, LabkitError::NothingToSplit);

    let a = &ctx.accounts;
    let base_mint = a.launch.base_mint;
    let bump = [a.launch.kit_bump];
    transfer_signed(
        &a.kit_quote,
        &a.recipient_token,
        &a.kit.to_account_info(),
        &a.quote_mint,
        &a.token_program,
        &[&[KIT_SEED, base_mint.as_ref(), &bump]],
        amount,
    )?;
    let launch = &mut ctx.accounts.launch;
    launch.released = launch
        .released
        .checked_add(amount)
        .ok_or(LabkitError::Overflow)?;
    emit!(GraduateReleased {
        base_mint,
        recipient: launch.recipient,
        lamports: amount,
        slot: Clock::get()?.slot,
    });
    Ok(())
}

/// A kit buys at most once per slot, so a bundle around it moves at most one capped buy.
fn once_per_slot(launch: &Launch) -> Result<()> {
    require!(
        Clock::get()?.slot > launch.last_execution_slot,
        LabkitError::ExecutedThisSlot
    );
    Ok(())
}

/// Raises Floor's high toward `price` and returns its line. `None` for other kits.
fn note_price(launch: &mut Launch, price: u128) -> Result<Option<u128>> {
    if launch.kit != KIT_FLOOR {
        return Ok(None);
    }
    let now = Clock::get()?.unix_timestamp;
    let high = raise_high(launch.high_q64, launch.high_at, price, now);
    if high > launch.high_q64 {
        launch.high_q64 = high;
        launch.high_at = now;
    }
    Ok(Some(floor_line(launch.high_q64, launch.drawdown_bps)))
}

/// The pool's reserves, net of booked fees. The pool must be the launch's: it names the
/// creator PDA as its creator, which only LaunchLab migration or a pool Raydium permits
/// can do, and it uses the `AmmConfig` the launch's platform graduates into.
fn amm_market(
    view: &CpmmPoolView,
    creator: &Pubkey,
    base_mint: &Pubkey,
    platform_config: &AccountInfo,
    quote_vault: &AccountInfo,
    base_vault: &AccountInfo,
    fee_rate: u64,
) -> Result<Market> {
    require_keys_eq!(view.creator, *creator, LabkitError::BadAmmPool);
    require_keys_eq!(
        view.amm_config,
        platform_cpswap_config(&platform_config.try_borrow_data()?)?,
        LabkitError::BadAmmPool
    );
    let quote_side = if view.mints[0] == WSOL_MINT { 0 } else { 1 };
    let base_side = 1 - quote_side;
    require!(
        view.mints[quote_side] == WSOL_MINT
            && view.mints[base_side] == *base_mint
            && view.vaults[quote_side] == quote_vault.key()
            && view.vaults[base_side] == base_vault.key(),
        LabkitError::BadAmmPool
    );
    let reserve = |vault: &AccountInfo, side: usize| -> Result<u64> {
        let data = vault.try_borrow_data()?;
        require!(data.len() >= TOKEN_AMOUNT_OFFSET + 8, LabkitError::BadAmmPool);
        u64_at(&data, TOKEN_AMOUNT_OFFSET)
            .checked_sub(view.booked[side])
            .ok_or_else(|| error!(LabkitError::BadAmmPool))
    };
    Ok(Market {
        quote: reserve(quote_vault, quote_side)?,
        base: reserve(base_vault, base_side)?,
        fee_rate,
        base_left: None,
    })
}

struct Bought<'a, 'info> {
    kit: &'a UncheckedAccount<'info>,
    kit_base: &'a InterfaceAccount<'info, TokenAccount>,
    base_mint: &'a InterfaceAccount<'info, Mint>,
    token_program: &'a Interface<'info, TokenInterface>,
    crank: Pubkey,
}

/// Burns everything in the kit's token account and books the buy.
fn finish(
    launch: &mut Launch,
    bought: Bought,
    spent: u64,
    planned: u64,
    venue: u8,
    price: u128,
) -> Result<()> {
    require!(spent > 0 && spent <= planned, LabkitError::Overspend);
    let burned = bought.kit_base.amount;
    let bump = [launch.kit_bump];
    burn(
        CpiContext::new_with_signer(
            bought.token_program.to_account_info(),
            Burn {
                mint: bought.base_mint.to_account_info(),
                from: bought.kit_base.to_account_info(),
                authority: bought.kit.to_account_info(),
            },
            &[&[KIT_SEED, launch.base_mint.as_ref(), &bump]],
        ),
        burned,
    )?;
    let slot = Clock::get()?.slot;
    launch.spent = launch.spent.checked_add(spent).ok_or(LabkitError::Overflow)?;
    launch.burned = launch.burned.checked_add(burned).ok_or(LabkitError::Overflow)?;
    launch.executions = launch.executions.checked_add(1).ok_or(LabkitError::Overflow)?;
    launch.last_execution_slot = slot;
    emit!(KitExecuted {
        base_mint: launch.base_mint,
        kit: launch.kit,
        venue,
        lamports: spent,
        burned,
        price_q64: price,
        crank: bought.crank,
        slot,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct ExecuteCurve<'info> {
    /// Pays for the kit's token account the first time.
    #[account(mut)]
    pub crank: Signer<'info>,

    #[account(
        mut,
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        has_one = base_mint,
        has_one = pool_state @ LabkitError::PoolMismatch,
        has_one = platform_config @ LabkitError::PlatformMismatch,
        constraint = launch.registered @ LabkitError::NotRegistered,
    )]
    pub launch: Box<Account<'info, Launch>>,

    /// CHECK: The kit PDA. Signs the buy and the burn.
    #[account(mut, seeds = [KIT_SEED, launch.base_mint.as_ref()], bump = launch.kit_bump)]
    pub kit: UncheckedAccount<'info>,

    #[account(mut, mint::token_program = token_program)]
    pub base_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(address = WSOL_MINT, mint::token_program = token_program)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        init_if_needed,
        payer = crank,
        associated_token::mint = base_mint,
        associated_token::authority = kit,
        associated_token::token_program = token_program,
    )]
    pub kit_base: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        associated_token::mint = quote_mint,
        associated_token::authority = kit,
        associated_token::token_program = token_program,
    )]
    pub kit_quote: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: The launch's LaunchLab `PoolState`, pinned by `has_one`.
    #[account(mut)]
    pub pool_state: UncheckedAccount<'info>,

    /// CHECK: LaunchLab `GlobalConfig`, checked against the pool in the handler.
    pub global_config: UncheckedAccount<'info>,

    /// CHECK: The launch's LaunchLab `PlatformConfig`, pinned by `has_one`.
    pub platform_config: UncheckedAccount<'info>,

    /// CHECK: LaunchLab PDA `["vault_auth_seed"]`, checked by LaunchLab.
    pub launchlab_authority: UncheckedAccount<'info>,

    /// CHECK: The pool's base vault, checked by LaunchLab.
    #[account(mut)]
    pub base_vault: UncheckedAccount<'info>,

    /// CHECK: The pool's quote vault, checked by LaunchLab.
    #[account(mut)]
    pub quote_vault: UncheckedAccount<'info>,

    /// CHECK: LaunchLab's event authority, checked by LaunchLab.
    pub event_authority: UncheckedAccount<'info>,

    /// CHECK: LaunchLab PDA `[platform_config, quote_mint]`, checked by LaunchLab.
    #[account(mut)]
    pub platform_fee_vault: UncheckedAccount<'info>,

    /// CHECK: LaunchLab PDA `[creator, quote_mint]`, checked by LaunchLab.
    #[account(mut)]
    pub creator_fee_vault: UncheckedAccount<'info>,

    /// CHECK: Address-checked LaunchLab program.
    #[account(address = LAUNCHLAB_PROGRAM_ID)]
    pub launchlab_program: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ExecuteSwap<'info> {
    /// Pays for the kit's token account the first time.
    #[account(mut)]
    pub crank: Signer<'info>,

    #[account(
        mut,
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        has_one = base_mint,
        has_one = pool_state @ LabkitError::PoolMismatch,
        has_one = platform_config @ LabkitError::PlatformMismatch,
        constraint = launch.registered @ LabkitError::NotRegistered,
    )]
    pub launch: Box<Account<'info, Launch>>,

    /// CHECK: The launch's LaunchLab `PlatformConfig`, pinned by `has_one`. Names the
    /// `AmmConfig` the pool must use.
    pub platform_config: UncheckedAccount<'info>,

    /// CHECK: The kit PDA. Signs the swap and the burn.
    #[account(mut, seeds = [KIT_SEED, launch.base_mint.as_ref()], bump = launch.kit_bump)]
    pub kit: UncheckedAccount<'info>,

    /// CHECK: The launch's creator PDA, which every LaunchLab-migrated pool names.
    #[account(seeds = [CREATOR_SEED, launch.base_mint.as_ref()], bump = launch.creator_bump)]
    pub creator: UncheckedAccount<'info>,

    #[account(mut, mint::token_program = token_program)]
    pub base_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(address = WSOL_MINT, mint::token_program = token_program)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        init_if_needed,
        payer = crank,
        associated_token::mint = base_mint,
        associated_token::authority = kit,
        associated_token::token_program = token_program,
    )]
    pub kit_base: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        associated_token::mint = quote_mint,
        associated_token::authority = kit,
        associated_token::token_program = token_program,
    )]
    pub kit_quote: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: The launch's LaunchLab `PoolState`, pinned by `has_one`.
    pub pool_state: UncheckedAccount<'info>,

    /// CHECK: CPMM `PoolState`, decoded and checked in the handler.
    #[account(mut)]
    pub cpmm_pool: UncheckedAccount<'info>,

    /// CHECK: The pool's `AmmConfig`, checked in the handler.
    pub amm_config: UncheckedAccount<'info>,

    /// CHECK: CPMM PDA `["vault_and_lp_mint_auth_seed"]`, checked by CPMM.
    pub cpmm_authority: UncheckedAccount<'info>,

    /// CHECK: The pool's WSOL vault, checked in the handler.
    #[account(mut)]
    pub quote_vault: UncheckedAccount<'info>,

    /// CHECK: The pool's token vault, checked in the handler.
    #[account(mut)]
    pub base_vault: UncheckedAccount<'info>,

    /// CHECK: The pool's observation account, checked in the handler.
    #[account(mut)]
    pub observation: UncheckedAccount<'info>,

    /// CHECK: Address-checked CPMM program.
    #[account(address = CPMM_PROGRAM_ID)]
    pub cpmm_program: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Checkpoint<'info> {
    pub crank: Signer<'info>,

    #[account(
        mut,
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        has_one = pool_state @ LabkitError::PoolMismatch,
        has_one = platform_config @ LabkitError::PlatformMismatch,
        constraint = launch.registered @ LabkitError::NotRegistered,
    )]
    pub launch: Account<'info, Launch>,

    /// CHECK: The launch's LaunchLab `PoolState`, pinned by `has_one`.
    pub pool_state: UncheckedAccount<'info>,

    /// CHECK: The launch's LaunchLab `PlatformConfig`, pinned by `has_one`. Names the
    /// `AmmConfig` a Raydium pool must use.
    pub platform_config: UncheckedAccount<'info>,

    /// CHECK: The launch's creator PDA, which every LaunchLab-migrated pool names.
    #[account(seeds = [CREATOR_SEED, launch.base_mint.as_ref()], bump = launch.creator_bump)]
    pub creator: UncheckedAccount<'info>,
}

#[derive(Accounts)]
pub struct Release<'info> {
    pub crank: Signer<'info>,

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

    /// CHECK: The kit PDA. Signs the transfer out of its WSOL account.
    #[account(seeds = [KIT_SEED, launch.base_mint.as_ref()], bump = launch.kit_bump)]
    pub kit: UncheckedAccount<'info>,

    #[account(address = WSOL_MINT, mint::token_program = token_program)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
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

    pub token_program: Interface<'info, TokenInterface>,
}
