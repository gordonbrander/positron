//! Patterns: the pure-data layer of the sequencer.

use crate::{NUM_TRACKS, Track};
use std::num::NonZeroU16;

/// Identifies a pattern within a [`crate::Sequencer`]: an index into its
/// pattern list, as returned by `add_pattern`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PatternId(pub u8);

/// Re-clamps swing on deserialization so the invariant holds for
/// hand-edited or corrupt data.
#[cfg(feature = "serde")]
fn de_swing<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    <u8 as serde::Deserialize>::deserialize(deserializer).map(|v| v.clamp(50, 80))
}

/// What to play: 16 tracks of steps.
///
/// A `Pattern` is plain data — it contains nothing about playback position,
/// so hosts can hold, diff, copy, and persist patterns without touching the
/// running sequencer.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Pattern {
    /// The 16 tracks.
    pub tracks: [Track; NUM_TRACKS],
    /// Swing percent, `50..=80`. Guarded by `set_swing`.
    #[cfg_attr(feature = "serde", serde(deserialize_with = "de_swing"))]
    swing: u8,
    /// Pattern-change quantization in steps: queued pattern changes apply
    /// when the master step counter reaches a multiple of this. `None` =
    /// the pattern's master length (its longest track length).
    pub change_length: Option<NonZeroU16>,
}

impl Default for Pattern {
    fn default() -> Self {
        Self {
            tracks: std::array::from_fn(|_| Track::default()),
            swing: 50,
            change_length: None,
        }
    }
}

impl Pattern {
    /// Swing amount in percent, `50..=80`. 50 = straight.
    pub fn swing(&self) -> u8 {
        self.swing
    }

    /// Sets the swing amount, clamping to `50..=80`.
    ///
    /// Swing delays the odd-indexed steps of every track (indexes 1, 3, 5, …
    /// within each track — under polymeter, parity is per-track by
    /// definition). The delay in pulses is `(48 × swing + 50) / 100 − 24`:
    /// 0 at 50%, 8 at 66% (triplet feel), 12 at 75%, 14 at 80%.
    pub fn set_swing(&mut self, percent: u8) {
        self.swing = percent.clamp(50, 80);
    }

    /// The swing delay in pulses applied to odd-indexed steps.
    pub(crate) fn swing_delay(&self) -> i16 {
        (48 * i16::from(self.swing) + 50) / 100 - 24
    }

    /// The pattern's master length in steps: its longest track length.
    /// This is the default pattern-change quantization (see
    /// [`change_length`](Self::change_length)).
    pub fn master_length(&self) -> u16 {
        u16::from(self.tracks.iter().map(Track::length).max().unwrap_or(1))
    }

    /// The effective change quantization: `change_length` if set, else the
    /// master length.
    pub(crate) fn effective_change_length(&self) -> u32 {
        self.change_length
            .map_or_else(|| u32::from(self.master_length()), |c| u32::from(c.get()))
    }
}
