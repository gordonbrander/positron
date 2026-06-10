//! Retrigs: rapid repeats of a fired trig.

use std::num::NonZeroU16;

/// Hit rate of a retrig train, as a note value.
///
/// Only rates representable as a whole number of pulses on the 96 PPQN grid
/// exist (the hardware's 1/80 is not; the nearest available is `R96`):
///
/// | Rate  | Note | Pulses |
/// |-------|------|--------|
/// | `R4`  | 1/4  | 96     |
/// | `R6`  | 1/6  | 64     |
/// | `R8`  | 1/8  | 48     |
/// | `R12` | 1/12 | 32     |
/// | `R16` | 1/16 | 24     |
/// | `R24` | 1/24 | 16     |
/// | `R32` | 1/32 | 12     |
/// | `R48` | 1/48 | 8      |
/// | `R64` | 1/64 | 6      |
/// | `R96` | 1/96 | 4      |
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RetrigRate {
    /// Quarter notes (96 pulses).
    R4,
    /// Quarter-note triplets (64 pulses).
    R6,
    /// Eighth notes (48 pulses).
    R8,
    /// Eighth-note triplets (32 pulses).
    R12,
    /// Sixteenth notes — one hit per step (24 pulses).
    #[default]
    R16,
    /// Sixteenth-note triplets (16 pulses).
    R24,
    /// Thirty-second notes (12 pulses).
    R32,
    /// Thirty-second-note triplets (8 pulses).
    R48,
    /// Sixty-fourth notes (6 pulses).
    R64,
    /// The fastest rate (4 pulses).
    R96,
}

impl RetrigRate {
    /// Pulses between consecutive hits.
    pub fn interval(self) -> u16 {
        match self {
            Self::R4 => 96,
            Self::R6 => 64,
            Self::R8 => 48,
            Self::R12 => 32,
            Self::R16 => 24,
            Self::R24 => 16,
            Self::R32 => 12,
            Self::R48 => 8,
            Self::R64 => 6,
            Self::R96 => 4,
        }
    }
}

/// How long a retrig train runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RetrigLength {
    /// The train covers this many pulses from the trig: hits land at
    /// `0, interval, 2×interval, …` up to and including this length.
    Pulses(NonZeroU16),
    /// The train runs until the next fired trig on the track replaces it.
    Infinite,
}

impl RetrigLength {
    /// Convenience constructor; `n` is clamped up to at least 1 pulse.
    pub fn pulses(n: u16) -> Self {
        Self::Pulses(NonZeroU16::new(n).unwrap_or(NonZeroU16::MIN))
    }
}

/// Per-step retrig settings: when the step's trig fires, it becomes the
/// first hit of a train.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Retrig {
    /// Interval between hits.
    pub rate: RetrigRate,
    /// How long the train runs.
    pub length: RetrigLength,
    /// Velocity ramp across a finite train, `-1.0..=1.0`. Guarded by
    /// `set_vel_ramp`.
    #[cfg_attr(feature = "serde", serde(deserialize_with = "de_ramp"))]
    vel_ramp: f32,
}

/// Re-clamps the ramp on deserialization so the invariant holds for
/// hand-edited or corrupt data.
#[cfg(feature = "serde")]
fn de_ramp<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<f32, D::Error> {
    <f32 as serde::Deserialize>::deserialize(deserializer)
        .map(|r| if r.is_nan() { 0.0 } else { r.clamp(-1.0, 1.0) })
}

impl Retrig {
    /// Creates retrig settings; `vel_ramp` is clamped to `-1.0..=1.0`.
    pub fn new(rate: RetrigRate, length: RetrigLength, vel_ramp: f32) -> Self {
        let mut r = Self {
            rate,
            length,
            vel_ramp: 0.0,
        };
        r.set_vel_ramp(vel_ramp);
        r
    }

    /// Velocity ramp across the train: the last hit of an `n`-hit train
    /// plays at `clamp(v0 + vel_ramp, 0, 1)`, hits in between interpolate
    /// linearly. `0.0` = flat; ignored by `Infinite` and single-hit trains.
    pub fn vel_ramp(&self) -> f32 {
        self.vel_ramp
    }

    /// Sets the velocity ramp, clamping to `-1.0..=1.0` (`NaN` becomes 0).
    pub fn set_vel_ramp(&mut self, ramp: f32) {
        self.vel_ramp = if ramp.is_nan() {
            0.0
        } else {
            ramp.clamp(-1.0, 1.0)
        };
    }
}

impl Default for Retrig {
    /// Sixteenth-note hits, one step long, flat velocity.
    fn default() -> Self {
        Self::new(RetrigRate::R16, RetrigLength::pulses(24), 0.0)
    }
}
