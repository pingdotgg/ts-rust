//! JavaScript-compatible `Number` operations and pseudo-bigint handling. This
//! was the `ts_jsnum` crate.

use std::{fmt, ops};

use num_bigint::{BigInt, BigUint, Sign};
use num_traits::ToPrimitive;

pub const MAX_SAFE_INTEGER: Number = Number(9_007_199_254_740_991.0);
pub const MIN_SAFE_INTEGER: Number = Number(-9_007_199_254_740_991.0);

#[must_use]
pub fn nan() -> Number {
    Number::nan()
}

#[must_use]
pub fn infinity(sign: i32) -> Number {
    Number::infinity(sign)
}

/// ECMAScript `StringToNumber` conversion.
#[must_use]
pub fn from_string(text: &str) -> Number {
    Number::from_string(text)
}

/// A JavaScript IEEE-754 `Number` value.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Number(pub f64);

impl Number {
    #[must_use]
    pub const fn new(value: f64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn value(self) -> f64 {
        self.0
    }

    // PORT: Go's `math.NaN()` has the bits 0x7ff8000000000001, Rust's
    // `f64::NAN` 0x7ff8000000000000. A NaN prints as `NaN`, so its bits do
    // not reach the output text.
    #[must_use]
    pub fn nan() -> Self {
        Self(f64::NAN)
    }

    #[must_use]
    pub fn infinity(sign: i32) -> Self {
        Self(if sign < 0 {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        })
    }

    #[must_use]
    pub fn is_nan(self) -> bool {
        self.0.is_nan()
    }

    #[must_use]
    pub fn is_infinite(self) -> bool {
        self.0.is_infinite()
    }

    /// ECMAScript `ToInt32`.
    #[must_use]
    pub fn to_int32(self) -> i32 {
        if !self.0.is_finite() || self.0 == 0.0 {
            return 0;
        }
        let integer = self.0.trunc() % 4_294_967_296.0;
        let unsigned = if integer < 0.0 {
            integer + 4_294_967_296.0
        } else {
            integer
        };
        unsigned.to_u32().unwrap_or(0).cast_signed()
    }

    /// ECMAScript `ToUint32`.
    #[must_use]
    pub fn to_uint32(self) -> u32 {
        self.to_int32().cast_unsigned()
    }

    #[must_use]
    pub fn signed_right_shift(self, other: Self) -> Self {
        Self(f64::from(self.to_int32() >> (other.to_uint32() & 31)))
    }

    #[must_use]
    pub fn unsigned_right_shift(self, other: Self) -> Self {
        Self(f64::from(self.to_uint32() >> (other.to_uint32() & 31)))
    }

    #[must_use]
    pub fn left_shift(self, other: Self) -> Self {
        Self(f64::from(
            self.to_int32().wrapping_shl(other.to_uint32() & 31),
        ))
    }

    #[must_use]
    pub fn bitwise_not(self) -> Self {
        Self(f64::from(!self.to_int32()))
    }

    #[must_use]
    pub fn bitwise_or(self, other: Self) -> Self {
        Self(f64::from(self.to_int32() | other.to_int32()))
    }

    #[must_use]
    pub fn bitwise_and(self, other: Self) -> Self {
        Self(f64::from(self.to_int32() & other.to_int32()))
    }

    #[must_use]
    pub fn bitwise_xor(self, other: Self) -> Self {
        Self(f64::from(self.to_int32() ^ other.to_int32()))
    }

    #[must_use]
    pub fn floor(self) -> Self {
        Self(self.0.floor())
    }

    #[must_use]
    pub fn abs(self) -> Self {
        Self(self.0.abs())
    }

    /// ECMAScript Number remainder operation.
    #[must_use]
    pub fn remainder(self, divisor: Self) -> Self {
        match () {
            () if self.is_nan() || divisor.is_nan() || self.is_infinite() => Self::nan(),
            () if divisor.is_infinite() => self,
            () if divisor.0 == 0.0 => Self::nan(),
            () if self.0 == 0.0 => self,
            () => Self(self.0 % divisor.0),
        }
    }

    /// ECMAScript Number exponentiation operation, as the evaluator folds `**`.
    ///
    /// Go: jsnum/jsnum.go:147 Number.Exponentiate. The result is Go's to the
    /// bit: Go's `math.Pow` (ported in `go_math`) and not the C library's
    /// `pow`, which can differ in the last bit (`10 ** -23`).
    #[must_use]
    pub fn exponentiate(self, exponent: Self) -> Self {
        let b = self.0;
        let e = exponent.0;
        if (b == 1.0 || b == -1.0) && e.is_infinite() || b == 1.0 && e.is_nan() {
            return Self::nan();
        }

        // Integer base and exponent with a result over 53 bits: exact big.Int
        // power. `i64::MAX as f64` is 2^63, as Go's `float64(math.MaxInt64)`.
        if b >= i64::MIN as f64
            && b <= i64::MAX as f64
            && b == b.trunc()
            && e >= 0.0
            && e <= i64::MAX as f64
            && e == e.trunc()
            && !e.is_infinite()
        {
            let magnitude = e * go_math::log2(b.abs());
            if magnitude > 53.0 && magnitude <= go_math::log2(f64::MAX) {
                // |b| >= 2 here, so the magnitude limit keeps e <= 1024.
                let exact = BigInt::from(go_math::int64(b)).pow(go_math::int64(e) as u32);
                return Self(big_float_to_f64(&exact, 256));
            }
        }
        Self(go_math::pow(b, e))
    }

    /// ECMAScript `StringToNumber` conversion.
    #[must_use]
    pub fn from_string(text: &str) -> Self {
        string_to_number(text)
    }
}

impl fmt::Display for Number {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut buffer = ryu_js::Buffer::new();
        formatter.write_str(buffer.format(self.0))
    }
}

