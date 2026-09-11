//! Integer money, quantity and rate types.
//!
//! No floating point appears anywhere in the money path. An auditor re-adds these
//! numbers by hand, so every operation is exact, checked, and rounds by a rule
//! that is written down rather than inherited from a hardware default.

use core::fmt;

/// Money in minor units. Poisha for BDT, cents for USD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Minor(i64);

/// A quantity in thousandths of a unit, so 1.5 kg is `Milli(1_500)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Milli(i64);

/// A rate in basis points, so 15 percent VAT is `Bp(1_500)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Bp(u32);

/// One hundred percent, the largest meaningful discount rate.
pub const BP_ONE: u32 = 10_000;

const MILLI_PER_UNIT: i128 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    /// A result did not fit in the underlying integer.
    Overflow,
    /// A rate outside 0 to 100 percent.
    RateOutOfRange { bp: u32 },
    /// A negative amount where only non-negative makes sense.
    Negative,
    /// Tendered less than the amount due.
    Underpaid { short_by: Minor },
}

impl fmt::Display for MoneyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => write!(f, "arithmetic overflow"),
            Self::RateOutOfRange { bp } => write!(f, "rate {bp} basis points is outside 0 to 10000"),
            Self::Negative => write!(f, "negative amount is not allowed here"),
            Self::Underpaid { short_by } => write!(f, "underpaid by {} minor units", short_by.get()),
        }
    }
}

impl core::error::Error for MoneyError {}

pub type Result<T> = core::result::Result<T, MoneyError>;

impl Minor {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn new(units: i64) -> Self {
        Self(units)
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub fn checked_add(self, other: Self) -> Result<Self> {
        self.0.checked_add(other.0).map(Self).ok_or(MoneyError::Overflow)
    }

    pub fn checked_sub(self, other: Self) -> Result<Self> {
        self.0.checked_sub(other.0).map(Self).ok_or(MoneyError::Overflow)
    }

    /// Arithmetic negation, failing at `i64::MIN` rather than wrapping.
    pub fn checked_neg(self) -> Result<Self> {
        self.0.checked_neg().map(Self).ok_or(MoneyError::Overflow)
    }

    /// Magnitude, failing at `i64::MIN` rather than wrapping.
    pub fn checked_abs(self) -> Result<Self> {
        self.0.checked_abs().map(Self).ok_or(MoneyError::Overflow)
    }

    /// Sum, failing on overflow rather than wrapping.
    pub fn sum<I: IntoIterator<Item = Self>>(items: I) -> Result<Self> {
        items.into_iter().try_fold(Self::ZERO, Self::checked_add)
    }

    /// Multiply by a quantity in milli-units, rounding half away from zero.
    ///
    /// A price of 4.30 for 1.5 kg is `430 * 1500 / 1000 = 645`.
    pub fn mul_qty(self, qty: Milli) -> Result<Self> {
        let product = i128::from(self.0)
            .checked_mul(i128::from(qty.get()))
            .ok_or(MoneyError::Overflow)?;
        to_minor(div_round_half_away(product, MILLI_PER_UNIT)?)
    }

    /// Apply a rate, rounding half away from zero. 15 percent of 100 is 15.
    pub fn apply_rate(self, rate: Bp) -> Result<Self> {
        let product = i128::from(self.0)
            .checked_mul(i128::from(rate.get()))
            .ok_or(MoneyError::Overflow)?;
        to_minor(div_round_half_away(product, i128::from(BP_ONE))?)
    }

    /// The part of this amount that belongs to `part` of `whole`.
    ///
    /// Half a line coming back brings half of what came off it. Rounded the way
    /// every other money split here is, half away from zero, so a whole line
    /// returned in two halves and a whole line returned at once can differ by at
    /// most the poisha the rounding decides, and never by the direction of it.
    ///
    /// `whole` of zero is not a division anybody meant: nothing was on the line,
    /// so nothing comes off it.
    pub fn share_of(self, part: Milli, whole: Milli) -> Result<Self> {
        if whole.get() == 0 {
            return Ok(Self::ZERO);
        }
        let product = i128::from(self.0)
            .checked_mul(i128::from(part.get()))
            .ok_or(MoneyError::Overflow)?;
        to_minor(div_round_half_away(product, i128::from(whole.get()))?)
    }

    /// Split a VAT-inclusive amount into its net part.
    ///
    /// `net = gross * 10000 / (10000 + rate)`. The VAT is then `gross - net`, which
    /// keeps net plus VAT exactly equal to the price on the shelf label.
    pub fn net_of_inclusive(self, rate: Bp) -> Result<Self> {
        let denominator = i128::from(BP_ONE)
            .checked_add(i128::from(rate.get()))
            .ok_or(MoneyError::Overflow)?;
        let numerator = i128::from(self.0)
            .checked_mul(i128::from(BP_ONE))
            .ok_or(MoneyError::Overflow)?;
        to_minor(div_round_half_away(numerator, denominator)?)
    }
}

impl Milli {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1_000);

    #[must_use]
    pub const fn new(thousandths: i64) -> Self {
        Self(thousandths)
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    pub fn checked_add(self, other: Self) -> Result<Self> {
        self.0.checked_add(other.0).map(Self).ok_or(MoneyError::Overflow)
    }
}

impl Bp {
    pub const ZERO: Self = Self(0);

    /// Reject anything outside 0 to 100 percent at construction, so downstream
    /// code never has to wonder.
    pub fn new(basis_points: u32) -> Result<Self> {
        if basis_points > BP_ONE {
            return Err(MoneyError::RateOutOfRange { bp: basis_points });
        }
        Ok(Self(basis_points))
    }

