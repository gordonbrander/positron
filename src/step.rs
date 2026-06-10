//! A single sequencer step.

use crate::{Condition, NUM_PARAM_LANES, Params, Retrig, UnitValue};

/// Re-clamps micro-timing on deserialization so the invariant holds for
/// hand-edited or corrupt data.
#[cfg(feature = "serde")]
fn de_micro<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i8, D::Error> {
    <i8 as serde::Deserialize>::deserialize(deserializer)
        .map(|v| v.clamp(-Step::MAX_MICRO, Step::MAX_MICRO))
}

/// Per-step parameter overrides — Elektron "p-locks".
///
/// `None` means "use the track default". Locks are pattern data: they survive
/// transport changes and only ever apply when their trig actually fires.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParamLocks {
    /// Velocity override for this step.
    pub velocity: Option<UnitValue>,
    /// Per-lane overrides for this step.
    pub lanes: [Option<UnitValue>; NUM_PARAM_LANES],
}

/// One step of a track: a trig slot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Step {
    /// Is there a trig on this step at all?
    pub trig: bool,
    /// Per-step parameter overrides. `Default` = no locks.
    pub locks: ParamLocks,
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
    /// Removes every p-lock from this step (the trig itself stays).
    pub fn clear_locks(&mut self) {
        self.locks = ParamLocks::default();
    }

    /// What this step would play against the given track defaults: per
    /// parameter, the lock if set, else the default. Pure data — useful for
    /// UI display as well as playback.
    pub fn resolve(&self, defaults: &Params) -> (f32, [f32; NUM_PARAM_LANES]) {
        let velocity = self.locks.velocity.unwrap_or(defaults.velocity).get();
        let lanes = std::array::from_fn(|i| self.locks.lanes[i].unwrap_or(defaults.lanes[i]).get());
        (velocity, lanes)
    }
}
