use anchor_lang::prelude::*;

use crate::constants::*;
use crate::errors::LabkitError;
use crate::payout::bps_of;

/// A constant-product market as a kit buy sees it.
pub struct Market {
    pub quote: u64,
    pub base: u64,
    /// Every fee a buy pays, on the 1_000_000 denominator.
    pub fee_rate: u64,
    /// Tokens left on the bonding curve. `None` on Raydium.
    pub base_left: Option<u64>,
}

pub fn validate_kit(kit: u8, drawdown_bps: u16) -> Result<()> {
    match kit {
        KIT_BURN | KIT_DRAW | KIT_GRADUATE | KIT_STAKE => require!(drawdown_bps == 0, LabkitError::InvalidKit),
        KIT_FLOOR => require!(
            (MIN_DRAWDOWN_BPS..=MAX_DRAWDOWN_BPS).contains(&drawdown_bps),
            LabkitError::InvalidKit
        ),
        _ => return err!(LabkitError::InvalidKit),
    }
    Ok(())
}

/// Lamports per raw token, Q64.64.
pub fn price_q64(quote: u64, base: u64) -> Result<u128> {
    require!(base > 0, LabkitError::BadPoolState);
    Ok(((quote as u128) << 64) / base as u128)
}

/// Floor's high after seeing `price` at `now`. It rises toward `price` by at most
/// `HIGH_RISE_BPS` per `HIGH_RISE_SECONDS` since it last rose, counting one window at
/// most. A high of zero takes the price as it is.
pub fn raise_high(high_q64: u128, high_at: i64, price: u128, now: i64) -> u128 {
    if high_q64 == 0 {
        return price;
    }
    let elapsed = now.saturating_sub(high_at).clamp(0, HIGH_RISE_SECONDS) as u128;
    let rise = high_q64.saturating_mul(HIGH_RISE_BPS as u128).saturating_mul(elapsed)
        / (BPS_DENOMINATOR as u128 * HIGH_RISE_SECONDS as u128);
    high_q64.max(price.min(high_q64.saturating_add(rise)))
}

pub fn floor_line(high_q64: u128, drawdown_bps: u16) -> u128 {
    high_q64.saturating_mul((BPS_DENOMINATOR - drawdown_bps as u64) as u128) / BPS_DENOMINATOR as u128
}

/// Lamports the next buy spends out of `balance`. A buy never takes more than the curve
/// has left. Floor passes its line, and buys back up to it and no further.
pub fn plan_buy(market: &Market, balance: u64, floor: Option<u128>) -> Result<u64> {
    let mut spend = balance.min(bps_of(market.quote, MAX_SPEND_OF_RESERVE_BPS as u16)?);
    let fill = match market.base_left {
        Some(left) => Some(gross_for_base(market, left)?),
        None => None,
    };
    if let Some(fill) = fill {
        spend = spend.min(fill);
    }
    if let Some(line) = floor {
        require!(
            price_q64(market.quote, market.base)? <= line,
            LabkitError::AboveFloor
        );
        let net = net_to_reach(market.quote, market.base, line)?;
        spend = spend.min(gross_up(net, market.fee_rate)?);
    }
    require!(
        spend > 0 && (spend >= MIN_EXECUTE_LAMPORTS || Some(spend) == fill),
        LabkitError::BelowMinimum
    );
    Ok(spend)
}

/// The least a buy of `amount_in` may receive: the constant-product output after fees,
/// less the rounding tolerance, and never more than the curve has left.
pub fn min_out(market: &Market, amount_in: u64) -> Result<u64> {
    let fee = ceil_div(
        amount_in as u128 * market.fee_rate as u128,
        RATE_DENOMINATOR as u128,
    );
    let net = (amount_in as u128).saturating_sub(fee);
    let out = net * market.base as u128 / (market.quote as u128 + net);
    let out = out * (BPS_DENOMINATOR - KIT_SLIPPAGE_BPS) as u128 / BPS_DENOMINATOR as u128;
    let out = u64::try_from(out).map_err(|_| error!(LabkitError::Overflow))?;
    let out = market.base_left.map_or(out, |left| out.min(left));
    require!(out > 0, LabkitError::BelowMinimum);
    Ok(out)
}

