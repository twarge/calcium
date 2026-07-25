//! The numeric tower.
//!
//! This is a symbolic system, so exactness matters more than speed: `1/3` has
//! to stay `1/3` through an arbitrary amount of algebra and only become
//! `0.3333` when it is printed. We therefore keep every value that *can* be
//! exact as a `BigRational` and fall back to `f64` only when an operation
//! genuinely leaves the rationals (`sqrt`, `sin`, irrational powers...).

use num_bigint::{BigInt, Sign};
use num_rational::BigRational;
use num_traits::{One, Signed, ToPrimitive, Zero};
use std::fmt;

#[derive(Clone, Debug)]
pub enum Num {
    /// An exact value.
    Rat(BigRational),
    /// An inexact value. Also carries the infinities and NaN.
    Flt(f64),
}

impl Num {
    pub fn zero() -> Num {
        Num::Rat(BigRational::zero())
    }

    pub fn one() -> Num {
        Num::Rat(BigRational::one())
    }

    pub fn from_i64(v: i64) -> Num {
        Num::Rat(BigRational::from_integer(BigInt::from(v)))
    }

    pub fn from_bigint(v: BigInt) -> Num {
        Num::Rat(BigRational::from_integer(v))
    }

    pub fn ratio(n: i64, d: i64) -> Num {
        Num::Rat(BigRational::new(BigInt::from(n), BigInt::from(d)))
    }

    /// Wraps an `f64`, but recovers exactness for values that are plainly
    /// integral. Keeps `2.0` from poisoning a chain of otherwise exact work.
    pub fn from_f64(v: f64) -> Num {
        if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.007_199_254_740_992e15 {
            Num::from_i64(v as i64)
        } else {
            Num::Flt(v)
        }
    }

    pub fn infinity() -> Num {
        Num::Flt(f64::INFINITY)
    }

    pub fn is_exact(&self) -> bool {
        matches!(self, Num::Rat(_))
    }

    pub fn is_zero(&self) -> bool {
        match self {
            Num::Rat(r) => r.is_zero(),
            Num::Flt(f) => *f == 0.0,
        }
    }

    pub fn is_one(&self) -> bool {
        match self {
            Num::Rat(r) => r.is_one(),
            Num::Flt(f) => *f == 1.0,
        }
    }

    pub fn is_negative(&self) -> bool {
        match self {
            Num::Rat(r) => r.is_negative(),
            Num::Flt(f) => *f < 0.0,
        }
    }

    pub fn is_infinite(&self) -> bool {
        matches!(self, Num::Flt(f) if f.is_infinite())
    }

    /// True for ordinary finite values; false for the infinities and NaN.
    pub fn is_finite_number(&self) -> bool {
        match self {
            Num::Rat(_) => true,
            Num::Flt(f) => f.is_finite(),
        }
    }

    pub fn is_nan(&self) -> bool {
        matches!(self, Num::Flt(f) if f.is_nan())
    }

    pub fn is_integer(&self) -> bool {
        match self {
            Num::Rat(r) => r.is_integer(),
            Num::Flt(f) => f.is_finite() && f.fract() == 0.0,
        }
    }

    pub fn to_f64(&self) -> f64 {
        match self {
            Num::Rat(r) => r.to_f64().unwrap_or(f64::NAN),
            Num::Flt(f) => *f,
        }
    }

    /// Exact integer value, if this is one that fits.
    pub fn to_i64(&self) -> Option<i64> {
        match self {
            Num::Rat(r) if r.is_integer() => r.to_integer().to_i64(),
            Num::Flt(f) if f.is_finite() && f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        }
    }

    pub fn to_bigint(&self) -> Option<BigInt> {
        match self {
            Num::Rat(r) if r.is_integer() => Some(r.to_integer()),
            Num::Flt(f) if f.is_finite() && f.fract() == 0.0 => Some(BigInt::from(*f as i128)),
            _ => None,
        }
    }

