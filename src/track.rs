//! Tracks and their default parameters.

use crate::{MAX_PAGES, MAX_STEPS, NUM_PARAM_LANES, STEPS_PER_PAGE, Step, UnitValue};

/// Error returned when a track length (or page count) is out of range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LengthError {
    /// The rejected length, in steps.
    pub steps: u16,
}

impl std::fmt::Display for LengthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "track length must be 1..={MAX_STEPS} steps, got {}",
            self.steps
        )
    }
}

impl std::error::Error for LengthError {}

/// Track-level default parameters.
///
/// Velocity is distinguished — it gates output and drives the retrig
/// velocity ramp. The lanes are generic, unlabeled `0..1` values: the engine
/// never knows what they control; the host owns the mapping (FM operators,
/// ADSR times, macros, …) and any scaling from `0..1` to real units. Adding a
/// parameter to your instrument = picking a lane index.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Params {
    /// Default velocity for trigs on this track. Defaults to `1.0`.
    pub velocity: UnitValue,
    /// Default values for the generic parameter lanes. Default to `0.0`.
    pub lanes: [UnitValue; NUM_PARAM_LANES],
}

impl Default for Params {
    fn default() -> Self {
        Self {
            velocity: UnitValue::ONE,
            lanes: [UnitValue::ZERO; NUM_PARAM_LANES],
        }
    }
}

/// One of the sequencer's 16 tracks: a cyclic sequence of steps.
///
/// Tracks have independent lengths (1..=128 steps), so they wrap — and loop —
/// independently of each other: polymeter.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Track {
    /// Active length in steps, `1..=128`. Invariant guarded by `set_length`.
    #[cfg_attr(feature = "serde", serde(deserialize_with = "de_length"))]
    length: u8,
    /// Default parameters used when a step has no lock.
    pub defaults: Params,
    /// Step storage. Only `steps[..length]` take part in playback.
    #[cfg_attr(feature = "serde", serde(with = "step_array"))]
    pub steps: [Step; MAX_STEPS],
}

/// Rejects out-of-range track lengths on deserialization (corrupt data,
/// unlike a clampable float, indicates a structurally broken file).
#[cfg(feature = "serde")]
fn de_length<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let v = <u8 as serde::Deserialize>::deserialize(deserializer)?;
    if v == 0 || usize::from(v) > MAX_STEPS {
        return Err(serde::de::Error::custom(format!(
            "track length out of range: {v}"
        )));
    }
    Ok(v)
}

/// serde lacks built-in support for arrays longer than 32, so the 128-step
/// array round-trips as a plain sequence.
#[cfg(feature = "serde")]
mod step_array {
    use super::{MAX_STEPS, Step};
    use serde::ser::SerializeSeq;

    pub fn serialize<S: serde::Serializer>(
        steps: &[Step; MAX_STEPS],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(MAX_STEPS))?;
        for step in steps {
            seq.serialize_element(step)?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<[Step; MAX_STEPS], D::Error> {
        let v = <Vec<Step> as serde::Deserialize>::deserialize(deserializer)?;
        let n = v.len();
        v.try_into()
            .map_err(|_| serde::de::Error::invalid_length(n, &"exactly 128 steps"))
    }
}

impl Default for Track {
    fn default() -> Self {
        Self {
            length: 16,
            defaults: Params::default(),
            steps: [Step::default(); MAX_STEPS],
        }
    }
}

impl Track {
    /// The track's active length in steps.
    pub fn length(&self) -> u8 {
        self.length
    }

    /// Sets the track length in steps, `1..=128`. Legal while playing: if the
    /// playhead is left beyond the new length, it wraps into range on the
    /// next tick (without counting a completed loop — an edit artifact is not
    /// a loop).
    ///
    /// # Errors
    /// Returns [`LengthError`] for 0 or anything above [`MAX_STEPS`].
    pub fn set_length(&mut self, steps: u8) -> Result<(), LengthError> {
        if steps == 0 || usize::from(steps) > MAX_STEPS {
            return Err(LengthError {
                steps: u16::from(steps),
            });
        }
        self.length = steps;
        Ok(())
    }

    /// Number of pages the active length occupies (16 steps per page,
    /// rounding up — a 20-step track has 2 pages).
    pub fn page_count(&self) -> usize {
        usize::from(self.length).div_ceil(STEPS_PER_PAGE)
    }

    /// The 16 steps of page `i`, or `None` if `i >= page_count()`. The last
    /// page is always returned in full, even when the track length ends
    /// mid-page (a UI would render the tail dimmed).
    pub fn page(&self, i: usize) -> Option<&[Step]> {
        (i < self.page_count()).then(|| &self.steps[i * STEPS_PER_PAGE..(i + 1) * STEPS_PER_PAGE])
    }

    /// Mutable access to the 16 steps of page `i`, or `None` if out of range.
    pub fn page_mut(&mut self, i: usize) -> Option<&mut [Step]> {
        (i < self.page_count())
            .then(|| &mut self.steps[i * STEPS_PER_PAGE..(i + 1) * STEPS_PER_PAGE])
    }

    /// Sets the length to `pages * 16` steps, `1..=8` pages.
    ///
    /// # Errors
    /// Returns [`LengthError`] for 0 or more than [`MAX_PAGES`] pages.
    pub fn set_page_count(&mut self, pages: usize) -> Result<(), LengthError> {
        let steps = pages * STEPS_PER_PAGE;
        if pages == 0 || pages > MAX_PAGES {
            return Err(LengthError {
                steps: steps as u16,
            });
        }
        self.length = steps as u8;
        Ok(())
    }

    /// Removes every p-lock from every step of this track.
    pub fn clear_all_locks(&mut self) {
        for step in &mut self.steps {
            step.clear_locks();
        }
    }

    /// The track's cyclic timeline length in pulses (`length() × 24`).
    pub fn pulse_length(&self) -> u32 {
        u32::from(self.length) * u32::from(crate::PULSES_PER_STEP)
    }

    /// The pulse position of `step`'s grid slot on the track's timeline
    /// (before micro-timing or swing displacement).
    pub fn step_pulse(&self, step: u8) -> u32 {
        u32::from(step) * u32::from(crate::PULSES_PER_STEP)
    }
}
