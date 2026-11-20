//! A Fenwick tree of stakes over the bytes after the `DrawTree` header. Index `i`
//! (1-based) belongs to one position for good, so a stake of zero stays as a zero entry.

use anchor_lang::prelude::*;

use crate::errors::LabkitError;

/// Discriminator, `bump`, `frozen`, `len`.
pub const HEADER_LEN: usize = 8 + 1 + 1 + 4;
pub const FROZEN_OFFSET: usize = 9;
pub const LEN_OFFSET: usize = 10;
pub const ENTRY_LEN: usize = 8;

pub fn len(data: &[u8]) -> usize {
    u32::from_le_bytes(data[LEN_OFFSET..LEN_OFFSET + 4].try_into().unwrap()) as usize
}

pub fn set_len(data: &mut [u8], len: usize) {
    data[LEN_OFFSET..LEN_OFFSET + 4].copy_from_slice(&(len as u32).to_le_bytes());
}

pub fn frozen(data: &[u8]) -> bool {
    data[FROZEN_OFFSET] != 0
}

pub fn set_frozen(data: &mut [u8], frozen: bool) {
    data[FROZEN_OFFSET] = frozen as u8;
}

pub fn space(len: usize) -> usize {
    HEADER_LEN + len * ENTRY_LEN
}

fn lowbit(i: usize) -> usize {
    i & i.wrapping_neg()
}

fn get(entries: &[u8], i: usize) -> u64 {
    let at = (i - 1) * ENTRY_LEN;
    u64::from_le_bytes(entries[at..at + ENTRY_LEN].try_into().unwrap())
}

fn put(entries: &mut [u8], i: usize, value: u64) {
    let at = (i - 1) * ENTRY_LEN;
    entries[at..at + ENTRY_LEN].copy_from_slice(&value.to_le_bytes());
}

/// Sum of entries 1..=i.
pub fn prefix(entries: &[u8], mut i: usize) -> u64 {
    let mut sum = 0u64;
    while i > 0 {
        sum += get(entries, i);
        i -= lowbit(i);
    }
    sum
}

pub fn add(entries: &mut [u8], len: usize, mut i: usize, amount: u64) -> Result<()> {
    require!(i >= 1 && i <= len, LabkitError::BadDrawTree);
    while i <= len {
        let value = get(entries, i).checked_add(amount).ok_or(LabkitError::Overflow)?;
        put(entries, i, value);
        i += lowbit(i);
    }
    Ok(())
}

/// Nodes hold range sums, so this can't see one entry going negative. The caller checks
/// `amount` against the position's stake.
pub fn sub(entries: &mut [u8], len: usize, mut i: usize, amount: u64) -> Result<()> {
    require!(i >= 1 && i <= len, LabkitError::BadDrawTree);
    while i <= len {
        let value = get(entries, i).checked_sub(amount).ok_or(LabkitError::BadDrawTree)?;
        put(entries, i, value);
        i += lowbit(i);
    }
    Ok(())
}

/// Appends `amount` as entry `len + 1`. `entries` must already have room for it.
pub fn push(entries: &mut [u8], len: usize, amount: u64) -> Result<usize> {
    let i = len + 1;
    let covered = prefix(entries, len) - prefix(entries, i - lowbit(i));
    put(entries, i, covered.checked_add(amount).ok_or(LabkitError::Overflow)?);
    Ok(i)
}

/// The index whose range of the total holds `point`: the least `i` with
/// `prefix(i) > point`. `point` must be under the total.
pub fn find(entries: &[u8], len: usize, point: u64) -> Result<usize> {
    require!(point < prefix(entries, len), LabkitError::BadDrawTree);
    let mut at = 0usize;
    let mut left = point;
    let mut step = if len == 0 { 0 } else { 1usize << (usize::BITS - 1 - len.leading_zeros()) };
    while step > 0 {
        let next = at + step;
        if next <= len {
            let value = get(entries, next);
            if value <= left {
                at = next;
                left -= value;
            }
        }
        step >>= 1;
    }
    Ok(at + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(stakes: &[u64]) -> Vec<u8> {
        let mut entries = vec![0u8; stakes.len() * ENTRY_LEN];
        for (n, stake) in stakes.iter().enumerate() {
            push(&mut entries, n, *stake).unwrap();
        }
        entries
    }

    fn owner_by_scan(stakes: &[u64], point: u64) -> usize {
        let mut sum = 0;
        for (n, stake) in stakes.iter().enumerate() {
            sum += stake;
            if point < sum {
                return n + 1;
            }
        }
        unreachable!()
    }

    #[test]
    fn appending_builds_the_same_prefix_sums_as_a_scan() {
        let stakes: Vec<u64> = (0..37).map(|n| (n * 7919 % 101) as u64).collect();
        let entries = tree(&stakes);
        for i in 0..=stakes.len() {
            assert_eq!(prefix(&entries, i), stakes[..i].iter().sum::<u64>());
        }
    }

    #[test]
    fn every_point_finds_the_stake_that_covers_it() {
        let stakes = [5u64, 0, 3, 0, 0, 9, 1, 4, 0, 2, 6];
        let entries = tree(&stakes);
        let total: u64 = stakes.iter().sum();
        for point in 0..total {
            assert_eq!(find(&entries, stakes.len(), point).unwrap(), owner_by_scan(&stakes, point));
        }
        assert!(find(&entries, stakes.len(), total).is_err());
    }

    #[test]
    fn updates_move_the_ranges() {
        let mut stakes = vec![10u64, 20, 30, 40, 50, 60, 70];
        let mut entries = tree(&stakes);
        add(&mut entries, stakes.len(), 3, 15).unwrap();
        stakes[2] += 15;
        sub(&mut entries, stakes.len(), 6, 60).unwrap();
        stakes[5] = 0;
        let total: u64 = stakes.iter().sum();
        assert_eq!(prefix(&entries, stakes.len()), total);
        for point in (0..total).step_by(3) {
            assert_eq!(find(&entries, stakes.len(), point).unwrap(), owner_by_scan(&stakes, point));
        }
        assert!(sub(&mut entries, stakes.len(), 1, 11).is_err());
        assert!(add(&mut entries, stakes.len(), 8, 1).is_err());
    }

    #[test]
    fn a_big_tree_needs_few_reads() {
        let stakes: Vec<u64> = (1..=100_000u64).map(|n| n % 1_000 + 1).collect();
        let entries = tree(&stakes);
        let total = prefix(&entries, stakes.len());
        assert_eq!(total, stakes.iter().sum::<u64>());
        for point in [0, 1, total / 3, total / 2, total - 1] {
            assert_eq!(find(&entries, stakes.len(), point).unwrap(), owner_by_scan(&stakes, point));
        }
    }
}