    fn as_rat(&self) -> Option<&BigRational> {
        match self {
            Num::Rat(r) => Some(r),
            Num::Flt(_) => None,
        }
    }

    pub fn neg(&self) -> Num {
        match self {
            Num::Rat(r) => Num::Rat(-r),
            Num::Flt(f) => Num::Flt(-f),
        }
    }

    pub fn abs(&self) -> Num {
        match self {
            Num::Rat(r) => Num::Rat(r.abs()),
            Num::Flt(f) => Num::Flt(f.abs()),
        }
    }

    pub fn add(&self, other: &Num) -> Num {
        match (self.as_rat(), other.as_rat()) {
            (Some(a), Some(b)) => Num::Rat(a + b),
            _ => Num::Flt(self.to_f64() + other.to_f64()),
        }
    }

    pub fn sub(&self, other: &Num) -> Num {
        match (self.as_rat(), other.as_rat()) {
            (Some(a), Some(b)) => Num::Rat(a - b),
            _ => Num::Flt(self.to_f64() - other.to_f64()),
        }
    }

    pub fn mul(&self, other: &Num) -> Num {
        match (self.as_rat(), other.as_rat()) {
            (Some(a), Some(b)) => Num::Rat(a * b),
            _ => Num::Flt(self.to_f64() * other.to_f64()),
        }
    }

    /// Division. `x/0` is `Infinity` rather than an error, so this cannot fail.
    pub fn div(&self, other: &Num) -> Num {
        // `∞ / ∞` answers 1. Not defensible as mathematics, but it keeps a
        // document with a stray infinity in it usable rather than poisoning
        // everything downstream with NaN.
        if self.is_infinite() && other.is_infinite() {
            return if self.is_negative() == other.is_negative() {
                Num::one()
            } else {
                Num::from_i64(-1)
            };
        }
        if other.is_zero() {
            if self.is_zero() {
                return Num::Flt(f64::NAN);
            }
            return Num::Flt(if self.is_negative() {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            });
        }
        match (self.as_rat(), other.as_rat()) {
            (Some(a), Some(b)) => Num::Rat(a / b),
            _ => Num::Flt(self.to_f64() / other.to_f64()),
        }
    }

    /// Exponentiation, staying exact for integer exponents.
    pub fn pow(&self, other: &Num) -> Num {
        if let (Num::Rat(base), Some(e)) = (self, other.to_i64()) {
            if e.unsigned_abs() <= 4096 {
                if base.is_zero() && e < 0 {
                    return Num::infinity();
                }
                let magnitude = num_traits::pow::pow(base.clone(), e.unsigned_abs() as usize);
                return Num::Rat(if e < 0 {
                    magnitude.recip()
                } else {
                    magnitude
                });
            }
        }
        // A rational raised to a rational can still land back on a rational
        // (`4^0.5`), so probe for a clean root before giving up on exactness.
        if let (Num::Rat(base), Num::Rat(exp)) = (self, other) {
            if let Some(root) = exact_root(base, exp) {
                return Num::Rat(root);
            }
        }
        Num::from_f64(self.to_f64().powf(other.to_f64()))
    }

    pub fn modulo(&self, other: &Num) -> Num {
        if other.is_zero() {
            return Num::Flt(f64::NAN);
        }
        match (self.as_rat(), other.as_rat()) {
            (Some(a), Some(b)) => {
                let q = (a / b).floor();
                Num::Rat(a - b * q)
            }
            _ => Num::Flt(self.to_f64().rem_euclid(other.to_f64().abs())),
        }
    }

    pub fn floor(&self) -> Num {
        match self {
            Num::Rat(r) => Num::Rat(r.floor()),
            Num::Flt(f) => Num::from_f64(f.floor()),
        }
    }

    pub fn ceil(&self) -> Num {
        match self {
            Num::Rat(r) => Num::Rat(r.ceil()),
            Num::Flt(f) => Num::from_f64(f.ceil()),
        }
    }