impl From<f64> for Number {
    fn from(value: f64) -> Self {
        Self(value)
    }
}

impl From<Number> for f64 {
    fn from(value: Number) -> Self {
        value.0
    }
}

macro_rules! arithmetic {
    ($trait:ident, $method:ident, $operator:tt) => {
        impl ops::$trait for Number {
            type Output = Self;

            fn $method(self, other: Self) -> Self {
                Self(self.0 $operator other.0)
            }
        }
    };
}

arithmetic!(Add, add, +);
arithmetic!(Sub, sub, -);
arithmetic!(Mul, mul, *);
arithmetic!(Div, div, /);

impl ops::Neg for Number {
    type Output = Self;

    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl ops::Rem for Number {
    type Output = Self;

    fn rem(self, other: Self) -> Self {
        self.remainder(other)
    }
}

fn string_to_number(text: &str) -> Number {
    let text = text.trim_matches(is_js_whitespace);
    match text {
        "" => return Number(0.0),
        "Infinity" | "+Infinity" => return Number::infinity(1),
        "-Infinity" => return Number::infinity(-1),
        _ => {}
    }
    if !text.chars().all(is_number_character) {
        return Number::nan();
    }
    if let Some(number) = parse_integer_literal(text) {
        return number;
    }

    let (unsigned, negative) = text
        .strip_prefix('-')
        .map_or((text, false), |value| (value, true));
    let unsigned = if negative {
        unsigned
    } else {
        unsigned.strip_prefix('+').unwrap_or(unsigned)
    };
    if !unsigned
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit() || character == '.')
    {
        return Number::nan();
    }
    let Some(value) = parse_decimal_float(unsigned) else {
        return Number::nan();
    };
    Number(if negative { -value } else { value })
}

fn parse_integer_literal(text: &str) -> Option<Number> {
    let prefix = text.get(0..2).unwrap_or_default();
    let (radix, digits, prefixed) = match prefix {
        "0b" | "0B" => (2, &text[2..], true),
        "0o" | "0O" => (8, &text[2..], true),
        "0x" | "0X" => (16, &text[2..], true),
        _ => (10, text, false),
    };
    if prefixed && (digits.is_empty() || !digits.chars().all(|character| character.is_digit(radix)))
    {
        return Some(Number::nan());
    }
    if !prefixed && !digits.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    if let Some(value) = parse_safe_integer(digits, radix) {
        return Some(Number(value as f64));
    }
    // Go: `new(big.Int).SetString(s, 0)` then `bi.Float64()` (jsnum/string.go
    // tryParseInt), which does not round in `SetInt`.
    let integer = BigInt::parse_bytes(digits.as_bytes(), radix)?;
    Some(Number(big_float_to_f64(&integer, integer.bits())))
}

/// The value of `digits` (valid digits of `radix`) when it is at most
/// 2^53, so that the `f64` is exact and equals the `BigInt` path's.
// PERF: Go `jsnum.FromString` tries `strconv.ParseInt` first too; most
// literals are small. The Effect plugin's re-checks parse each literal again
// (effperf3 item 8).
fn parse_safe_integer(digits: &str, radix: u32) -> Option<u64> {
    let mut value: u64 = 0;
    for byte in digits.bytes() {
        // `value` is at most 2^53 here, so this does not overflow.
        value = value * u64::from(radix) + u64::from(char::from(byte).to_digit(radix)?);
        if value > 1 << 53 {
            return None;
        }
    }
    Some(value)
}

fn parse_decimal_float(text: &str) -> Option<f64> {
    let (mantissa, exponent) = text.find(['e', 'E']).map_or((text, None), |index| {
        (&text[..index], Some(&text[index + 1..]))
    });
    if mantissa.matches('.').count() > 1 {
        return None;
    }
    let (integer, fraction) = mantissa
        .split_once('.')
        .map_or((mantissa, None), |(integer, fraction)| {
            (integer, Some(fraction))
        });
    if integer.is_empty() && fraction.is_none_or(str::is_empty)
        || !integer.chars().all(|character| character.is_ascii_digit())
        || fraction.is_some_and(|value| !value.chars().all(|character| character.is_ascii_digit()))
    {
        return None;
    }
    if let Some(exponent) = exponent {
        let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if digits.is_empty() || !digits.chars().all(|character| character.is_ascii_digit()) {
            return None;
        }
    }
    text.parse().ok()
}

fn is_number_character(character: char) -> bool {
    character.is_ascii_digit()
        || matches!(
            character,
            'a'..='f' | 'A'..='F' | '.' | '-' | '+' | 'x' | 'X' | 'o' | 'O'
        )
}

fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r'
            | '\t'
            | '\u{000b}'
            | '\u{000c}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// A decimal representation of a JavaScript bigint without requiring bigint
/// arithmetic at call sites.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PseudoBigInt {
    pub negative: bool,
    pub base10_value: String,
}

impl PseudoBigInt {
    #[must_use]
    pub fn new(value: &str, negative: bool) -> Self {
        let value = value.trim_start_matches('0').to_owned();
        Self {
            negative: negative && !value.is_empty(),
            base10_value: value,
        }
    }

    #[must_use]
    pub fn sign(&self) -> i8 {
        if self.base10_value.is_empty() {
            0
        } else if self.negative {
            -1
        } else {
            1
        }
    }

    // Go: jsnum/pseudobigint.go:41 PseudoBigInt.Compare (ts#64280)
    /// Compares two bigints by value: -1, 0 or 1.
    #[must_use]
    pub fn compare(&self, other: &PseudoBigInt) -> i32 {
        let c = self.sign().cmp(&other.sign()) as i32;
        if c != 0 {
            return c;
        }
        // PORT: Go `len` is the byte length; the value is ASCII digits.
        let mut c = self.base10_value.len().cmp(&other.base10_value.len()) as i32;
        if c == 0 {
            c = self.base10_value.as_str().cmp(other.base10_value.as_str()) as i32;
        }
        if self.negative {
            c = -c;
        }
        c
    }

