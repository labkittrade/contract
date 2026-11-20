use anchor_lang::prelude::*;
use anchor_lang::solana_program::hash::hashv;
use anchor_lang::system_program::{transfer, Transfer};
use anchor_lang::Discriminator;
use anchor_spl::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::constants::*;
use crate::draw_tree;
use crate::errors::LabkitError;
use crate::events::{DrawCancelled, DrawRequested, DrawSettled};
use crate::payout::transfer_signed;
use crate::state::{Draw, DrawTree, Launch, StakePool, StakePosition};
use crate::vrf;

/// Draw kit only: opens the launch's stake pool, draw tree and schedule. Anyone can send
/// it, once. The first draw is due a full interval later.
pub fn open(ctx: Context<OpenDraw>) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let base_mint = ctx.accounts.launch.base_mint;
    let pool = &mut ctx.accounts.stake_pool;
    pool.bump = ctx.bumps.stake_pool;
    pool.base_mint = base_mint;
    pool.last_update = now;
    let draw = &mut ctx.accounts.draw;
    draw.bump = ctx.bumps.draw;
    draw.base_mint = base_mint;
    draw.last_draw = now;
    ctx.accounts.draw_tree.bump = ctx.bumps.draw_tree;
    Ok(())
}

/// Checks that `tree` is the launch's draw tree and returns it unfrozen.
fn open_tree<'a, 'info>(tree: &'a AccountInfo<'info>, base_mint: &Pubkey) -> Result<&'a AccountInfo<'info>> {
    require_keys_eq!(*tree.owner, crate::ID, LabkitError::BadDrawTree);
    let data = tree.try_borrow_data()?;
    require!(
        data.len() >= draw_tree::HEADER_LEN && data[..8] == DrawTree::DISCRIMINATOR,
        LabkitError::BadDrawTree
    );
    let expected = Pubkey::create_program_address(&[DRAW_TREE_SEED, base_mint.as_ref(), &[data[8]]], &crate::ID)
        .map_err(|_| error!(LabkitError::BadDrawTree))?;
    require_keys_eq!(tree.key(), expected, LabkitError::BadDrawTree);
    require!(!draw_tree::frozen(&data), LabkitError::DrawInProgress);
    Ok(tree)
}

/// Adds a stake to the tree, giving the position its index the first time. The owner
/// pays the rent for the tree's new entry.
pub(crate) fn tree_stake<'info>(
    tree: &AccountInfo<'info>,
    owner: &AccountInfo<'info>,
    system_program: &AccountInfo<'info>,
    position: &mut StakePosition,
    amount: u64,
) -> Result<()> {
    let tree = open_tree(tree, &position.base_mint)?;
    if position.slot != 0 {
        let mut data = tree.try_borrow_mut_data()?;
        let len = draw_tree::len(&data);
        return draw_tree::add(&mut data[draw_tree::HEADER_LEN..], len, position.slot as usize, amount);
    }
    let len = draw_tree::len(&tree.try_borrow_data()?);
    let space = draw_tree::space(len + 1);
    let rent = Rent::get()?.minimum_balance(space);
    if tree.lamports() < rent {
        transfer(
            CpiContext::new(system_program.clone(), Transfer { from: owner.clone(), to: tree.clone() }),
            rent - tree.lamports(),
        )?;
    }
    tree.realloc(space, false)?;
    let mut data = tree.try_borrow_mut_data()?;
    let slot = draw_tree::push(&mut data[draw_tree::HEADER_LEN..], len, amount)?;
    draw_tree::set_len(&mut data, slot);
    position.slot = slot as u32;
    Ok(())
}

pub(crate) fn tree_unstake(tree: &AccountInfo, position: &StakePosition, amount: u64) -> Result<()> {
    let tree = open_tree(tree, &position.base_mint)?;
    let mut data = tree.try_borrow_mut_data()?;
    let len = draw_tree::len(&data);
    draw_tree::sub(&mut data[draw_tree::HEADER_LEN..], len, position.slot as usize, amount)
}