    pub fn truncate(&self) -> Num {
        match self {
            Num::Rat(r) => Num::Rat(r.trunc()),
            Num::Flt(f) => Num::from_f64(f.trunc()),
        }
    }

    /// Round half to even: `round(1.5) => 2`, `round(2.5) => 2`.
    pub fn round(&self) -> Num {
        match self {
            Num::Rat(r) => Num::Rat(round_half_even(r)),
            Num::Flt(f) => {
                let lo = f.floor();
                let frac = f - lo;
                let v = if frac > 0.5 {
                    lo + 1.0
                } else if frac < 0.5 {
                    lo
                } else if (lo / 2.0).fract() == 0.0 {
                    lo
                } else {
                    lo + 1.0
                };
                Num::from_f64(v)
            }
        }
    }

    pub fn sign(&self) -> Num {
        if self.is_zero() {
            Num::zero()
        } else if self.is_negative() {
            Num::from_i64(-1)
        } else {
            Num::one()
        }
    }

    pub fn cmp_num(&self, other: &Num) -> Option<std::cmp::Ordering> {
        match (self.as_rat(), other.as_rat()) {
            (Some(a), Some(b)) => Some(a.cmp(b)),
            _ => self.to_f64().partial_cmp(&other.to_f64()),
        }
    }

    pub fn eq_num(&self, other: &Num) -> bool {
        self.cmp_num(other) == Some(std::cmp::Ordering::Equal)
    }
}

/// Numeric equality, so `Rat(1)` and `Flt(1.0)` compare equal. Deliberately
/// *not* structural: exactness is an implementation detail, not part of a
/// value's identity.
impl PartialEq for Num {
    fn eq(&self, other: &Num) -> bool {
        self.eq_num(other)
    }
}

/// `base^(p/q)` when the result is itself rational — e.g. `(4/9)^(1/2)`.
fn exact_root(base: &BigRational, exp: &BigRational) -> Option<BigRational> {
    let q = exp.denom().to_u32()?;
    let p = exp.numer().to_i32()?;
    if q == 0 || q > 64 || p.unsigned_abs() > 1024 {
        return None;
    }
    if base.is_negative() && q % 2 == 0 {
        return None; // even root of a negative is not real
    }
    let n = integer_root(base.numer(), q)?;
    let d = integer_root(base.denom(), q)?;
    let root = BigRational::new(n, d);
    let magnitude = num_traits::pow::pow(root, p.unsigned_abs() as usize);
    Some(if p < 0 { magnitude.recip() } else { magnitude })
}

/// Exact `q`th root of an integer, or `None` when it is irrational.
fn integer_root(v: &BigInt, q: u32) -> Option<BigInt> {
    let negative = v.sign() == Sign::Minus;
    let magnitude = v.magnitude();
    let root = magnitude.nth_root(q);
    if &num_traits::pow::pow(root.clone(), q as usize) != magnitude {
        return None;
    }
    let root = BigInt::from(root);
    Some(if negative { -root } else { root })
}

