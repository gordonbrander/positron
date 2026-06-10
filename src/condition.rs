//! Trig conditions: when an enabled trig actually fires.

use crate::UnitValue;

/// Error from [`Condition::ratio`] for out-of-range arguments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RatioError {
    /// The rejected numerator.
    pub a: u8,
    /// The rejected denominator.
    pub b: u8,
}

impl std::fmt::Display for RatioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ratio condition requires 1 <= a <= b <= 8, got {}:{}",
            self.a, self.b
        )
    }
}

impl std::error::Error for RatioError {}

/// When a trig fires.
///
/// Probability and logical conditions share one parameter slot per step, as
/// on Elektron hardware — they are mutually exclusive, which is why this is a
/// single enum rather than separate fields.
///
/// "Updates state" below refers to the per-track memory that `Pre`/`Nei`
/// read: the result of the most recent *state-writing* conditional evaluated
/// on a track. `Always`, `Pre`, and `Nei` themselves are transparent — they
/// never write it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum Condition {
    /// No condition: the trig always fires. Transparent to `Pre`/`Nei` state.
    #[default]
    Always,
    /// Fires with the given probability; updates state. A random draw happens
    /// only when an enabled trig with this condition is evaluated, so
    /// unrelated edits don't shift other tracks' random sequences.
    Percent(UnitValue),
    /// Fires iff fill mode is active; updates state.
    Fill,
    /// Fires iff fill mode is inactive; updates state.
    NotFill,
    /// Fires iff the most recent state-writing conditional on the same track
    /// passed. Transparent: chains of `Pre` all follow the same upstream
    /// condition.
    Pre,
    /// Complement of [`Pre`](Self::Pre). Transparent.
    NotPre,
    /// Like [`Pre`](Self::Pre) but reads the neighbor track (track N−1).
    /// On track 0 it never fires. Transparent.
    Nei,
    /// Complement of [`Nei`](Self::Nei); passes on track 0. Transparent.
    NotNei,
    /// Fires only on the track's first loop after transport start; updates
    /// state.
    First,
    /// Fires on every loop except the first; updates state.
    NotFirst,
    /// Fires on loop `a` of every `b`-loop cycle (1-based); updates state.
    /// E.g. `1:4` fires on loops 0, 4, 8, … (0-indexed).
    ///
    /// Fields are public for ergonomic literals; evaluation defensively
    /// clamps `b` to `1..=8` and `a` to `1..=b`, so no value can panic. Use
    /// [`Condition::ratio`] to construct validated values.
    Ratio {
        /// Which loop of the cycle fires, `1..=b`.
        a: u8,
        /// Cycle length in loops, `1..=8`.
        b: u8,
    },
}

impl Condition {
    /// Validated constructor for [`Condition::Ratio`].
    ///
    /// # Errors
    /// Returns [`RatioError`] unless `1 <= a <= b <= 8`.
    pub fn ratio(a: u8, b: u8) -> Result<Self, RatioError> {
        if a >= 1 && a <= b && b <= 8 {
            Ok(Self::Ratio { a, b })
        } else {
            Err(RatioError { a, b })
        }
    }
}