/// Lamports in the kit that no winner is owed yet.
fn pot(kit_balance: u64, pool: &StakePool) -> u64 {
    kit_balance.saturating_sub(pool.distributed - pool.claimed)
}

/// Asks ORAO for randomness once the draw is due and the pot is big enough, and freezes
/// stakes until it is settled. The pot refunds the keeper in WSOL for the VRF fee and the
/// rent the request keeps once fulfilled; ORAO itself returns the rest of the rent to the
/// keeper when it answers.
/// The keeper picks `seed`; without ORAO's keys it can't tell what any seed will yield,
/// and ORAO rejects a seed it has seen, so old randomness can't be replayed.
pub fn request(ctx: Context<RequestDraw>, seed: [u8; 32]) -> Result<()> {
    let now = Clock::get()?.unix_timestamp;
    let draw = &ctx.accounts.draw;
    require!(!draw.pending, LabkitError::DrawInProgress);
    require!(now >= draw.last_draw + DRAW_INTERVAL_SECONDS, LabkitError::DrawNotDue);
    require!(ctx.accounts.stake_pool.total_staked > 0, LabkitError::NoStakers);
    require!(
        pot(ctx.accounts.kit_quote.amount, &ctx.accounts.stake_pool) >= DRAW_MIN_POT_LAMPORTS,
        LabkitError::PotTooSmall
    );

    let base_mint = ctx.accounts.launch.base_mint;
    let a = &ctx.accounts;
    let keeper = a.keeper.to_account_info();
    let before = keeper.lamports();
    vrf::request(
        &a.vrf.to_account_info(),
        &keeper,
        &a.network_state.to_account_info(),
        &a.treasury.to_account_info(),
        &a.randomness.to_account_info(),
        &a.system_program.to_account_info(),
        seed,
    )?;
    let returned = a
        .randomness
        .lamports()
        .saturating_sub(Rent::get()?.minimum_balance(ORAO_FULFILLED_LEN));
    let cost = before.saturating_sub(keeper.lamports()).saturating_sub(returned);
    require!(cost <= DRAW_MAX_COST_LAMPORTS, LabkitError::DrawTooCostly);
    let bump = [a.launch.kit_bump];
    transfer_signed(
        &a.kit_quote,
        &a.keeper_quote,
        &a.kit.to_account_info(),
        &a.quote_mint,
        &a.token_program,
        &[&[KIT_SEED, base_mint.as_ref(), &bump]],
        cost,
    )?;

    draw_tree::set_frozen(&mut ctx.accounts.draw_tree.try_borrow_mut_data()?, true);
    let draw = &mut ctx.accounts.draw;
    draw.pending = true;
    draw.seed = seed;
    draw.requested_at = now;
    emit!(DrawRequested { base_mint, seed, cost, slot: Clock::get()?.slot });
    Ok(())
}

/// A point in `0..total` for pick `n`, uniform up to a 2^-64 bias.
fn pick(randomness: &[u8; 64], n: usize, total: u64) -> u64 {
    let hash = hashv(&[randomness, &[n as u8]]).to_bytes();
    (u128::from_le_bytes(hash[..16].try_into().unwrap()) % total as u128) as u64
}