/// Lamports, fees included, that buy `base_out` tokens. One lamport over, because the
/// fee rounds up on the gross amount.
fn gross_for_base(market: &Market, base_out: u64) -> Result<u64> {
    require!(base_out < market.base, LabkitError::BadPoolState);
    let net = ceil_div(
        market.quote as u128 * base_out as u128,
        (market.base - base_out) as u128,
    );
    let net = u64::try_from(net).map_err(|_| error!(LabkitError::Overflow))?;
    gross_up(net, market.fee_rate)?
        .checked_add(1)
        .ok_or_else(|| error!(LabkitError::Overflow))
}

/// Lamports in, fees included, for `net` lamports to reach the reserves.
fn gross_up(net: u64, fee_rate: u64) -> Result<u64> {
    require!(fee_rate < RATE_DENOMINATOR, LabkitError::Overflow);
    let gross = ceil_div(
        net as u128 * RATE_DENOMINATOR as u128,
        (RATE_DENOMINATOR - fee_rate) as u128,
    );
    u64::try_from(gross).map_err(|_| error!(LabkitError::Overflow))
}

/// Lamports that must reach the quote reserve to lift the price to `target_q64`.
/// Zero when the price is already there.
fn net_to_reach(quote: u64, base: u64, target_q64: u128) -> Result<u64> {
    if price_q64(quote, base)? >= target_q64 {
        return Ok(0);
    }
    let k = quote as u128 * base as u128;
    let squared = mul_shr64(target_q64, k)?;
    let mut new_quote = isqrt(squared);
    if new_quote * new_quote < squared {
        new_quote += 1;
    }
    let net = new_quote.saturating_sub(quote as u128);
    u64::try_from(net).map_err(|_| error!(LabkitError::Overflow))
}

fn ceil_div(a: u128, b: u128) -> u128 {
    a.div_ceil(b)
}

/// `(a * b) >> 64`, rejecting results that do not fit in `u128`.
fn mul_shr64(a: u128, b: u128) -> Result<u128> {
    const LO: u128 = (1u128 << 64) - 1;
    let (a_lo, a_hi) = (a & LO, a >> 64);
    let (b_lo, b_hi) = (b & LO, b >> 64);
    let ll = a_lo * b_lo;
    let lh = a_lo * b_hi;
    let hl = a_hi * b_lo;
    let hh = a_hi * b_hi;
    let mid = (ll >> 64) + (lh & LO) + (hl & LO);
    let hi = hh + (lh >> 64) + (hl >> 64) + (mid >> 64);
    require!(hi >> 64 == 0, LabkitError::Overflow);
    Ok((hi << 64) | (mid & LO))
}

fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let (mut lo, mut hi) = (1u128, n.min(1u128 << 64));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if mid <= n / mid {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEE: u64 = 17_500;

    fn curve(left: u64) -> Market {
        Market {
            quote: 30_000_000_000,
            base: 1_073_000_000_000_000,
            fee_rate: FEE,
            base_left: Some(left),
        }
    }

    #[test]
    fn kits_validate_their_drawdown() {
        validate_kit(KIT_BURN, 0).unwrap();
        validate_kit(KIT_GRADUATE, 0).unwrap();
        validate_kit(KIT_STAKE, 0).unwrap();
        assert!(validate_kit(KIT_STAKE, 500).is_err());
        validate_kit(KIT_FLOOR, 2_000).unwrap();
        assert!(validate_kit(KIT_BURN, 100).is_err());
        assert!(validate_kit(KIT_FLOOR, 0).is_err());
        assert!(validate_kit(KIT_FLOOR, 9_500).is_err());
        validate_kit(KIT_DRAW, 0).unwrap();
        assert!(validate_kit(KIT_DRAW, 1_000).is_err());
    }

    #[test]
    fn a_buy_is_capped_at_one_percent_of_the_quote_reserve() {
        let market = curve(793_100_000_000_000);
        assert_eq!(plan_buy(&market, 5_000_000_000, None).unwrap(), 300_000_000);
        assert_eq!(plan_buy(&market, 50_000_000, None).unwrap(), 50_000_000);
        assert!(plan_buy(&market, 1_000_000, None).is_err());
    }

    #[test]
    fn the_last_buy_fills_the_curve_even_under_the_minimum() {
        let market = curve(1_000_000);
        let spend = plan_buy(&market, 300_000_000, None).unwrap();
        assert!(spend < MIN_EXECUTE_LAMPORTS);
        let out = market.base as u128 * net_after_fee(spend) as u128
            / (market.quote as u128 + net_after_fee(spend) as u128);
        assert!(out as u64 >= 1_000_000);
        let floor = min_out(&market, spend).unwrap();
        assert!(floor <= 1_000_000 && floor as u128 <= out);
    }

    fn net_after_fee(amount: u64) -> u64 {
        amount - ceil_div(amount as u128 * FEE as u128, RATE_DENOMINATOR as u128) as u64
    }

    #[test]
    fn floor_buys_only_under_the_line_and_only_back_up_to_it() {
        let market = Market { base_left: None, ..curve(0) };
        let spot = price_q64(market.quote, market.base).unwrap();
        let high = spot * 10 / 8;
        assert!(plan_buy(&market, 5_000_000_000, Some(floor_line(high, 3_000))).is_err());
        let line = floor_line(high, 1_950);
        let spend = plan_buy(&market, 5_000_000_000, Some(line)).unwrap();
        assert!(spend < 300_000_000);
        let net = net_after_fee(spend) as u128;
        let quote = market.quote as u128 + net;
        let base = market.quote as u128 * market.base as u128 / quote;
        let landed = (quote << 64) / base;
        assert!(landed >= line && landed - line < line / 1_000);
    }

    #[test]
    fn min_out_leaves_room_for_rounding_only() {
        let market = Market { base_left: None, ..curve(0) };
        let amount = 100_000_000;
        let exact = market.base as u128 * net_after_fee(amount) as u128
            / (market.quote as u128 + net_after_fee(amount) as u128);
        let floor = min_out(&market, amount).unwrap() as u128;
        assert!(floor < exact && floor * 10_000 >= exact * 9_899);
    }

    #[test]
    fn the_high_rises_at_most_one_step_per_window() {
        let high = 1u128 << 64;
        let step = high * (BPS_DENOMINATOR + HIGH_RISE_BPS) as u128 / BPS_DENOMINATOR as u128;
        assert_eq!(raise_high(high, 0, high * 5, 0), high);
        assert_eq!(raise_high(high, 0, high * 5, HIGH_RISE_SECONDS), step);
        assert_eq!(raise_high(high, 0, high * 5, 100 * HIGH_RISE_SECONDS), step);
        let half = raise_high(high, 0, high * 5, HIGH_RISE_SECONDS / 2);
        assert!(half > high && half < step);
        assert_eq!(raise_high(high, 0, high + 7, HIGH_RISE_SECONDS), high + 7);
        assert_eq!(raise_high(high, 0, high / 2, HIGH_RISE_SECONDS), high);
        assert_eq!(raise_high(high, HIGH_RISE_SECONDS, high * 5, 0), high);
        assert_eq!(raise_high(0, 0, 123, 0), 123);
    }

    #[test]
    fn isqrt_is_exact_on_squares() {
        for n in [0u128, 1, 4, 9, 1 << 40, (1u128 << 63) * (1u128 << 63)] {
            let r = isqrt(n);
            assert!(r * r <= n && (r + 1) * (r + 1) > n);
        }
    }
}