    /// Parses a scanner-validated signed bigint literal.
    #[must_use]
    pub fn parse_valid(text: &str) -> Self {
        let (text, negative) = text
            .strip_prefix('-')
            .map_or((text, false), |text| (text, true));
        Self::new(&parse_pseudo_big_int(text), negative)
    }
}

impl fmt::Display for PseudoBigInt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.base10_value.is_empty() {
            formatter.write_str("0")
        } else if self.negative {
            write!(formatter, "-{}", self.base10_value)
        } else {
            formatter.write_str(&self.base10_value)
        }
    }
}

/// Converts a scanner-validated bigint literal to unsigned decimal text.
///
/// # Panics
///
/// Panics when a non-decimal literal is malformed.
#[must_use]
pub fn parse_pseudo_big_int(text: &str) -> String {
    let text = text.strip_suffix('n').unwrap_or(text);
    let prefix = text.get(0..2).unwrap_or_default();
    let radix = match prefix {
        "0b" | "0B" => Some(2),
        "0o" | "0O" => Some(8),
        "0x" | "0X" => Some(16),
        _ => None,
    };
    let Some(radix) = radix else {
        let value = text.trim_start_matches('0');
        return if value.is_empty() { "0" } else { value }.to_owned();
    };
    let digits = text[2..].replace('_', "");
    BigInt::parse_bytes(digits.as_bytes(), radix)
        .unwrap_or_else(|| panic!("Failed to parse big int: {text:?}"))
        .to_string()
}

/// Go's `new(big.Float).SetPrec(precision).SetInt(value).Float64()`
/// (math/big/float.go:601 and :961): `SetInt` rounds `value` to `precision`
/// bits, then `Float64` rounds that to 53 bits, both half to even. The `f64`
/// of the result is exact, or ±Inf from 2^1024 up. `Exponentiate` uses
/// precision 256. `big.Int.Float64` (the literal parser) is the case
/// `precision >= value.bits()`: one rounding.
///
/// Do not use num-bigint's `to_f64`: its sticky bit can miss 1 bits below
/// the top 64, so a tie can round down where Go rounds up (gopow1 skeptic).
fn big_float_to_f64(value: &BigInt, precision: u64) -> f64 {
    let (kept, first_shift) = round_half_even(value.magnitude(), precision);
    let (kept, second_shift) = round_half_even(&kept, 53);
    // At most 2^53 (a carry), so the conversion is exact.
    let mantissa = kept.to_u64().unwrap_or_default();
    if mantissa == 0 {
        return 0.0;
    }
    // The value is `mantissa << shift`, and `top` is its bit length. Go:
    // `e := r.exp - 1`, Inf when `e > 1023`.
    let shift = first_shift + second_shift;
    let top = u64::from(64 - mantissa.leading_zeros()) + shift;
    let magnitude = if top > 1024 {
        f64::INFINITY
    } else {
        // Go: `msb64(r.mant) >> ebits` puts the leading 1 at bit 52, and the
        // mask drops it. The bits that this shift drops are zero.
        let fraction = ((mantissa << mantissa.leading_zeros()) >> 11) & ((1 << 52) - 1);
        f64::from_bits(((top - 1 + 1023) << 52) | fraction)
    };
    if value.sign() == Sign::Minus {
        -magnitude
    } else {
        magnitude
    }
}

/// Go's `big.Float.round` (math/big/float.go:400) in its default mode,
/// ToNearestEven, for an integer: the top `precision` bits of `magnitude`,
/// one higher when the dropped bits are over half, or exactly half and the
/// kept bits are odd. Returns the kept bits and the count of dropped bits. A
/// carry gives `1 << precision`, the same value as Go's `exp++`.
fn round_half_even(magnitude: &BigUint, precision: u64) -> (BigUint, u64) {
    let shift = magnitude.bits().saturating_sub(precision);
    if shift == 0 {
        return (magnitude.clone(), 0);
    }
    let mut kept = magnitude >> shift;
    // Go's rbit is the top dropped bit; its sbit is 1 when any bit below is 1.
    let round_bit = magnitude.bit(shift - 1);
    let sticky_bit = magnitude
        .trailing_zeros()
        .is_some_and(|zeros| zeros < shift - 1);
    if round_bit && (sticky_bit || kept.bit(0)) {
        kept += 1_u8;
    }
    (kept, shift)
}

/// The Go `math` functions that `**` folding uses, ported from Go 1.27
/// (`src/math`, the toolchain of the pinned tsgo) so that folded numbers equal
/// tsgo's to the bit. Log and Exp follow the amd64 assembly (`log_amd64.s`,
/// `exp_amd64.s`) that tsgo runs on x86-64. Like Go's, Exp uses fused
/// multiply-add when the CPU has AVX and FMA. A build for another target
/// (arm64 too) runs this amd64 math with the FMA path, so it equals Go on
/// amd64 with FMA (the oracle's platform), not Go on arm64: there Go's Log
/// is the pure Go `log.go` and its Exp is `exp_arm64.s`. No other code in
/// this module depends on the CPU.
mod go_math {
    const SHIFT: u32 = 52;
    const MASK: u64 = 0x7ff;
    const BIAS: i64 = 1023;
    const SIGN_MASK: u64 = 1 << 63;
    const POS_INF_BITS: u64 = 0x7ff0_0000_0000_0000;

    /// Go's `math.Pow` (pow.go): exact special cases, `Exp(yf*Log(x))` for the
    /// fraction of `y`, and repeated squaring for its integer part.
    pub(super) fn pow(x: f64, y: f64) -> f64 {
        pow_with(x, y, use_fma())
    }