/// Settles a draw once ORAO has answered. Pick `n` is a point in the total stake drawn
/// from the randomness; its owner wins if their stake is old enough, else pick `n + 1`
/// is tried. The remaining accounts are the picked positions, in order, up to the
/// winner; anyone can work them out from the randomness and the tree. If all
/// `DRAW_PICKS` picks are too new, the pot rolls over.
pub fn settle<'info>(ctx: Context<'_, '_, 'info, 'info, SettleDraw<'info>>) -> Result<()> {
    let draw = &ctx.accounts.draw;
    require!(draw.pending, LabkitError::NoDrawPending);
    let randomness = vrf::fulfilled(&ctx.accounts.randomness, &draw.seed)?.ok_or(LabkitError::NotFulfilled)?;
    let base_mint = ctx.accounts.launch.base_mint;
    let eligible_before = draw.requested_at - DRAW_MIN_STAKE_AGE_SECONDS;
    let prize = pot(ctx.accounts.kit_quote.amount, &ctx.accounts.stake_pool);

    let mut winner = Pubkey::default();
    {
        let data = ctx.accounts.draw_tree.try_borrow_data()?;
        let len = draw_tree::len(&data);
        let entries = &data[draw_tree::HEADER_LEN..];
        let total = draw_tree::prefix(entries, len);
        require!(total > 0, LabkitError::NoStakers);
        for n in 0..DRAW_PICKS {
            let slot = draw_tree::find(entries, len, pick(&randomness, n, total))?;
            let info = ctx.remaining_accounts.get(n).ok_or(LabkitError::BadCandidate)?;
            let mut position = Account::<StakePosition>::try_from(info)?;
            require!(
                position.base_mint == base_mint && position.slot as usize == slot,
                LabkitError::BadCandidate
            );
            if position.staked_at <= eligible_before {
                position.earned = position.earned.checked_add(prize).ok_or(LabkitError::Overflow)?;
                position.exit(&crate::ID)?;
                winner = position.owner;
                break;
            }
        }
    }

    let won = winner != Pubkey::default();
    if won {
        let pool = &mut ctx.accounts.stake_pool;
        pool.distributed = pool.distributed.checked_add(prize).ok_or(LabkitError::Overflow)?;
    }
    draw_tree::set_frozen(&mut ctx.accounts.draw_tree.try_borrow_mut_data()?, false);
    let draw = &mut ctx.accounts.draw;
    draw.pending = false;
    draw.round += 1;
    draw.last_draw = Clock::get()?.unix_timestamp;
    draw.last_winner = winner;
    draw.last_prize = if won { prize } else { 0 };
    if won {
        draw.won = draw.won.checked_add(prize).ok_or(LabkitError::Overflow)?;
    }
    emit!(DrawSettled {
        base_mint,
        round: draw.round,
        winner,
        prize: draw.last_prize,
        slot: Clock::get()?.slot,
    });
    Ok(())
}

/// Drops a request ORAO never answered, after `DRAW_TIMEOUT_SECONDS`, and unfreezes
/// stakes. An answered request can only be settled, so nobody can discard an outcome.
pub fn cancel(ctx: Context<CancelDraw>) -> Result<()> {
    let draw = &ctx.accounts.draw;
    require!(draw.pending, LabkitError::NoDrawPending);
    require!(
        Clock::get()?.unix_timestamp >= draw.requested_at + DRAW_TIMEOUT_SECONDS,
        LabkitError::NotTimedOut
    );
    require!(
        vrf::fulfilled(&ctx.accounts.randomness, &draw.seed)?.is_none(),
        LabkitError::AlreadyFulfilled
    );
    draw_tree::set_frozen(&mut ctx.accounts.draw_tree.try_borrow_mut_data()?, false);
    let draw = &mut ctx.accounts.draw;
    draw.pending = false;
    emit!(DrawCancelled {
        base_mint: draw.base_mint,
        seed: draw.seed,
        slot: Clock::get()?.slot,
    });
    Ok(())
}

