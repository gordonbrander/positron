//! The unit-interval value type used for velocities and parameter lanes.

/// A value clamped to `0.0..=1.0`.
///
/// Constructors clamp, so a stored `UnitValue` is always valid; `NaN` clamps
/// to `0.0`.
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
pub struct UnitValue(f32);

impl UnitValue {
    /// The minimum value, `0.0`.
    pub const ZERO: Self = Self(0.0);
    /// The maximum value, `1.0`.
    pub const ONE: Self = Self(1.0);

    /// Creates a `UnitValue`, clamping `v` into `0.0..=1.0` (`NaN` becomes `0.0`).
    pub fn new(v: f32) -> Self {
        if v >= 1.0 {
            Self(1.0)
        } else if v >= 0.0 {
            Self(v)
        } else {
            Self(0.0)
        }
    }

    /// Returns the inner value, guaranteed to lie in `0.0..=1.0`.
    pub fn get(self) -> f32 {
        self.0
    }
}

impl Default for UnitValue {
    /// Defaults to `1.0` (full velocity).
    fn default() -> Self {
        Self::ONE
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for UnitValue {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f32(self.0)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for UnitValue {
    /// Deserializes as a plain `f32`, re-clamping so the invariant holds
    /// even for hand-edited or corrupt data.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <f32 as serde::Deserialize>::deserialize(deserializer).map(Self::new)
    }
}

#[cfg(test)]
mod tests {
    use super::UnitValue;

    #[test]
    fn clamps() {
        assert_eq!(UnitValue::new(0.5).get(), 0.5);
        assert_eq!(UnitValue::new(-3.0).get(), 0.0);
        assert_eq!(UnitValue::new(7.0).get(), 1.0);
        assert_eq!(UnitValue::new(f32::NAN).get(), 0.0);
        assert_eq!(UnitValue::new(f32::NEG_INFINITY).get(), 0.0);
        assert_eq!(UnitValue::new(f32::INFINITY).get(), 1.0);
    }
}
