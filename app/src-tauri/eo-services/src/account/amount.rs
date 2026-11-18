//! Money in an account reading, held as integers.
//!
//! The site reports every value as a decimal PED figure truncated to the
//! whole PEC, so a reading's values are whole PEC ([`Pec`]). Figures
//! derived from readings are [`Amount`]s in micro-PED (1 PED is 1,000,000
//! micro-PED), and each carries how it was derived: [`Amount::Exact`] when
//! it comes from a quantity and a unit value that reproduce every
//! truncated value it was read from, [`Amount::WholePec`] when it comes
//! from the truncated values themselves. No figure here is ever a float.
//!
//! Every sum, difference and product is checked: a figure that would leave
//! the range of the integers it is held in is [`AccountError::Overflow`],
//! never a wrapped number. There are deliberately no arithmetic operators
//! on these types.

use serde::{Deserialize, Serialize};

use super::AccountError;

/// Micro-PED in one PEC.
pub const MICRO_PER_PEC: i64 = 10_000;
/// Micro-PED in one PED.
pub const MICRO_PER_PED: i64 = 1_000_000;
/// The largest quantity one row of the site's list may carry.
pub const MAX_QUANTITY: i64 = 1_000_000_000_000;
/// The largest value, in PEC, one row of the list or the PED Card may carry.
pub const MAX_VALUE_PEC: i64 = 1_000_000_000_000;

/// A checked integer result as the module's error.
pub(crate) fn checked(value: Option<i64>) -> Result<i64, AccountError> {
    value.ok_or(AccountError::Overflow)
}

/// The checked sum of some integers.
pub(crate) fn checked_sum(values: impl IntoIterator<Item = i64>) -> Result<i64, AccountError> {
    values
        .into_iter()
        .try_fold(0i64, |total, value| checked(total.checked_add(value)))
}

/// A count as a signed figure.
pub(crate) fn count(n: usize) -> Result<i64, AccountError> {
    i64::try_from(n).map_err(|_| AccountError::Overflow)
}

/// A whole number of PEC (hundredths of a PED), as the site reports values.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Pec(pub i64);

impl Pec {
    pub const ZERO: Pec = Pec(0);

    /// The same figure in micro-PED.
    pub fn micro_ped(self) -> Result<i64, AccountError> {
        checked(self.0.checked_mul(MICRO_PER_PEC))
    }

    pub fn plus(self, other: Pec) -> Result<Pec, AccountError> {
        checked(self.0.checked_add(other.0)).map(Pec)
    }

    pub fn minus(self, other: Pec) -> Result<Pec, AccountError> {
        checked(self.0.checked_sub(other.0)).map(Pec)
    }

    /// The checked sum of some PEC figures.
    pub fn total(values: impl IntoIterator<Item = Pec>) -> Result<Pec, AccountError> {
        checked_sum(values.into_iter().map(|pec| pec.0)).map(Pec)
    }
}

/// How an amount was derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Precision {
    /// From a quantity and a unit value that reproduce every truncated
    /// value it was read from.
    Exact,
    /// From values the site truncated to the whole PEC: each value it was
    /// read from may hide up to one PEC more.
    WholePec,
}

/// A derived amount in micro-PED, signed, with its precision.
///
/// Combining amounts keeps the weaker precision: anything derived in part
/// from a truncated value is [`Amount::WholePec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "precision",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Amount {
    Exact { micro_ped: i64 },
    WholePec { micro_ped: i64 },
}

impl Amount {
    /// Nothing moved, known exactly.
    pub const NONE: Amount = Amount::Exact { micro_ped: 0 };

    pub fn exact(micro_ped: i64) -> Amount {
        Amount::Exact { micro_ped }
    }

    /// A figure of truncated values.
    pub fn whole_pec(pec: Pec) -> Result<Amount, AccountError> {
        Ok(Amount::WholePec {
            micro_ped: pec.micro_ped()?,
        })
    }

    /// `units` at an exact unit value in micro-PED.
    pub fn exact_units(units: i64, unit: i64) -> Result<Amount, AccountError> {
        Ok(Amount::Exact {
            micro_ped: checked(units.checked_mul(unit))?,
        })
    }

    pub fn micro_ped(self) -> i64 {
        match self {
            Amount::Exact { micro_ped } | Amount::WholePec { micro_ped } => micro_ped,
        }
    }

    pub fn precision(self) -> Precision {
        match self {
            Amount::Exact { .. } => Precision::Exact,
            Amount::WholePec { .. } => Precision::WholePec,
        }
    }

    fn with_precision(precision: Precision, micro_ped: i64) -> Amount {
        match precision {
            Precision::Exact => Amount::Exact { micro_ped },
            Precision::WholePec => Amount::WholePec { micro_ped },
        }
    }

    fn weaker(self, other: Amount) -> Precision {
        if self.precision() == Precision::Exact && other.precision() == Precision::Exact {
            Precision::Exact
        } else {
            Precision::WholePec
        }
    }

