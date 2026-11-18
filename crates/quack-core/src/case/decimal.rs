//! Exact decimal numbers for `number` context answers (§4.6).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::limits::{CONTEXT_NUMBER_MAX_ABS, CONTEXT_NUMBER_MAX_DECIMALS};

/// Millionths in one: the scale of [`Decimal`].
const SCALE: i128 = 1_000_000;

/// A `number` answer, held exactly as a count of millionths and never as a
/// binary float, so `0.1` stays `0.1`.
///
/// Its absolute value is at most 10^15 and it has at most 6 decimal places
/// (§3.9). It is stored and sent on the API as its canonical decimal
/// string: no exponent, no leading `+`, no trailing fractional zeros
/// (`"2.5"`, `"-3"`, `"0.000001"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Decimal(i128);

/// Why text is not an acceptable [`Decimal`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DecimalError {
    /// Not optional `-`, digits, and optional `.` with digits.
    #[error("not a number")]
    NotANumber,
    /// Larger than 10^15 in absolute value, or more than 6 decimal places.
    #[error("number out of range")]
    OutOfRange,
}

impl Decimal {
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// The value as a count of millionths, e.g. `2.5` is `2_500_000`.
    pub const fn millionths(self) -> i128 {
        self.0
    }

    /// Builds a decimal from millionths; `None` outside ±10^15.
    pub const fn from_millionths(millionths: i128) -> Option<Self> {
        if millionths.unsigned_abs() <= CONTEXT_NUMBER_MAX_ABS.unsigned_abs() * SCALE.unsigned_abs()
        {
            Some(Self(millionths))
        } else {
            None
        }
    }

    /// Parses the §4.6 grammar from the original text: optional `-`,
    /// digits, optional `.` followed by digits. No exponent, no `+`, no
    /// spaces, no bare `.5` or `5.`. Trailing fractional zeros don't count
    /// as decimal places (`2.5000000` is `2.5`).
    pub fn parse(text: &str) -> Result<Self, DecimalError> {
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (int, frac) = match digits.split_once('.') {
            Some((int, frac)) => (int, Some(frac)),
            None => (digits, None),
        };
        let all_digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        if !all_digits(int) || frac.is_some_and(|f| !all_digits(f)) {
            return Err(DecimalError::NotANumber);
        }
        let frac = frac.unwrap_or_default().trim_end_matches('0');
        if frac.len() > CONTEXT_NUMBER_MAX_DECIMALS as usize {
            return Err(DecimalError::OutOfRange);
        }
        let mut millionths: i128 = 0;
        for b in int.bytes() {
            millionths = millionths
                .checked_mul(10)
                .and_then(|v| v.checked_add(i128::from(b - b'0')))
                .filter(|v| *v <= CONTEXT_NUMBER_MAX_ABS)
                .ok_or(DecimalError::OutOfRange)?;
        }
        millionths *= SCALE;
        let mut place = SCALE / 10;
        for b in frac.bytes() {
            millionths += i128::from(b - b'0') * place;
            place /= 10;
        }
        if negative {
            millionths = -millionths;
        }
        Self::from_millionths(millionths).ok_or(DecimalError::OutOfRange)
    }
}

impl FromStr for Decimal {
    type Err = DecimalError;

    fn from_str(s: &str) -> Result<Self, DecimalError> {
        Self::parse(s)
    }
}

impl fmt::Display for Decimal {
    /// Writes the canonical decimal string.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let abs = self.0.unsigned_abs();
        let scale = SCALE.unsigned_abs();
        let (int, frac) = (abs / scale, abs % scale);
        if self.0 < 0 {
            f.write_str("-")?;
        }
        write!(f, "{int}")?;
        if frac != 0 {
            let digits = format!("{frac:06}");
            write!(f, ".{}", digits.trim_end_matches('0'))?;
        }
        Ok(())
    }
}

impl Serialize for Decimal {
    /// Serializes as the canonical decimal string.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Decimal {
    /// Reads a canonical (or any grammatical) decimal string, or a JSON
    /// integer. Binary floats are refused: they may already have lost the
    /// value.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = Decimal;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a decimal string or an integer")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Decimal, E> {
                Decimal::parse(v).map_err(E::custom)
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Decimal, E> {
                Decimal::from_millionths(i128::from(v) * SCALE)
                    .ok_or_else(|| E::custom(DecimalError::OutOfRange))
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Decimal, E> {
                Decimal::from_millionths(i128::from(v) * SCALE)
                    .ok_or_else(|| E::custom(DecimalError::OutOfRange))
            }
        }

        d.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn canon(s: &str) -> Result<String, DecimalError> {
        Decimal::parse(s).map(|d| d.to_string())
    }

    #[test]
    fn grammar() {
        for (input, out) in [
            ("0", "0"),
            ("-0", "0"),
            ("2.50", "2.5"),
            ("007", "7"),
            ("-3", "-3"),
            ("0.1", "0.1"),
            ("0.000001", "0.000001"),
            ("2.5000000", "2.5"),
            ("1000000000000000", "1000000000000000"),
            ("-1000000000000000.000000", "-1000000000000000"),
            ("999999999999999.999999", "999999999999999.999999"),
        ] {
            assert_eq!(canon(input), Ok(out.to_owned()), "{input}");
        }
        for bad in [
            "", "-", ".5", "5.", "+5", "1e3", "1E3", " 1", "1 ", "1 000", "1,5", "0x10", "--1",
            "1.2.3", "NaN", "inf", "١",
        ] {
            assert_eq!(canon(bad), Err(DecimalError::NotANumber), "{bad:?}");
        }
        for big in [
            "1000000000000000.000001",
            "1000000000000001",
            "-1000000000000001",
            "0.0000001",
            "1.1234567",
            "99999999999999999999999999999999999999999999",
        ] {
            assert_eq!(canon(big), Err(DecimalError::OutOfRange), "{big}");
        }
    }

    #[test]
    fn exact_where_floats_are_not() {
        let d = Decimal::parse("123456789012345.123456").unwrap();
        assert_eq!(d.millionths(), 123_456_789_012_345_123_456);
        assert_eq!(d.to_string(), "123456789012345.123456");
        assert_eq!(
            Decimal::parse("0.3").unwrap().millionths(),
            Decimal::parse("0.1").unwrap().millionths() * 3
        );
    }

    #[test]
    fn serde_uses_the_canonical_string() {
        let d = Decimal::parse("-2.50").unwrap();
        assert_eq!(serde_json::to_value(d).unwrap(), serde_json::json!("-2.5"));
        assert_eq!(
            serde_json::from_value::<Decimal>(serde_json::json!("-2.5")).unwrap(),
            d
        );
        assert_eq!(
            serde_json::from_value::<Decimal>(serde_json::json!(42)).unwrap(),
            Decimal::parse("42").unwrap()
        );
        assert!(serde_json::from_value::<Decimal>(serde_json::json!(0.5)).is_err());
        assert!(serde_json::from_value::<Decimal>(serde_json::json!("1e3")).is_err());
        assert_eq!(Decimal::from_millionths(10_i128.pow(21) + 1), None);
    }

    proptest! {
        #[test]
        fn canonical_strings_round_trip(m in -(10_i128.pow(21))..=10_i128.pow(21)) {
            let d = Decimal::from_millionths(m).unwrap();
            let text = d.to_string();
            prop_assert_eq!(Decimal::parse(&text), Ok(d));
            prop_assert!(!text.contains('e') && !text.starts_with('+'));
            prop_assert!(!(text.contains('.') && text.ends_with('0')));
        }
    }
}
