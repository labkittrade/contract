use anchor_lang::prelude::*;
use anchor_spl::token_interface::{transfer_checked, Mint, TokenAccount, TokenInterface, TransferChecked};

use crate::constants::*;
use crate::errors::LabkitError;
use crate::events::{StakeRewardsClaimed, Staked, Unstaked};
use crate::payout::transfer_signed;
use crate::instructions::draw::{tree_stake, tree_unstake};
use crate::rewards::{settle, sync};
use crate::state::{Launch, StakePool, StakePosition};

/// Stake kit only: opens the launch's stake pool. Anyone can send it, once.
pub fn open_pool(ctx: Context<OpenStakePool>) -> Result<()> {
    let pool = &mut ctx.accounts.stake_pool;
    pool.bump = ctx.bumps.stake_pool;
    pool.base_mint = ctx.accounts.launch.base_mint;
    pool.last_update = Clock::get()?.unix_timestamp;
    Ok(())
}

/// Moves the owner's tokens into the pool. Opens the position the first time.
pub fn stake(ctx: Context<Stake>, amount: u64) -> Result<()> {
    require!(amount > 0, LabkitError::ZeroAmount);
    let now = Clock::get()?.unix_timestamp;
    let base_mint = ctx.accounts.launch.base_mint;
    let owner = ctx.accounts.owner.key();

    let pool = &mut ctx.accounts.stake_pool;
    let position = &mut ctx.accounts.position;
    if position.owner == Pubkey::default() {
        position.bump = ctx.bumps.position;
        position.owner = owner;
        position.base_mint = base_mint;
    }
    position.staked_at = now;
    if ctx.accounts.launch.kit == KIT_DRAW {
        let tree = ctx.accounts.draw_tree.as_ref().ok_or(LabkitError::BadDrawTree)?;
        tree_stake(
            tree,
            &ctx.accounts.owner.to_account_info(),
            &ctx.accounts.system_program.to_account_info(),
            position,
            amount,
        )?;
    } else {
        sync(pool, ctx.accounts.kit_quote.amount, now)?;
        settle(position, pool)?;
    }

    let a = &ctx.accounts;
    transfer_checked(
        CpiContext::new(
            a.token_program.to_account_info(),
            TransferChecked {
                from: a.owner_base.to_account_info(),
                mint: a.base_mint.to_account_info(),
                to: a.vault.to_account_info(),
                authority: a.owner.to_account_info(),
            },
        ),
        amount,
        a.base_mint.decimals,
    )?;

    let pool = &mut ctx.accounts.stake_pool;
    let position = &mut ctx.accounts.position;
    position.amount = position.amount.checked_add(amount).ok_or(LabkitError::Overflow)?;
    pool.total_staked = pool.total_staked.checked_add(amount).ok_or(LabkitError::Overflow)?;
    emit!(Staked {
        base_mint,
        owner,
        amount,
        position: position.amount,
        total_staked: pool.total_staked,
        slot: Clock::get()?.slot,
    });
    Ok(())
}

/// Returns staked tokens to the owner. Earned rewards stay claimable.
pub fn unstake(ctx: Context<Unstake>, amount: u64) -> Result<()> {
    require!(amount > 0, LabkitError::ZeroAmount);
    require!(
        amount <= ctx.accounts.position.amount,
        LabkitError::InsufficientStake
    );
    let now = Clock::get()?.unix_timestamp;
    let pool = &mut ctx.accounts.stake_pool;
    let position = &mut ctx.accounts.position;
    if ctx.accounts.launch.kit == KIT_DRAW {
        let tree = ctx.accounts.draw_tree.as_ref().ok_or(LabkitError::BadDrawTree)?;
        tree_unstake(tree, position, amount)?;
    } else {
        sync(pool, ctx.accounts.kit_quote.amount, now)?;
        settle(position, pool)?;
    }

    let a = &ctx.accounts;
    let base_mint = a.launch.base_mint;
    let bump = [a.stake_pool.bump];
    transfer_signed(
        &a.vault,
        &a.owner_base,
        &a.stake_pool.to_account_info(),
        &a.base_mint,
        &a.token_program,
        &[&[STAKE_POOL_SEED, base_mint.as_ref(), &bump]],
        amount,
    )?;

    let pool = &mut ctx.accounts.stake_pool;
    let position = &mut ctx.accounts.position;
    position.amount -= amount;
    pool.total_staked -= amount;
    emit!(Unstaked {
        base_mint,
        owner: position.owner,
        amount,
        position: position.amount,
        total_staked: pool.total_staked,
        slot: Clock::get()?.slot,
    });
    Ok(())
}