    pub fn plus(self, other: Amount) -> Result<Amount, AccountError> {
        Ok(Amount::with_precision(
            self.weaker(other),
            checked(self.micro_ped().checked_add(other.micro_ped()))?,
        ))
    }

    pub fn minus(self, other: Amount) -> Result<Amount, AccountError> {
        Ok(Amount::with_precision(
            self.weaker(other),
            checked(self.micro_ped().checked_sub(other.micro_ped()))?,
        ))
    }

    pub fn negated(self) -> Result<Amount, AccountError> {
        Ok(Amount::with_precision(
            self.precision(),
            checked(self.micro_ped().checked_neg())?,
        ))
    }

    /// The same amount, held as no better than whole PEC.
    pub fn at_most_whole_pec(self) -> Amount {
        Amount::WholePec {
            micro_ped: self.micro_ped(),
        }
    }

    /// The checked sum of some amounts; an empty sum is exactly nothing.
    pub fn total(amounts: impl IntoIterator<Item = Amount>) -> Result<Amount, AccountError> {
        amounts
            .into_iter()
            .try_fold(Amount::NONE, |total, amount| total.plus(amount))
    }

    /// Whether two amounts agree within `rows` PEC, the most the site's
    /// truncation of that many rows can hide.
    pub fn agrees_with(self, other: Amount, rows: usize) -> Result<bool, AccountError> {
        let tolerance = checked(count(rows)?.checked_mul(MICRO_PER_PEC))?;
        let gap = checked(self.micro_ped().checked_sub(other.micro_ped()))?;
        Ok(checked(gap.checked_abs())? <= tolerance)
    }
}

/// Why a decimal PED figure from the site is not a whole PEC amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueRefusal {
    /// Below zero.
    Negative,
    /// Finer than a PEC: the site truncates every value to the whole PEC.
    NotWholePec,
    /// Not a plain decimal figure.
    Unreadable,
    /// Above the largest value a row or the Card may carry.
    OutOfRange,
}

impl ValueRefusal {
    pub fn describe(self) -> &'static str {
        match self {
            ValueRefusal::Negative => "is negative",
            ValueRefusal::NotWholePec => "is finer than a whole PEC",
            ValueRefusal::Unreadable => "is not a decimal figure",
            ValueRefusal::OutOfRange => "is beyond the largest value the app reads",
        }
    }
}