    /// Go's `math.Log2` (log10.go): exact for powers of two.
    pub(super) fn log2(x: f64) -> f64 {
        let (frac, exp) = frexp(x);
        if frac == 0.5 {
            return (exp - 1) as f64;
        }
        // Go's constant `1/Ln2` is exact before it rounds: LOG2_E.
        log(frac) * std::f64::consts::LOG2_E + exp as f64
    }

    /// Go's `int64(x)` on amd64 (CVTTSD2SQ): a value out of range gives
    /// `math.MinInt64`, where Rust's `as` saturates. In `exponentiate`, only
    /// a base of exactly 2^63 is out of range.
    pub(super) fn int64(x: f64) -> i64 {
        if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&x) {
            x as i64
        } else {
            i64::MIN
        }
    }

    /// Go's `useFMA` (exp_amd64.go): `cpu.X86.HasAVX && cpu.X86.HasFMA`.
    fn use_fma() -> bool {
        #[cfg(target_arch = "x86_64")]
        {
            std::arch::is_x86_feature_detected!("avx") && std::arch::is_x86_feature_detected!("fma")
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            true
        }
    }

    pub(super) fn pow_with(x: f64, y: f64, fma: bool) -> f64 {
        if y == 0.0 || x == 1.0 {
            return 1.0;
        } else if y == 1.0 {
            return x;
        } else if x.is_nan() || y.is_nan() {
            return f64::NAN;
        } else if x == 0.0 {
            if y < 0.0 {
                return if x.is_sign_negative() && is_odd_int(y) {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                };
            } else if y > 0.0 {
                return if x.is_sign_negative() && is_odd_int(y) {
                    x
                } else {
                    0.0
                };
            }
        } else if y.is_infinite() {
            return if x == -1.0 {
                1.0
            } else if (x.abs() < 1.0) == (y == f64::INFINITY) {
                0.0
            } else {
                f64::INFINITY
            };
        } else if x.is_infinite() {
            if x == f64::NEG_INFINITY {
                return pow_with(1.0 / x, -y, fma); // Pow(-0, -y)
            }
            if y < 0.0 {
                return 0.0;
            } else if y > 0.0 {
                return f64::INFINITY;
            }
        } else if y == 0.5 {
            return x.sqrt();
        } else if y == -0.5 {
            return 1.0 / x.sqrt();
        }

        let (mut yi, mut yf) = modf(y.abs());
        if yf != 0.0 && x < 0.0 {
            return f64::NAN;
        }
        if yi >= 9_223_372_036_854_775_808.0 {
            // yi is a large even int that will lead to overflow (or underflow
            // to 0) for all x except -1 (x == 1 was handled earlier).
            return if x == -1.0 {
                1.0
            } else if (x.abs() < 1.0) == (y > 0.0) {
                0.0
            } else {
                f64::INFINITY
            };
        }

        // ans = a1 * 2**ae (= 1 for now).
        let mut a1 = 1.0;
        let mut ae: i64 = 0;

        // ans *= x**yf
        if yf != 0.0 {
            if yf > 0.5 {
                yf -= 1.0;
                yi += 1.0;
            }
            a1 = exp_with(yf * log(x), fma);
        }

        // ans *= x**yi by multiplying in successive squarings of x according
        // to the bits of yi. Powers of two go into ae.
        let (mut x1, mut xe) = frexp(x);
        let mut i = yi as i64;
        while i != 0 {
            if !(-(1 << 12)..=(1 << 12)).contains(&xe) {
                // xe would overflow the shift below. i still has a bit set, so
                // ae is past the float64 range and Ldexp gives 0 or Inf.
                ae += xe;
                break;
            }
            if (i & 1) == 1 {
                a1 *= x1;
                ae += xe;
            }
            x1 *= x1;
            xe <<= 1;
            if x1 < 0.5 {
                x1 += x1;
                xe -= 1;
            }
            i >>= 1;
        }

        // ans = a1*2**ae; if y < 0 { ans = 1 / ans }, in the opposite order.
        if y < 0.0 {
            a1 = 1.0 / a1;
            ae = -ae;
        }
        ldexp(a1, ae)
    }

    fn is_odd_int(x: f64) -> bool {
        if x.abs() >= (1_u64 << 53) as f64 {
            // Every float64 at or over 2^53 is an even integer.
            return false;
        }
        let (xi, xf) = modf(x);
        xf == 0.0 && (xi as i64 & 1) == 1
    }

    fn modf(f: f64) -> (f64, f64) {
        let integer = f.trunc();
        (integer, (f - integer).copysign(f))
    }

    fn normalize(x: f64) -> (f64, i64) {
        const SMALLEST_NORMAL: f64 = 2.2250738585072014e-308; // 2**-1022
        if x.abs() < SMALLEST_NORMAL {
            return (x * (1_u64 << 52) as f64, -52);
        }
        (x, 0)
    }

    fn frexp(f: f64) -> (f64, i64) {
        if f == 0.0 || f.is_infinite() || f.is_nan() {
            return (f, 0);
        }
        let (f, mut exp) = normalize(f);
        let mut x = f.to_bits();
        exp += ((x >> SHIFT) & MASK) as i64 - BIAS + 1;
        x &= !(MASK << SHIFT);
        x |= ((-1 + BIAS) as u64) << SHIFT;
        (f64::from_bits(x), exp)
    }

    fn ldexp(frac: f64, exp: i64) -> f64 {
        if frac == 0.0 || frac.is_infinite() || frac.is_nan() {
            return frac;
        }
        let (frac, e) = normalize(frac);
        let mut exp = exp + e;
        let mut x = frac.to_bits();
        exp += ((x >> SHIFT) & MASK) as i64 - BIAS;
        if exp < -1075 {
            return 0.0_f64.copysign(frac); // underflow
        }
        if exp > 1023 {
            return if frac < 0.0 {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            };
        }
        let mut m = 1.0;
        if exp < -1022 {
            // denormal
            exp += 53;
            m = 1.0 / (1_u64 << 53) as f64;
        }
        x &= !(MASK << SHIFT);
        x |= ((exp + BIAS) as u64) << SHIFT;
        m * f64::from_bits(x)
    }

    /// Go's `archLog` (log_amd64.s). Unlike Go's pure `log`, it does not
    /// normalize a subnormal `x`.
    fn log(x: f64) -> f64 {
        const HSQRT2: f64 = 7.07106781186547524401e-01; // sqrt(2)/2
        const LN2_HI: f64 = 6.93147180369123816490e-01; // 0x3fe62e42fee00000
        const LN2_LO: f64 = 1.90821492927058770002e-10; // 0x3dea39ef35793c76
        const L1: f64 = 6.666666666666735130e-01; // 0x3FE5555555555593
        const L2: f64 = 3.999999999940941908e-01; // 0x3FD999999997FA04
        const L3: f64 = 2.857142874366239149e-01; // 0x3FD2492494229359
        const L4: f64 = 2.222219843214978396e-01; // 0x3FCC71C51D8E78AF
        const L5: f64 = 1.818357216161805012e-01; // 0x3FC7466496CB03DE
        const L6: f64 = 1.531383769920937332e-01; // 0x3FC39A09D078C69F
        const L7: f64 = 1.479819860511658591e-01; // 0x3FC2F112DF3E5244

        let bits = x.to_bits();
        if bits & !SIGN_MASK == 0 {
            return f64::NEG_INFINITY;
        }
        if bits & SIGN_MASK != 0 {
            return f64::from_bits(0x7ff8_0000_0000_0001);
        }
        if bits >= POS_INF_BITS {
            return x; // +Inf or NaN
        }
        // f1, ki := math.Frexp(x); k := float64(ki), from the bits.
        let mut f1 = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0.5_f64.to_bits());
        let mut k = f64::from(((bits >> SHIFT) & MASK) as i32 - 0x3fe);
        // if f1 < math.Sqrt2/2 { k -= 1; f1 *= 2 }. CMPSD 5 (not less than)
        // also takes f1 == Sqrt2/2.
        if f1 <= HSQRT2 {
            k -= 1.0;
            f1 *= 2.0;
        }
        let f = f1 - 1.0;
        let s = f / (2.0 + f);
        let s2 = s * s;
        let s4 = s2 * s2;
        let t1 = s2 * (((L7 * s4 + L5) * s4 + L3) * s4 + L1);
        let t2 = s4 * ((L6 * s4 + L4) * s4 + L2);
        let r = t1 + t2;
        let hfsq = 0.5 * f * f;
        k * LN2_HI - ((hfsq - (s * (hfsq + r) + k * LN2_LO)) - f)
    }

    /// Go's `archExp` (exp_amd64.s): a reduced argument, a Taylor series of
    /// e^(r/16), four squarings, then a scale by 2^k. With `fma`, the steps
    /// that Go fuses (VFNMADD231SD, VFMADD213SD) round once.
    pub(super) fn exp_with(x: f64, fma: bool) -> f64 {
        const LOG2E: f64 = 1.4426950408889634073599246810018920; // 1/LN2
        const LN2U: f64 = 0.69314718055966295651160180568695068359375; // upper half LN2
        const LN2L: f64 = 0.28235290563031577122588448175013436025525412068e-12; // lower half LN2
        const OVERFLOW: f64 = 7.09782712893384e+02;
        const T2: f64 = 1.6666666666666666667e-1;
        const T3: f64 = 4.1666666666666666667e-2;
        const T4: f64 = 8.3333333333333333333e-3;
        const T5: f64 = 1.3888888888888888889e-3;
        const T6: f64 = 1.9841269841269841270e-4;
        const T7: f64 = 2.4801587301587301587e-5;

        let bits = x.to_bits();
        if bits & !SIGN_MASK >= POS_INF_BITS {
            // -Inf gives 0. NaN and +Inf give x.
            return if bits == f64::NEG_INFINITY.to_bits() {
                0.0
            } else {
                x
            };
        }
        if x > OVERFLOW {
            return f64::INFINITY;
        }
        // CVTSD2SL rounds to nearest even. Out of i32 range it gives i32::MIN,
        // which underflows below.
        let scaled = LOG2E * x;
        let mut k = if (-2_147_483_648.0..2_147_483_648.0).contains(&scaled.round_ties_even()) {
            scaled.round_ties_even() as i32
        } else {
            i32::MIN
        };
        let kf = f64::from(k);
        let mut r;
        if fma {
            r = (-kf).mul_add(LN2U, x);
            r = (-kf).mul_add(LN2L, r);
            r *= 0.0625;
            let mut p = T7;
            for c in [T6, T5, T4, T3, T2, 0.5, 1.0] {
                p = p.mul_add(r, c);
            }
            r *= p;
            for _ in 0..3 {
                r *= r + 2.0;
            }
            r = (r + 2.0).mul_add(r, 1.0);
        } else {
            r = x - LN2U * kf;
            r -= LN2L * kf;
            r *= 0.0625;
            let mut p = T7;
            for c in [T6, T5, T4, T3, T2, 0.5, 1.0] {
                p = p * r + c;
            }
            r *= p;
            for _ in 0..4 {
                r *= r + 2.0;
            }
            r += 1.0;
        }
        // return fr * 2**k
        k += 0x3ff;
        let mut scale_bits = 1;
        if k <= 0 {
            // denormal
            if k < -52 {
                return 0.0;
            }
            r *= f64::from_bits(u64::from((k + 0x3fe).cast_unsigned()) << SHIFT);
        } else if k >= 0x7ff {
            return f64::INFINITY;
        } else {
            scale_bits = k;
        }
        r * f64::from_bits(u64::from(scale_bits.cast_unsigned()) << SHIFT)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BigInt, MAX_SAFE_INTEGER, MIN_SAFE_INTEGER, Number, PseudoBigInt, parse_pseudo_big_int,
    };

    // The expected bits are Go's: `math.Float64bits(float64(jsnum.FromString(text)))`
    // at pin 3e9236361fa9 (Go 1.27.1, amd64). The last five literals are
    // integers over 2^64 with a tie and one far 1 bit at the 53-bit rounding
    // point: Go rounds them up, and num-bigint's `to_f64` (the old expected
    // value of this test) rounds them down.
    #[test]
    fn small_integer_literals_parse_like_bigint() {
        for (text, go_bits) in [
            ("0", 0x0000_0000_0000_0000),
            ("00", 0x0000_0000_0000_0000),
            ("7", 0x401c_0000_0000_0000),
            ("0012", 0x4028_0000_0000_0000),
            ("9007199254740991", 0x433f_ffff_ffff_ffff),
            ("9007199254740992", 0x4340_0000_0000_0000),
            ("9007199254740993", 0x4340_0000_0000_0000),
            ("18446744073709551616", 0x43f0_0000_0000_0000),
            ("0x1F", 0x403f_0000_0000_0000),
            ("0X0000ff", 0x406f_e000_0000_0000),
            ("0b101", 0x4014_0000_0000_0000),
            ("0o777", 0x407f_f000_0000_0000),
            ("0x20000000000000", 0x4340_0000_0000_0000),
            ("0x20000000000001", 0x4340_0000_0000_0000),
            ("0xFFFFFFFFFFFFFFFFFF", 0x4470_0000_0000_0000),
            ("0X100DBB81EB0C348000000000080000000", 0x47f0_0dbb_81eb_0c35),
            (
                "341423214327874980323079271629654589440",
                0x47f0_0dbb_81eb_0c35,
            ),
            (
                "0o4006673403654141510000000000000020000000000",
                0x47f0_0dbb_81eb_0c35,
            ),
            ("0x10913d7b51169a8000800000000000000", 0x47f0_913d_7b51_169b),
            (
                "352348447587066043971605188579392749568",
                0x47f0_913d_7b51_169b,
            ),
        ] {
            assert_eq!(Number::from_string(text).0.to_bits(), go_bits, "{text}");
        }
    }

    fn assert_number(actual: Number, expected: Number) {
        if expected.is_nan() {
            assert!(actual.is_nan());
        } else {
            assert_eq!(actual.0.to_bits(), expected.0.to_bits());
        }
    }

    #[test]
    fn converts_to_int32_and_performs_bitwise_operations() {
        let cases = [
            (Number(0.0), 0),
            (Number::nan(), 0),
            (Number::infinity(1), 0),
            (Number(2_147_483_648.0), i32::MIN),
            (MAX_SAFE_INTEGER, -1),
            (MIN_SAFE_INTEGER, 1),
            (Number(-0.4), 0),
            (Number(f64::MAX), 0),
        ];
        for (input, expected) in cases {
            assert_eq!(input.to_int32(), expected);
        }
        assert_eq!(
            Number(f64::from(0xdead_beef_u32)).bitwise_not(),
            Number(559_038_736.0)
        );
        assert_eq!(
            Number(-1.0).unsigned_right_shift(Number(0.0)),
            Number(4_294_967_295.0)
        );
        assert_eq!(Number(1.0).left_shift(Number(33.0)), Number(2.0));
    }

    #[test]
    fn implements_remainder_and_exponentiation_special_cases() {
        assert!(Number(1.0).remainder(Number(0.0)).is_nan());
        assert!(Number::infinity(1).remainder(Number(2.0)).is_nan());
        assert_eq!(Number(2.0).remainder(Number::infinity(1)), Number(2.0));
        assert_eq!(
            Number(-0.0).remainder(Number(3.0)).0.to_bits(),
            (-0.0_f64).to_bits()
        );
        assert!(Number(-1.0).exponentiate(Number::infinity(1)).is_nan());
        assert!(Number(1.0).exponentiate(Number::nan()).is_nan());
        assert_eq!(
            Number(2.0).exponentiate(Number(60.0)),
            Number(1_152_921_504_606_846_976.0)
        );
    }

    // Expected bits are Go's: tsc/internal/jsnum Exponentiate at pin
    // 3e9236361fa9 with Go 1.27.1 on amd64, with and without FMA (equal here).
    // The C library's pow gives the last bit differently for the first five.
    #[test]
    fn exponentiate_matches_go() {
        let cases = [
            (27.0, 1.0 / 3.0, 0x4007_ffff_ffff_ffff), // 2.9999999999999996, not 3
            (10.0, -23.0, 0x3b28_2db3_4012_b252),     // 1.0000000000000001e-23
            (1.1, 10.0, 0x4004_bffc_0c03_023d),       // 2.593742460100002
            (2.0, -0.5, 0x3fe6_a09e_667f_3bcc),       // 0.7071067811865475
            (2.0, 1.5, 0x4006_a09e_667f_3bcc),        // 2.82842712474619
            (2.0, 0.5, 0x3ff6_a09e_667f_3bcd),
            (7.0, -5.0, 0x3f0f_31d2_b366_47fc),
            (10.0, 22.0, 0x4480_f0cf_064d_d592),
            (10.0, 23.0, 0x44b5_2d02_c7e1_4af6),
            (3.0, 40.0, 0x43e5_1716_8a45_23fd),
            (-3.0, 41.0, 0xc3ff_a2a1_cf67_b5fc),
            (3.0, 646.0, 0x7fed_906a_378b_5987),
            (3.0, 647.0, f64::INFINITY.to_bits()),
            // Ties after the 256-bit rounding that num-bigint's to_f64 rounds
            // down (gopow1 skeptic).
            (21_265.0, 58.0, 0x740c_3b21_e289_64df),
            (228_861.0, 36.0, 0x67fe_de2f_6ded_4ea1),
            (321_798.0, 7.0, 0x47f0_cd57_547b_4e59),
            (-321_798.0, 7.0, 0xc7f0_cd57_547b_4e59),
            (10_836.0, 43.0, 0x63f4_6d5e_7794_4e7d),
            (2.0, -1074.0, 1),
            (2.0, -1075.0, 0),
            // Go's int64(2^63) is math.MinInt64 on amd64, so the sign flips.
            (9_223_372_036_854_775_808.0, 1.0, 0xc3e0_0000_0000_0000),
            (9_223_372_036_854_775_808.0, 3.0, 0xcbc0_0000_0000_0000),
            (-9_223_372_036_854_775_808.0, 1.0, 0xc3e0_0000_0000_0000),
        ];
        for (base, exponent, bits) in cases {
            let result = Number(base).exponentiate(Number(exponent)).0;
            assert_eq!(result.to_bits(), bits, "{base} ** {exponent} = {result}");
        }
    }

    // Expected bits are Go's (pin 3e9236361fa9, Go 1.27.1):
    // `new(big.Float).SetPrec(256).SetInt(x).Float64()`, `x.Float64()` and
    // `jsnum.FromString`.
    #[test]
    fn big_integers_round_like_go_big_float() {
        // An even top, a tie at the 53-bit point, then one far 1 bit: Go
        // rounds up. num-bigint's to_f64 does not see the far bit and rounds
        // down to even.
        let even = BigInt::from((1_u64 << 52) | 2);
        let far = (even << 77_u8) | (BigInt::from(1) << 76_u8) | (BigInt::from(1) << 10_u8);
        // An odd top, a 0, then 203 ones: the 256-bit rounding makes a tie,
        // which rounds up to even. One rounding goes down.
        let odd = BigInt::from((1_u64 << 52) | 1);
        let ones = (odd << 204_u8) | ((BigInt::from(1) << 203_u8) - 1);
        // Rounds to 2^1024 (Inf) in two steps, and to f64::MAX in one.
        let edge = (BigInt::from(1) << 1024_u16) - (BigInt::from(1) << 970_u16) - 1;
        let cases = [
            (&far, 0x4800_0000_0000_0003, 0x4800_0000_0000_0003),
            (&ones, 0x4ff0_0000_0000_0002, 0x4ff0_0000_0000_0001),
            (&edge, f64::INFINITY.to_bits(), f64::MAX.to_bits()),
        ];
        for (value, precision_256, single) in cases {
            assert_eq!(super::big_float_to_f64(value, 256).to_bits(), precision_256);
            assert_eq!(
                super::big_float_to_f64(value, value.bits()).to_bits(),
                single
            );
            let negative = -value;
            let bits = super::big_float_to_f64(&negative, 256).to_bits();
            assert_eq!(bits, precision_256 | 1 << 63);
        }
        for literal in [
            "0x200000000000050000000000000000400",
            "680564733841877304716067844435153519616",
        ] {
            assert_eq!(
                Number::from_string(literal).0.to_bits(),
                0x4800_0000_0000_0003
            );
        }
    }

    // Go: math/all_test.go TestPow (Go 1.27). Go checks Pow(10, vf[i]) within
    // a tolerance; these are the exact bits of Go on amd64.
    #[test]
    fn go_math_pow_matches_go_test_pow() {
        use super::go_math::pow_with;
        use std::f64::consts::PI;
        let pow10 = [
            (4.979_011_924_883_673_5, 0x40f7_4323_b8db_f69f),
            (7.738_872_474_578_104_5, 0x418a_22de_7ad2_f19c),
            (-0.276_880_057_192_001_59, 0x3fe0_ea38_20d4_d0e1),
            (-5.010_603_618_271_074_9, 0x3ee4_7737_260a_ef1f),
            (9.636_293_707_198_417_3, 0x41f0_1f90_5495_8966),
            (2.926_377_239_243_964_6, 0x408a_608a_7b55_16d4),
            (5.229_083_431_459_306_6, 0x4104_afd2_a97f_13ca),
            (2.727_939_910_436_010_2, 0x4080_b3ec_579b_da20),
            (1.825_308_091_680_855, 0x4050_b86f_c2f5_d9b5),
            (-8.685_924_768_575_601_3, 0x3e21_b428_7ac6_cfa6),
        ];
        let inf = f64::INFINITY;
        let nan = f64::NAN;
        let next_up = f64::from_bits(1.0_f64.to_bits() + 1);
        let next_down = f64::from_bits(1.0_f64.to_bits() - 1);
        // vfpowSC and powSC.
        let special = [
            (-inf, -PI, 0.0),
            (-inf, -3.0, -0.0),
            (-inf, -0.0, 1.0),
            (-inf, 0.0, 1.0),
            (-inf, 1.0, -inf),
            (-inf, 3.0, -inf),
            (-inf, PI, inf),
            (-inf, 0.5, inf),
            (-inf, nan, nan),
            (-PI, -inf, 0.0),
            (-PI, -PI, nan),
            (-PI, -0.0, 1.0),
            (-PI, 0.0, 1.0),
            (-PI, 1.0, -PI),
            (-PI, PI, nan),
            (-PI, inf, inf),
            (-PI, nan, nan),
            (-1.0, -inf, 1.0),
            (-1.0, inf, 1.0),
            (-1.0, nan, nan),
            (-0.5, -inf, inf),
            (-0.5, inf, 0.0),
            (-0.0, -inf, inf),
            (-0.0, -PI, inf),
            (-0.0, -0.5, inf),
            (-0.0, -3.0, -inf),
            (-0.0, 3.0, -0.0),
            (-0.0, PI, 0.0),
            (-0.0, 0.5, 0.0),
            (-0.0, inf, 0.0),
            (0.0, -inf, inf),
            (0.0, -PI, inf),
            (0.0, -3.0, inf),
            (0.0, -0.0, 1.0),
            (0.0, 0.0, 1.0),
            (0.0, 3.0, 0.0),
            (0.0, PI, 0.0),
            (0.0, inf, 0.0),
            (0.0, nan, nan),
            (0.5, -inf, inf),
            (0.5, inf, 0.0),
            (1.0, -inf, 1.0),
            (1.0, inf, 1.0),
            (1.0, nan, 1.0),
            (PI, -inf, 0.0),
            (PI, -0.0, 1.0),
            (PI, 0.0, 1.0),
            (PI, 1.0, PI),
            (PI, inf, inf),
            (PI, nan, nan),
            (inf, -PI, 0.0),
            (inf, -0.0, 1.0),
            (inf, 0.0, 1.0),
            (inf, 1.0, inf),
            (inf, PI, inf),
            (inf, nan, nan),
            (nan, -PI, nan),
            (nan, -0.0, 1.0),
            (nan, 0.0, 1.0),
            (nan, 1.0, nan),
            (nan, PI, nan),
            (nan, nan, nan),
            // Issue #7394 overflow checks.
            (2.0, 4_294_967_296.0, inf),
            (2.0, -4_294_967_296.0, 0.0),
            (-2.0, 4_294_967_297.0, -inf),
            (0.5, 35_184_372_088_832.0, 0.0),
            (0.5, -35_184_372_088_832.0, inf),
            (next_up, 9_223_372_036_854_775_808.0, inf),
            (next_down, 9_223_372_036_854_775_808.0, 0.0),
            (-next_down, 9_223_372_036_854_775_808.0, 0.0),
            (-next_up, 9_223_372_036_854_775_808.0, inf),
            // Issue #57465.
            (-0.0, 1e19, 0.0),
            (-0.0, -1e19, inf),
            (-0.0, 9_007_199_254_740_991.0, -0.0),
            (-0.0, -9_007_199_254_740_991.0, -inf),
        ];
        for fma in [false, true] {
            for (x, bits) in pow10 {
                assert_eq!(
                    pow_with(10.0, x, fma).to_bits(),
                    bits,
                    "10 ** {x}, fma {fma}"
                );
            }
            for (x, y, expected) in special {
                let result = pow_with(x, y, fma);
                assert!(
                    result.to_bits() == expected.to_bits() || result.is_nan() && expected.is_nan(),
                    "{x} ** {y} = {result}, want {expected}"
                );
            }
        }
        // Go's Exp fuses multiply-adds when the CPU has AVX and FMA, and the
        // result can change: 0.35355339059327373 without FMA.
        assert_eq!(pow_with(0.5, 1.5, true).to_bits(), 0x3fd6_a09e_667f_3bcd);
        assert_eq!(pow_with(0.5, 1.5, false).to_bits(), 0x3fd6_a09e_667f_3bcc);
    }

    #[test]
    fn formats_numbers_like_javascript() {
        let cases = [
            (Number::nan(), "NaN"),
            (Number::infinity(1), "Infinity"),
            (Number::infinity(-1), "-Infinity"),
            (Number(-0.0), "0"),
            (Number(std::f64::consts::PI), "3.141592653589793"),
            (Number(1e20), "100000000000000000000"),
            (Number(1e21), "1e+21"),
            (Number::from(f64::from_bits(1)), "5e-324"),
            (
                Number::from(f64::from_bits(0x000f_ffff_ffff_ffff)),
                "2.225073858507201e-308",
            ),
            (Number(-2.109_808_898_695_963e16), "-21098088986959630"),
            (Number(19_686_109_595_169_230_000.0), "19686109595169230000"),
        ];
        for (number, expected) in cases {
            assert_eq!(number.to_string(), expected);
        }
    }

    #[test]
    fn parses_javascript_number_strings() {
        let cases = [
            ("", Number(0.0)),
            ("1.", Number(1.0)),
            (".0000", Number(0.0)),
            ("  +Infinity\n", Number::infinity(1)),
            ("-.0", Number(-0.0)),
            ("0b1010", Number(10.0)),
            ("0o12", Number(10.0)),
            ("0x123456789abcdef0", Number(1_311_768_467_463_790_300.0)),
            ("1e1000", Number::infinity(1)),
            (
                "010000000000000000000",
                Number(10_000_000_000_000_000_000.0),
            ),
            ("\u{00a0}1234.56\u{feff}", Number(1234.56)),
        ];
        for (text, expected) in cases {
            assert_number(Number::from_string(text), expected);
        }
        for invalid in ["NaN", ".", "1e", "++0", "0_0", "0b2", "1.f", "\u{200b}"] {
            assert!(Number::from_string(invalid).is_nan(), "{invalid:?}");
        }
    }

    #[test]
    fn overflowing_radix_literals_convert_to_positive_infinity() {
        for (prefix, digit, count) in [("0b", '1', 1_100), ("0o", '7', 400), ("0x", 'f', 300)] {
            let literal = format!("{prefix}{}", digit.to_string().repeat(count));
            assert_eq!(Number::from_string(&literal), Number::infinity(1));
        }
    }

    #[test]
    fn handles_pseudo_bigints() {
        assert_eq!(parse_pseudo_big_int("00042n"), "42");
        assert_eq!(parse_pseudo_big_int("0b1010_0101n"), "165");
        assert_eq!(parse_pseudo_big_int("0o755n"), "493");
        assert_eq!(
            parse_pseudo_big_int("0x18ee90ff6c373e0ee4e3f0ad2n"),
            "123456789012345678901234567890"
        );
        let negative = PseudoBigInt::parse_valid("-00042n");
        assert_eq!(negative.to_string(), "-42");
        assert_eq!(negative.sign(), -1);
        assert_eq!(PseudoBigInt::new("000", true).sign(), 0);
    }
}