    /// A VAT rate, which may exceed 100 percent only in absurd jurisdictions and
    /// therefore uses the same bound.
    pub fn vat(basis_points: u32) -> Result<Self> {
        Self::new(basis_points)
    }

    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }
}

/// Divide, rounding halves away from zero.
///
/// Commercial rounding, and symmetric about zero so a refund is the exact
/// negative of the sale it reverses. Banker's rounding is deliberately not used:
/// it surprises shopkeepers who check the arithmetic on paper.
pub(crate) fn div_round_half_away(numerator: i128, denominator: i128) -> Result<i128> {
    if denominator == 0 {
        return Err(MoneyError::Overflow);
    }
    let negative = (numerator < 0) != (denominator < 0);
    let numerator_abs = numerator.checked_abs().ok_or(MoneyError::Overflow)?;
    let denominator_abs = denominator.checked_abs().ok_or(MoneyError::Overflow)?;

    let doubled = numerator_abs.checked_mul(2).ok_or(MoneyError::Overflow)?;
    let adjusted = doubled.checked_add(denominator_abs).ok_or(MoneyError::Overflow)?;
    let twice_denominator = denominator_abs.checked_mul(2).ok_or(MoneyError::Overflow)?;
    let magnitude = adjusted
        .checked_div(twice_denominator)
        .ok_or(MoneyError::Overflow)?;

    if negative {
        magnitude.checked_neg().ok_or(MoneyError::Overflow)
    } else {
        Ok(magnitude)
    }
}

pub(crate) fn to_minor(value: i128) -> Result<Minor> {
    i64::try_from(value).map(Minor).map_err(|_| MoneyError::Overflow)
}

#[cfg(test)]
mod tests {
    // Tests assert with plain arithmetic and panic on failure, which is the point
    // of them. The workspace bans both in production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::arithmetic_side_effects)]

    use super::*;

    #[test]
    fn rounds_halves_away_from_zero() {
        assert_eq!(div_round_half_away(5, 2), Ok(3));
        assert_eq!(div_round_half_away(-5, 2), Ok(-3));
        assert_eq!(div_round_half_away(7, 2), Ok(4));
        assert_eq!(div_round_half_away(4, 2), Ok(2));
        assert_eq!(div_round_half_away(1, 3), Ok(0));
        assert_eq!(div_round_half_away(2, 3), Ok(1));
    }

    #[test]
    fn multiplies_price_by_a_fractional_quantity() {
        // 4.30 per kg for 1.5 kg
        let price = Minor::new(430);
        assert_eq!(price.mul_qty(Milli::new(1_500)), Ok(Minor::new(645)));
        // a third of a kilo rounds to the nearest poisha
        assert_eq!(price.mul_qty(Milli::new(333)), Ok(Minor::new(143)));
    }

    #[test]
    fn applies_a_vat_rate() {
        assert_eq!(Minor::new(10_000).apply_rate(Bp::new(1_500).unwrap()), Ok(Minor::new(1_500)));
        // rounds the half up rather than truncating it away
        assert_eq!(Minor::new(10).apply_rate(Bp::new(1_500).unwrap()), Ok(Minor::new(2)));
    }

    #[test]
    fn splits_a_vat_inclusive_price() {
        let gross = Minor::new(11_500);
        let rate = Bp::new(1_500).unwrap();
        let net = gross.net_of_inclusive(rate).unwrap();
        assert_eq!(net, Minor::new(10_000));
        assert_eq!(gross.checked_sub(net), Ok(Minor::new(1_500)));
    }

    #[test]
    fn rejects_a_rate_above_one_hundred_percent() {
        assert_eq!(Bp::new(10_001), Err(MoneyError::RateOutOfRange { bp: 10_001 }));
    }

    #[test]
    fn shares_an_amount_by_how_much_of_the_line_is_coming_back() {
        let came_off = Minor::new(1_000);
        // Half a line brings half of what came off it.
        assert_eq!(came_off.share_of(Milli::new(500), Milli::ONE), Ok(Minor::new(500)));
        // Two of four brings half, and one of three brings a third rounded the
        // way the rest of the money here rounds: 333.33 is 333.
        assert_eq!(came_off.share_of(Milli::new(2_000), Milli::new(4_000)), Ok(Minor::new(500)));
        assert_eq!(came_off.share_of(Milli::new(1_000), Milli::new(3_000)), Ok(Minor::new(333)));
        // Halves away from zero, not to the even number: 5 shared one of two is
        // 3, which is what a shopkeeper checking it on paper gets.
        assert_eq!(Minor::new(5).share_of(Milli::new(1_000), Milli::new(2_000)), Ok(Minor::new(3)));
        // All of it, and none of it.
        assert_eq!(came_off.share_of(Milli::ONE, Milli::ONE), Ok(came_off));
        assert_eq!(came_off.share_of(Milli::ZERO, Milli::ONE), Ok(Minor::ZERO));
        // Nothing was on the line, so nothing comes off it. Not a division.
        assert_eq!(came_off.share_of(Milli::ONE, Milli::ZERO), Ok(Minor::ZERO));
    }

    #[test]
    fn reports_overflow_rather_than_wrapping() {
        assert_eq!(Minor::new(i64::MAX).checked_add(Minor::new(1)), Err(MoneyError::Overflow));
        assert_eq!(Minor::new(i64::MAX).mul_qty(Milli::new(2_000)), Err(MoneyError::Overflow));
    }
}