/// Read a decimal PED figure as whole PEC from its text, with no float
/// arithmetic.
///
/// The site's JSON numbers have at most two decimals; the number's text is
/// the shortest that reproduces the parsed figure, which for such a figure
/// is its own digits (`96.55`, `25.0`, `0`). A third significant decimal,
/// a sign, or an exponent refuses, as does a figure above
/// [`MAX_VALUE_PEC`].
pub fn pec_from_decimal(number: &serde_json::Number) -> Result<Pec, ValueRefusal> {
    let text = number.to_string();
    if text.starts_with('-') {
        return Err(ValueRefusal::Negative);
    }
    if text.contains(['e', 'E']) {
        return Err(ValueRefusal::Unreadable);
    }
    let (whole, fraction) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let fraction = fraction.trim_end_matches('0');
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || !(fraction.is_empty() || digits(fraction)) {
        return Err(ValueRefusal::Unreadable);
    }
    if fraction.len() > 2 {
        return Err(ValueRefusal::NotWholePec);
    }
    let whole: i64 = whole.parse().map_err(|_| ValueRefusal::OutOfRange)?;
    let hundredths: i64 = match fraction.len() {
        0 => 0,
        1 => {
            fraction
                .parse::<i64>()
                .map_err(|_| ValueRefusal::Unreadable)?
                * 10
        }
        _ => fraction.parse().map_err(|_| ValueRefusal::Unreadable)?,
    };
    whole
        .checked_mul(100)
        .and_then(|pec| pec.checked_add(hundredths))
        .filter(|pec| *pec <= MAX_VALUE_PEC)
        .map(Pec)
        .ok_or(ValueRefusal::OutOfRange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn number(text: &str) -> serde_json::Number {
        serde_json::from_str(text).expect("a JSON number")
    }

    #[test]
    fn the_site_figures_read_as_whole_pec() {
        assert_eq!(pec_from_decimal(&number("96.55")), Ok(Pec(9655)));
        assert_eq!(pec_from_decimal(&number("25.0")), Ok(Pec(2500)));
        assert_eq!(pec_from_decimal(&number("0")), Ok(Pec(0)));
        assert_eq!(pec_from_decimal(&number("0.01")), Ok(Pec(1)));
        assert_eq!(pec_from_decimal(&number("8.4")), Ok(Pec(840)));
        assert_eq!(pec_from_decimal(&number("249.52")), Ok(Pec(24952)));
        assert_eq!(pec_from_decimal(&number("171.65")), Ok(Pec(17165)));
    }

    #[test]
    fn a_figure_finer_than_a_pec_below_zero_or_out_of_range_refuses() {
        assert_eq!(
            pec_from_decimal(&number("0.005")),
            Err(ValueRefusal::NotWholePec)
        );
        assert_eq!(
            pec_from_decimal(&number("96.5587")),
            Err(ValueRefusal::NotWholePec)
        );
        assert_eq!(
            pec_from_decimal(&number("-0.01")),
            Err(ValueRefusal::Negative)
        );
        assert_eq!(pec_from_decimal(&number("-3")), Err(ValueRefusal::Negative));
        assert_eq!(
            pec_from_decimal(&number("1e17")),
            Err(ValueRefusal::Unreadable)
        );
        assert_eq!(
            pec_from_decimal(&number("10000000000.00")),
            Ok(Pec(MAX_VALUE_PEC))
        );
        assert_eq!(
            pec_from_decimal(&number("10000000000.01")),
            Err(ValueRefusal::OutOfRange)
        );
        assert_eq!(
            pec_from_decimal(&number("99999999999999999999")),
            Err(ValueRefusal::Unreadable),
            "a figure the JSON reader holds as a float prints with an exponent"
        );
    }

    #[test]
    fn combining_amounts_keeps_the_weaker_precision() {
        let exact = Amount::exact(1_000);
        let truncated = Amount::whole_pec(Pec(2)).unwrap();
        assert_eq!(exact.plus(exact).unwrap(), Amount::exact(2_000));
        assert_eq!(
            exact.plus(truncated).unwrap(),
            Amount::WholePec { micro_ped: 21_000 }
        );
        assert_eq!(
            truncated.minus(exact).unwrap(),
            Amount::WholePec { micro_ped: 19_000 }
        );
        assert_eq!(exact.negated().unwrap(), Amount::exact(-1_000));
        assert_eq!(Amount::total([exact, exact]).unwrap(), Amount::exact(2_000));
        assert_eq!(Amount::total([]).unwrap(), Amount::NONE);
        assert_eq!(
            Amount::total([exact, truncated]).unwrap().precision(),
            Precision::WholePec
        );
        assert_eq!(
            exact.at_most_whole_pec(),
            Amount::WholePec { micro_ped: 1_000 }
        );
    }

    #[test]
    fn arithmetic_that_would_leave_the_range_is_an_error_never_a_wrapped_figure() {
        let huge = Amount::exact(i64::MAX);
        assert!(matches!(
            huge.plus(Amount::exact(1)),
            Err(AccountError::Overflow)
        ));
        assert!(matches!(
            Amount::exact(i64::MIN).negated(),
            Err(AccountError::Overflow)
        ));
        assert!(matches!(
            Amount::exact(i64::MIN).minus(Amount::exact(1)),
            Err(AccountError::Overflow)
        ));
        assert!(matches!(
            Amount::exact_units(MAX_QUANTITY, MAX_VALUE_PEC),
            Err(AccountError::Overflow)
        ));
        assert!(matches!(
            Pec(i64::MAX).micro_ped(),
            Err(AccountError::Overflow)
        ));
        assert!(matches!(
            Pec::total([Pec(i64::MAX), Pec(1)]),
            Err(AccountError::Overflow)
        ));
        assert!(matches!(
            checked_sum([i64::MAX, 1]),
            Err(AccountError::Overflow)
        ));
    }

    #[test]
    fn amounts_agree_within_a_pec_per_row() {
        let a = Amount::exact(60_000_000);
        let b = Amount::exact(59_950_000);
        assert!(a.agrees_with(b, 5).unwrap());
        assert!(!a.agrees_with(b, 4).unwrap());
        assert!(b.agrees_with(a, 5).unwrap());
    }

    #[test]
    fn an_amount_serialises_with_its_precision() {
        assert_eq!(
            serde_json::to_value(Amount::exact(660_288)).unwrap(),
            serde_json::json!({"precision": "exact", "microPed": 660_288})
        );
        assert_eq!(
            serde_json::to_value(Amount::whole_pec(Pec(3)).unwrap()).unwrap(),
            serde_json::json!({"precision": "wholePec", "microPed": 30_000})
        );
        let back: Amount =
            serde_json::from_value(serde_json::json!({"precision": "wholePec", "microPed": 7}))
                .unwrap();
        assert_eq!(back, Amount::WholePec { micro_ped: 7 });
    }

    proptest! {
        /// Every whole-PEC figure up to the largest the app reads, written as
        /// the site writes it and parsed as JSON, reads back as exactly its
        /// PEC.
        #[test]
        fn every_written_figure_reads_back_exactly(pec in 0i64..=MAX_VALUE_PEC) {
            let two_decimals = format!("{}.{:02}", pec / 100, pec % 100);
            prop_assert_eq!(pec_from_decimal(&number(&two_decimals)), Ok(Pec(pec)));
            if pec % 10 == 0 {
                let one_decimal = format!("{}.{}", pec / 100, (pec % 100) / 10);
                prop_assert_eq!(pec_from_decimal(&number(&one_decimal)), Ok(Pec(pec)));
            }
            if pec % 100 == 0 {
                let whole = format!("{}", pec / 100);
                prop_assert_eq!(pec_from_decimal(&number(&whole)), Ok(Pec(pec)));
            }
        }
    }
}