#[derive(Accounts)]
pub struct OpenDraw<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,

    #[account(
        seeds = [LAUNCH_SEED, launch.base_mint.as_ref()],
        bump = launch.bump,
        constraint = launch.kit == KIT_DRAW @ LabkitError::WrongKit,
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

    #[account(
        init,
        payer = payer,
        space = 8 + Draw::INIT_SPACE,
        seeds = [DRAW_SEED, launch.base_mint.as_ref()],
        bump,
    )]
    pub draw: Box<Account<'info, Draw>>,

    #[account(
        init,
        payer = payer,
        space = 8 + DrawTree::INIT_SPACE,
        seeds = [DRAW_TREE_SEED, launch.base_mint.as_ref()],
        bump,
    )]
    pub draw_tree: Box<Account<'info, DrawTree>>,

    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct RequestDraw<'info> {
    /// Pays ORAO and is refunded from the pot.
    #[account(mut)]
    pub keeper: Signer<'info>,

    #[account(mut, token::mint = quote_mint, token::token_program = token_program)]
    pub keeper_quote: Box<InterfaceAccount<'info, TokenAccount>>,

    #[account(seeds = [LAUNCH_SEED, launch.base_mint.as_ref()], bump = launch.bump)]
    pub launch: Box<Account<'info, Launch>>,

    #[account(seeds = [STAKE_POOL_SEED, launch.base_mint.as_ref()], bump = stake_pool.bump)]
    pub stake_pool: Box<Account<'info, StakePool>>,

    /// Only a Draw launch has one, so it stands in for checking the kit.
    #[account(mut, seeds = [DRAW_SEED, launch.base_mint.as_ref()], bump = draw.bump)]
    pub draw: Box<Account<'info, Draw>>,

    /// CHECK: The launch's draw tree, by seeds. Only its `frozen` byte is written.
    #[account(mut, seeds = [DRAW_TREE_SEED, launch.base_mint.as_ref()], bump, owner = crate::ID)]
    pub draw_tree: UncheckedAccount<'info>,

    /// CHECK: The kit PDA, which signs the refund.
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

    /// CHECK: ORAO VRF.
    #[account(address = ORAO_VRF_PROGRAM_ID)]
    pub vrf: UncheckedAccount<'info>,

    /// CHECK: ORAO's network state. ORAO checks it.
    #[account(mut, seeds = [ORAO_NETWORK_STATE_SEED], bump, seeds::program = ORAO_VRF_PROGRAM_ID)]
    pub network_state: UncheckedAccount<'info>,

    /// CHECK: ORAO's treasury. ORAO checks it against the network state.
    #[account(mut)]
    pub treasury: UncheckedAccount<'info>,

    /// CHECK: ORAO's request account for `seed`. ORAO checks it.
    #[account(mut)]
    pub randomness: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SettleDraw<'info> {
    #[account(seeds = [LAUNCH_SEED, launch.base_mint.as_ref()], bump = launch.bump)]
    pub launch: Box<Account<'info, Launch>>,

    #[account(mut, seeds = [STAKE_POOL_SEED, launch.base_mint.as_ref()], bump = stake_pool.bump)]
    pub stake_pool: Box<Account<'info, StakePool>>,

    #[account(mut, seeds = [DRAW_SEED, launch.base_mint.as_ref()], bump = draw.bump)]
    pub draw: Box<Account<'info, Draw>>,

    /// CHECK: The launch's draw tree, by seeds.
    #[account(mut, seeds = [DRAW_TREE_SEED, launch.base_mint.as_ref()], bump, owner = crate::ID)]
    pub draw_tree: UncheckedAccount<'info>,

    /// CHECK: The kit PDA, which owns `kit_quote`.
    #[account(seeds = [KIT_SEED, launch.base_mint.as_ref()], bump = launch.kit_bump)]
    pub kit: UncheckedAccount<'info>,

    /// CHECK: Address-checked WSOL mint.
    #[account(address = WSOL_MINT)]
    pub quote_mint: UncheckedAccount<'info>,

    #[account(
        associated_token::mint = quote_mint,
        associated_token::authority = kit,
        associated_token::token_program = token_program,
    )]
    pub kit_quote: Box<InterfaceAccount<'info, TokenAccount>>,

    /// CHECK: ORAO's request account for `draw.seed`, checked by address and owner.
    pub randomness: UncheckedAccount<'info>,

    pub token_program: Interface<'info, TokenInterface>,
}

#[derive(Accounts)]
pub struct CancelDraw<'info> {
    #[account(seeds = [LAUNCH_SEED, launch.base_mint.as_ref()], bump = launch.bump)]
    pub launch: Box<Account<'info, Launch>>,

    #[account(mut, seeds = [DRAW_SEED, launch.base_mint.as_ref()], bump = draw.bump)]
    pub draw: Box<Account<'info, Draw>>,

    /// CHECK: The launch's draw tree, by seeds.
    #[account(mut, seeds = [DRAW_TREE_SEED, launch.base_mint.as_ref()], bump, owner = crate::ID)]
    pub draw_tree: UncheckedAccount<'info>,

    /// CHECK: ORAO's request account for `draw.seed`, checked by address and owner.
    pub randomness: UncheckedAccount<'info>,
}
