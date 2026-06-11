//! A single sequencer step.

use crate::{Condition, Retrig};

/// Re-clamps micro-timing on deserialization so the invariant holds for
/// hand-edited or corrupt data.
#[cfg(feature = "serde")]
fn de_micro<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i8, D::Error> {
    <i8 as serde::Deserialize>::deserialize(deserializer)
        .map(|v| v.clamp(-Step::MAX_MICRO, Step::MAX_MICRO))
}

/// One step of a track: a trig slot.
///
/// Parameter locks ("p-locks") are not stored here — they live in the
/// pattern-level lock pool, edited through the
/// [`Pattern`](crate::Pattern) lock API.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Step {
    /// Is there a trig on this step at all?
    pub trig: bool,
    /// When the trig fires. `Default` = [`Condition::Always`].
    pub condition: Condition,
    /// Micro-timing nudge in pulses, `-23..=23`. Guarded by `set_micro`.
    #[cfg_attr(feature = "serde", serde(deserialize_with = "de_micro"))]
    micro: i8,
    /// Retrig: when this trig fires it starts a train of rapid repeats.
    /// `None` = a single hit.
    pub retrig: Option<Retrig>,
}

impl Step {
    /// The largest micro-timing nudge: one pulse short of a full step.
    pub const MAX_MICRO: i8 = 23;

    /// This step's micro-timing nudge in pulses. Negative plays early (the
    /// event lands near the end of the previous step window), positive late.
    pub fn micro(&self) -> i8 {
        self.micro
    }

    /// Sets the micro-timing nudge, clamping to `-23..=23`.
    pub fn set_micro(&mut self, pulses: i8) {
        self.micro = pulses.clamp(-Self::MAX_MICRO, Self::MAX_MICRO);
    }
}