/// Pays the owner everything the position has earned, or won in draws, in WSOL.
pub fn claim_rewards(ctx: Context<ClaimStakeRewards>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let pool = &mut ctx.accounts.stake_pool;
    let position = &mut ctx.accounts.position;
    if ctx.accounts.launch.kit != KIT_DRAW {
        sync(pool, ctx.accounts.kit_quote.amount, now)?;
        settle(position, pool)?;
    }
    let amount = position.earned;
    require!(amount > 0, LabkitError::NothingToClaim);

    let a = &ctx.accounts;
    let base_mint = a.launch.base_mint;
    let bump = [a.launch.kit_bump];
    transfer_signed(
        &a.kit_quote,
        &a.owner_quote,
        &a.kit.to_account_info(),
        &a.quote_mint,
        &a.token_program,
        &[&[KIT_SEED, base_mint.as_ref(), &bump]],
        amount,
    )?;

    let pool = &mut ctx.accounts.stake_pool;
    let position = &mut ctx.accounts.position;
    position.earned = 0;
    position.claimed = position.claimed.checked_add(amount).ok_or(LabkitError::Overflow)?;
    pool.claimed = pool.claimed.checked_add(amount).ok_or(LabkitError::Overflow)?;
    emit!(StakeRewardsClaimed {
        base_mint,
        owner: position.owner,
        lamports: amount,
        slot: Clock::get()?.slot,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct OpenStakePool<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        constraint = launch.kit == KIT_STAKE @ LabkitError::WrongKit,
        constraint = launch.registered @ LabkitError::NotRegistered,
    )]
    pub launch: Box<Account<'info, Launch>>,

    #[account(
        init,
        payer = payer,
        space = 8 + StakePool::INIT_SPACE,
        seeds = [STAKE_POOL_SEED, launch.base_mint.as_ref()],
        bump,
    )]
    pub stake_pool: Box<Account<'info, StakePool>>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Stake<'info> {
    /// Pays for the position the first time.
    #[account(mut)]
    pub owner: Signer<'info>,

    #[account(
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        has_one = base_mint,
    )]
    pub launch: Box<Account<'info, Launch>>,

    /// Only Stake and Draw launches have one, so it stands in for checking the kit.
    #[account(mut, seeds = [STAKE_POOL_SEED, launch.base_mint.as_ref()], bump = stake_pool.bump)]
    pub stake_pool: Box<Account<'info, StakePool>>,

    /// CHECK: Draw only: the launch's draw tree, checked by owner, discriminator and seeds.
    #[account(mut)]
    pub draw_tree: Option<UncheckedAccount<'info>>,

    #[account(
        init_if_needed,
        payer = owner,
        space = 8 + StakePosition::INIT_SPACE,
        seeds = [STAKE_SEED, launch.base_mint.as_ref(), owner.key().as_ref()],
        bump,
    )]
    pub position: Box<Account<'info, StakePosition>>,

    #[account(mint::token_program = token_program)]
    pub base_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        token::mint = base_mint,
        token::authority = owner,
        token::token_program = token_program,
    )]
    pub owner_base: Box<InterfaceAccount<'info, TokenAccount>>,

    /// The client creates it with the associated token program's idempotent create.
    #[account(
        mut,
        associated_token::mint = base_mint,
        associated_token::authority = stake_pool,
        associated_token::token_program = token_program,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: The kit PDA, which owns `kit_quote`.
    #[account(seeds = [KIT_SEED, launch.base_mint.as_ref()], bump = launch.kit_bump)]
    pub kit: UncheckedAccount<'info>,

    /// CHECK: Address-checked WSOL mint.
    #[account(address = WSOL_MINT)]
    pub quote_mint: UncheckedAccount<'info>,

    /// The client creates it the same way if no fees have reached the kit yet.
    #[account(
        associated_token::mint = quote_mint,
        associated_token::authority = kit,
        associated_token::token_program = token_program,
    )]
    pub kit_quote: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct Unstake<'info> {
    pub owner: Signer<'info>,

    #[account(
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        has_one = base_mint,
    )]
    pub launch: Box<Account<'info, Launch>>,

    #[account(mut, seeds = [STAKE_POOL_SEED, launch.base_mint.as_ref()], bump = stake_pool.bump)]
    pub stake_pool: Box<Account<'info, StakePool>>,

    /// CHECK: Draw only: the launch's draw tree, checked by owner, discriminator and seeds.
    #[account(mut)]
    pub draw_tree: Option<UncheckedAccount<'info>>,

    #[account(
        mut,
        seeds = [STAKE_SEED, launch.base_mint.as_ref(), owner.key().as_ref()],
        bump = position.bump,
    )]
    pub position: Box<Account<'info, StakePosition>>,

    #[account(mint::token_program = token_program)]
    pub base_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        mut,
        token::mint = base_mint,
        token::authority = owner,
        token::token_program = token_program,
    )]
    pub owner_base: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(
        mut,
        associated_token::mint = base_mint,
        associated_token::authority = stake_pool,
        associated_token::token_program = token_program,
    )]
    pub vault: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: The kit PDA, which owns `kit_quote`.
    #[account(seeds = [KIT_SEED, launch.base_mint.as_ref()], bump = launch.kit_bump)]
    pub kit: UncheckedAccount<'info>,

    #[account(address = WSOL_MINT, mint::token_program = token_program)]
    pub quote_mint: Box<InterfaceAccount<'info, Mint>>,

    #[account(
        associated_token::mint = quote_mint,
        associated_token::authority = kit,
        associated_token::token_program = token_program,
    )]
    pub kit_quote: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct ClaimStakeRewards<'info> {
    pub owner: Signer<'info>,

    #[account(seeds = [LAUNCH_SEED, launch.base_mint.as_ref()], bump = launch.bump)]
    pub launch: Box<Account<'info, Launch>>,

    #[account(mut, seeds = [STAKE_POOL_SEED, launch.base_mint.as_ref()], bump = stake_pool.bump)]
    pub stake_pool: Box<Account<'info, StakePool>>,

    #[account(
        mut,
        seeds = [STAKE_SEED, launch.base_mint.as_ref(), owner.key().as_ref()],
        bump = position.bump,
    )]
    pub position: Box<Account<'info, StakePosition>>,

    /// CHECK: The kit PDA. Signs the payout from `kit_quote`.
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
        token::authority = owner,
        token::token_program = token_program,
    )]
    pub owner_quote: Box<InterfaceAccount<'info, TokenAccount>>,

    pub token_program: Interface<'info, TokenInterface>,
}
