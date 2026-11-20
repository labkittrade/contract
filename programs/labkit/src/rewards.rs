use anchor_lang::prelude::*;

use crate::constants::STAKE_STREAM_SECONDS;
use crate::errors::LabkitError;
use crate::state::{StakePool, StakePosition};

/// Brings the pool up to `now`: streams what is due to the current stakers, then starts
/// streaming whatever reached the kit since the last update. `balance` is the kit's WSOL.
pub fn sync(pool: &mut StakePool, balance: u64, now: i64) -> Result<()> {
    if pool.total_staked == 0 {
        // Nothing streams to an empty pool. The first staker restarts the window rather
        // than collecting everything that piled up before them.
        pool.stream_end = now.saturating_add(STAKE_STREAM_SECONDS);
    } else if now > pool.last_update && pool.streaming > 0 {
        let released = if now >= pool.stream_end {
            pool.streaming
        } else {
            let elapsed = (now - pool.last_update) as u128;
            let window = (pool.stream_end - pool.last_update) as u128;
            (pool.streaming as u128 * elapsed / window) as u64
        };
        let per_token = ((released as u128) << 64) / pool.total_staked as u128;
        pool.reward_per_token_q64 = pool
            .reward_per_token_q64
            .checked_add(per_token)
            .ok_or(LabkitError::Overflow)?;
        pool.streaming -= released;
        pool.distributed = pool
            .distributed
            .checked_add(released)
            .ok_or(LabkitError::Overflow)?;
    }
    pool.last_update = now;

    let owed = pool.distributed - pool.claimed;
    let booked = pool.streaming.checked_add(owed).ok_or(LabkitError::Overflow)?;
    let income = balance.saturating_sub(booked);
    if income > 0 {
        pool.streaming += income;
        pool.stream_end = now.saturating_add(STAKE_STREAM_SECONDS);
    }
    Ok(())
}

/// Credits the position with what it earned since it was last settled.
pub fn settle(position: &mut StakePosition, pool: &StakePool) -> Result<()> {
    let delta = pool.reward_per_token_q64 - position.reward_per_token_q64;
    let earned = (position.amount as u128)
        .checked_mul(delta)
        .ok_or(LabkitError::Overflow)?
        >> 64;
    let earned = u64::try_from(earned).map_err(|_| error!(LabkitError::Overflow))?;
    position.earned = position.earned.checked_add(earned).ok_or(LabkitError::Overflow)?;
    position.reward_per_token_q64 = pool.reward_per_token_q64;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 60 * 60;

    fn pool() -> StakePool {
        StakePool {
            bump: 0,
            base_mint: Pubkey::default(),
            total_staked: 0,
            reward_per_token_q64: 0,
            streaming: 0,
            stream_end: 0,
            last_update: 0,
            distributed: 0,
            claimed: 0,
        }
    }

    fn position(amount: u64, pool: &StakePool) -> StakePosition {
        StakePosition {
            bump: 0,
            owner: Pubkey::default(),
            base_mint: Pubkey::default(),
            amount,
            reward_per_token_q64: pool.reward_per_token_q64,
            earned: 0,
            claimed: 0,
            slot: 0,
            staked_at: 0,
        }
    }

    #[test]
    fn rewards_stream_evenly_over_the_window() {
        let mut p = pool();
        sync(&mut p, 0, 0).unwrap();
        let mut alice = position(1_000, &p);
        p.total_staked = 1_000;
        sync(&mut p, 3_000_000, 0).unwrap();
        assert_eq!(p.streaming, 3_000_000);

        sync(&mut p, 3_000_000, DAY).unwrap();
        settle(&mut alice, &p).unwrap();
        assert!(alice.earned.abs_diff(1_000_000) <= 1);

        sync(&mut p, 3_000_000, 10 * DAY).unwrap();
        settle(&mut alice, &p).unwrap();
        assert!(alice.earned.abs_diff(3_000_000) <= 1);
        assert_eq!(p.streaming, 0);
    }

    #[test]
    fn what_piled_up_before_anyone_staked_streams_from_the_first_stake() {
        let mut p = pool();
        sync(&mut p, 5_000_000, 0).unwrap();
        sync(&mut p, 5_000_000, 30 * DAY).unwrap();
        assert_eq!(p.distributed, 0);
        assert_eq!(p.stream_end, 30 * DAY + STAKE_STREAM_SECONDS);

        let mut alice = position(10, &p);
        p.total_staked = 10;
        sync(&mut p, 5_000_000, 30 * DAY + DAY).unwrap();
        settle(&mut alice, &p).unwrap();
        assert!(alice.earned.abs_diff(5_000_000 / 3) <= 1);
    }

    #[test]
    fn stakers_share_by_amount_and_time() {
        let mut p = pool();
        sync(&mut p, 0, 0).unwrap();
        let mut alice = position(300, &p);
        p.total_staked = 300;
        sync(&mut p, 6_000_000, 0).unwrap();

        sync(&mut p, 6_000_000, DAY).unwrap();
        settle(&mut alice, &p).unwrap();
        let mut bob = position(100, &p);
        p.total_staked = 400;

        sync(&mut p, 6_000_000, 3 * DAY).unwrap();
        settle(&mut alice, &p).unwrap();
        settle(&mut bob, &p).unwrap();
        assert!(alice.earned.abs_diff(2_000_000 + 3_000_000) <= 2);
        assert!(bob.earned.abs_diff(1_000_000) <= 2);
        assert!(alice.earned + bob.earned <= p.distributed);
    }

    #[test]
    fn a_new_deposit_restarts_the_window_with_what_is_left() {
        let mut p = pool();
        sync(&mut p, 0, 0).unwrap();
        p.total_staked = 1;
        sync(&mut p, 3_000_000, 0).unwrap();
        sync(&mut p, 4_000_000, 2 * DAY).unwrap();
        assert_eq!(p.distributed, 2_000_000);
        assert_eq!(p.streaming, 2_000_000);
        assert_eq!(p.stream_end, 2 * DAY + STAKE_STREAM_SECONDS);
    }

    #[test]
    fn claims_leave_the_books_balanced() {
        let mut p = pool();
        sync(&mut p, 0, 0).unwrap();
        let mut alice = position(7, &p);
        p.total_staked = 7;
        sync(&mut p, 1_000_003, 0).unwrap();
        sync(&mut p, 1_000_003, 10 * DAY).unwrap();
        settle(&mut alice, &p).unwrap();
        p.claimed += alice.earned;
        let balance = 1_000_003 - alice.earned;
        sync(&mut p, balance, 11 * DAY).unwrap();
        assert_eq!(p.streaming, 0);
    }
}