fn round_half_even(r: &BigRational) -> BigRational {
    let floor = r.floor();
    let frac = r - &floor;
    let half = BigRational::new(BigInt::from(1), BigInt::from(2));
    let one = BigRational::one();
    match frac.cmp(&half) {
        std::cmp::Ordering::Greater => floor + one,
        std::cmp::Ordering::Less => floor,
        std::cmp::Ordering::Equal => {
            if floor.to_integer() % 2u8 == BigInt::from(0) {
                floor
            } else {
                floor + one
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

/// How numbers are rendered back into the document.
#[derive(Clone, Debug)]
pub struct NumFormat {
    /// Names that denote units. The formatter needs these to decide whether a
    /// coefficient of 1 is meaningful: `1 hr` keeps its `1`, while the `1` in
    /// `1 x` is noise. Populated from the environment.
    pub units: std::sync::Arc<std::collections::HashSet<String>>,
    /// Decimal places, from the `@precision` directive. Defaults to 4.
    pub precision: usize,
    /// Whether to insert thousands separators, from `@group`.
    pub grouping: bool,
    /// Culture-dependent separators, from `@culture`.
    pub decimal_sep: char,
    pub group_sep: char,
}

impl Default for NumFormat {
    fn default() -> Self {
        NumFormat {
            units: std::sync::Arc::new(std::collections::HashSet::new()),
            precision: 4,
            grouping: true,
            decimal_sep: '.',
            group_sep: ',',
        }
    }
}

/// Above this magnitude, or below its reciprocal-ish counterpart, numbers
/// switch to scientific notation. `123,456,789,012,000` prints plain but
/// `1.2346e17` does not; `0.0001` prints plain but `1.2346e-7` does not.
const SCI_HIGH: f64 = 1e15;
const SCI_LOW: f64 = 1e-4;

impl Num {
    pub fn format(&self, fmt: &NumFormat) -> String {
        match self {
            Num::Flt(f) if f.is_nan() => return "NaN".to_string(),
            Num::Flt(f) if f.is_infinite() => {
                return if *f < 0.0 { "-Infinity" } else { "Infinity" }.to_string()
            }
            _ => {}
        }

        let magnitude = self.to_f64().abs();
        if magnitude != 0.0 && (magnitude >= SCI_HIGH || magnitude < SCI_LOW) {
            return self.format_scientific(fmt);
        }
        self.format_plain(fmt)
    }

    fn format_plain(&self, fmt: &NumFormat) -> String {
        let (negative, int_digits, frac_digits) = self.decimal_digits(fmt.precision);
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        if fmt.grouping {
            push_grouped(&mut out, &int_digits, fmt.group_sep);
        } else {
            out.push_str(&int_digits);
        }
        if !frac_digits.is_empty() {
            out.push(fmt.decimal_sep);
            out.push_str(&frac_digits);
        }
        out
    }

    fn format_scientific(&self, fmt: &NumFormat) -> String {
        let v = self.to_f64();
        let exp = v.abs().log10().floor() as i32;
        let mantissa = Num::from_f64(v / 10f64.powi(exp));
        let (negative, int_digits, frac_digits) = mantissa.decimal_digits(fmt.precision);
        let mut out = String::new();
        if negative {
            out.push('-');
        }
        out.push_str(&int_digits);
        if !frac_digits.is_empty() {
            out.push(fmt.decimal_sep);
            out.push_str(&frac_digits);
        }
        out.push('e');
        out.push_str(&exp.to_string());
        out
    }

    /// Decomposes into sign, integer digits and (trailing-zero-trimmed)
    /// fractional digits, rounded to `precision` places. Works from the exact
    /// rational when there is one, so long integers never lose digits.
    fn decimal_digits(&self, precision: usize) -> (bool, String, String) {
        let rat = match self {
            Num::Rat(r) => r.clone(),
            Num::Flt(f) => match BigRational::from_float(*f) {
                Some(r) => r,
                None => return (false, "0".to_string(), String::new()),
            },
        };
        let negative = rat.is_negative();
        let rat = rat.abs();

        let scale = num_traits::pow::pow(BigInt::from(10), precision);
        let scaled = round_half_even(&(rat * BigRational::from_integer(scale.clone())));
        let scaled = scaled.to_integer();

        let int_part = &scaled / &scale;
        let frac_part = &scaled % &scale;

        let mut frac_digits = String::new();
        if precision > 0 {
            frac_digits = format!("{:0>width$}", frac_part, width = precision);
            while frac_digits.ends_with('0') {
                frac_digits.pop();
            }
        }
        // Rounding to zero should not print as "-0".
        let negative = negative && !(int_part.is_zero() && frac_digits.is_empty());
        (negative, int_part.to_string(), frac_digits)
    }
}

fn push_grouped(out: &mut String, digits: &str, sep: char) {
    let len = digits.len();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(ch);
    }
}

impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format(&NumFormat::default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(n: Num) -> String {
        n.format(&NumFormat::default())
    }

    #[test]
    fn formats_grouping_and_precision() {
        assert_eq!(show(Num::from_i64(999)), "999");
        assert_eq!(show(Num::from_i64(1000)), "1,000");
        assert_eq!(show(Num::from_i64(1234567890)), "1,234,567,890");
        assert_eq!(show(Num::ratio(-22, 7)), "-3.1429");
        assert_eq!(show(Num::ratio(2, 3)), "0.6667");
        assert_eq!(show(Num::ratio(1, 2)), "0.5");
        assert_eq!(show(Num::from_i64(0)), "0");
    }

    #[test]
    fn trims_trailing_zeros_not_significant_digits() {
        // 123,456.789012 at precision 4 rounds to ...7890, and the trailing
        // zero is dropped. This exact case appears in the Reference document.
        let n = Num::Rat(BigRational::new(
            BigInt::from(123456789012i64),
            BigInt::from(1000000),
        ));
        assert_eq!(show(n), "123,456.789");
    }

    #[test]
    fn switches_to_scientific_at_the_extremes() {
        assert_eq!(show(Num::from_f64(1.0868e21)), "1.0868e21");
        // Straight from the Reference: `123,456.789012 W in nW`.
        assert_eq!(show(Num::from_f64(1.23456789012e14)), "123,456,789,012,000");
        assert_eq!(show(Num::from_f64(1.2346e-7)), "1.2346e-7");
        // 1.2346e-4 sits just above the threshold and prints plainly.
        assert_eq!(show(Num::from_f64(1.2346e-4)), "0.0001");
    }

    #[test]
    fn exact_arithmetic_survives_long_chains() {
        // 1/3 + 1/6 is exactly 1/2, not 0.49999999999999994.
        let third = Num::ratio(1, 3);
        let sixth = Num::ratio(1, 6);
        assert_eq!(show(third.add(&sixth)), "0.5");
        assert!(third.add(&sixth).is_exact());
    }

    #[test]
    fn big_integers_do_not_lose_digits() {
        let two = Num::from_i64(2);
        let p = two.pow(&Num::from_i64(32)).sub(&Num::one());
        assert_eq!(show(p), "4,294,967,295");
    }

    #[test]
    fn exact_roots_stay_exact() {
        assert_eq!(show(Num::from_i64(9).pow(&Num::ratio(1, 2))), "3");
        assert_eq!(show(Num::ratio(4, 9).pow(&Num::ratio(1, 2))), "0.6667");
        assert!(Num::from_i64(9).pow(&Num::ratio(1, 2)).is_exact());
        assert!(!Num::from_i64(2).pow(&Num::ratio(1, 2)).is_exact());
    }

    #[test]
    fn rounds_half_to_even() {
        assert_eq!(show(Num::ratio(3, 2).round()), "2");
        assert_eq!(show(Num::ratio(5, 2).round()), "2");
        assert_eq!(show(Num::from_f64(2.50001).round()), "3");
        assert_eq!(show(Num::ratio(21, 10).round()), "2");
        assert_eq!(show(Num::ratio(-21, 10).round()), "-2");
    }

    #[test]
    fn division_by_zero_is_infinity() {
        assert_eq!(show(Num::from_i64(5).div(&Num::zero())), "Infinity");
        assert_eq!(show(Num::from_i64(-5).div(&Num::zero())), "-Infinity");
    }

    #[test]
    fn floor_and_ceil_go_the_right_way_on_negatives() {
        assert_eq!(show(Num::ratio(-21, 10).floor()), "-3");
        assert_eq!(show(Num::ratio(-21, 10).ceil()), "-2");
        assert_eq!(show(Num::ratio(-21, 10).truncate()), "-2");
    }
}
