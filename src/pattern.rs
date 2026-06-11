//! Patterns: the pure-data layer of the sequencer.

use crate::locks::{LockError, LockLane, ResolvedStep, VELOCITY_DEST};
use crate::{MAX_LOCK_LANES, MAX_STEPS, NUM_PARAM_LANES, NUM_TRACKS, Track, UnitValue};
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

/// The lock pool round-trips sparsely: each occupied slot becomes
/// `{track, dest, steps: [(step, value), …]}` with only its locked steps
/// listed (in ascending order), so files stay small and contain no `u128`.
/// Values re-clamp for free via `UnitValue`; everything structural — bad
/// indices, duplicate destinations or steps, empty entries, an
/// over-capacity pool — is rejected, since it indicates a broken file
/// rather than a clampable scalar.
#[cfg(feature = "serde")]
mod lock_pool {
    use super::{
        LockLane, MAX_LOCK_LANES, MAX_STEPS, NUM_PARAM_LANES, NUM_TRACKS, UnitValue, VELOCITY_DEST,
    };
    use serde::de::Error as _;
    use serde::ser::SerializeSeq;

    /// Serde-only mirror of one pool slot.
    #[derive(serde::Serialize, serde::Deserialize)]
    struct LockLaneRepr {
        track: u8,
        dest: u8,
        steps: Vec<(u8, UnitValue)>,
    }

    pub fn serialize<S: serde::Serializer>(
        locks: &[LockLane],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(locks.len()))?;
        for lock in locks {
            let steps: Vec<(u8, UnitValue)> = (0..MAX_STEPS)
                .filter(|&i| lock.steps >> i & 1 == 1)
                .map(|i| (i as u8, lock.values[i]))
                .collect();
            seq.serialize_element(&LockLaneRepr {
                track: lock.track,
                dest: lock.dest,
                steps,
            })?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<LockLane>, D::Error> {
        let reprs = <Vec<LockLaneRepr> as serde::Deserialize>::deserialize(deserializer)?;
        if reprs.len() > MAX_LOCK_LANES {
            return Err(D::Error::custom(format!(
                "lock pool over capacity: {} > {MAX_LOCK_LANES}",
                reprs.len()
            )));
        }
        let mut pool: Vec<LockLane> = Vec::with_capacity(reprs.len());
        for repr in reprs {
            if usize::from(repr.dest) >= NUM_PARAM_LANES && repr.dest != VELOCITY_DEST {
                return Err(D::Error::custom(format!(
                    "lock destination out of range: {}",
                    repr.dest
                )));
            }
            if usize::from(repr.track) >= NUM_TRACKS {
                return Err(D::Error::custom(format!(
                    "lock track out of range: {}",
                    repr.track
                )));
            }
            if pool
                .iter()
                .any(|l| l.track == repr.track && l.dest == repr.dest)
            {
                return Err(D::Error::custom(format!(
                    "duplicate lock destination: track {}, dest {}",
                    repr.track, repr.dest
                )));
            }
            if repr.steps.is_empty() {
                return Err(D::Error::custom(
                    "empty lock entry (a freed slot must be removed, not kept)",
                ));
            }
            let mut lock = LockLane::new(repr.track, repr.dest);
            for (step, value) in repr.steps {
                let i = usize::from(step);
                if i >= MAX_STEPS {
                    return Err(D::Error::custom(format!("lock step out of range: {step}")));
                }
                if lock.steps >> i & 1 == 1 {
                    return Err(D::Error::custom(format!("duplicate lock step: {step}")));
                }
                lock.steps |= 1u128 << i;
                lock.values[i] = value;
            }
            pool.push(lock);
        }
        Ok(pool)
    }
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
    /// The parameter-lock pool: at most [`MAX_LOCK_LANES`] distinct
    /// (track, destination) entries, edited only through the lock API so
    /// the cap and invariants hold. Heap, touched only at edit time —
    /// never by `tick()`.
    #[cfg_attr(feature = "serde", serde(default, with = "lock_pool"))]
    locks: Vec<LockLane>,
}

impl Default for Pattern {
    fn default() -> Self {
        Self {
            tracks: std::array::from_fn(|_| Track::default()),
            swing: 50,
            change_length: None,
            locks: Vec::new(),
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

    // --- Parameter locks ---------------------------------------------------
    //
    // Locks live in a per-pattern pool: one slot per distinct
    // (track, destination), at most MAX_LOCK_LANES slots. The methods below
    // are the only way to touch the pool, which is what keeps its invariants
    // (cap, no duplicate destinations, empty slots freed, cleared value
    // slots zeroed). None of them panic.

    /// Locks `lane` of `track` at `step` to `value` — an Elektron p-lock.
    /// The locked value rides on the step's fired events instead of the
    /// track default.
    ///
    /// # Errors
    /// [`LockError::OutOfRange`] for invalid indices.
    /// [`LockError::PoolFull`] when this would occupy a new pool slot and
    /// the pattern already holds [`MAX_LOCK_LANES`] distinct destinations
    /// (locking more steps of an already-locked destination always
    /// succeeds).
    pub fn set_lane_lock(
        &mut self,
        track: usize,
        step: usize,
        lane: usize,
        value: UnitValue,
    ) -> Result<(), LockError> {
        if lane >= NUM_PARAM_LANES {
            return Err(LockError::OutOfRange);
        }
        self.set_lock(track, step, lane as u8, value)
    }

    /// The lock on `lane` of `track` at `step`, or `None` if that lane is
    /// unlocked there (or any index is out of range).
    pub fn lane_lock(&self, track: usize, step: usize, lane: usize) -> Option<UnitValue> {
        if lane >= NUM_PARAM_LANES {
            return None;
        }
        self.get_lock(track, step, lane as u8)
    }

    /// Removes the lock on `lane` of `track` at `step`; the step reverts to
    /// the track default. Clearing a destination's last locked step frees
    /// its pool slot. No-op for unlocked steps or out-of-range indices.
    pub fn clear_lane_lock(&mut self, track: usize, step: usize, lane: usize) {
        if lane < NUM_PARAM_LANES {
            self.clear_lock(track, step, lane as u8);
        }
    }

    /// Locks `track`'s velocity at `step` to `value`. Velocity is a lock
    /// destination like any lane and counts toward the pool cap.
    ///
    /// # Errors
    /// [`LockError::OutOfRange`] for invalid indices.
    /// [`LockError::PoolFull`] when this would occupy a new pool slot and
    /// the pool is full.
    pub fn set_velocity_lock(
        &mut self,
        track: usize,
        step: usize,
        value: UnitValue,
    ) -> Result<(), LockError> {
        self.set_lock(track, step, VELOCITY_DEST, value)
    }

    /// The velocity lock of `track` at `step`, or `None` if velocity is
    /// unlocked there (or any index is out of range).
    pub fn velocity_lock(&self, track: usize, step: usize) -> Option<UnitValue> {
        self.get_lock(track, step, VELOCITY_DEST)
    }

    /// Removes the velocity lock of `track` at `step`. No-op for unlocked
    /// steps or out-of-range indices.
    pub fn clear_velocity_lock(&mut self, track: usize, step: usize) {
        self.clear_lock(track, step, VELOCITY_DEST);
    }

    /// Removes every lock (velocity and all lanes) of `track` at `step`.
    pub fn clear_step_locks(&mut self, track: usize, step: usize) {
        if track >= NUM_TRACKS || step >= MAX_STEPS {
            return;
        }
        let t = track as u8;
        self.locks.retain_mut(|lock| {
            if lock.track == t && lock.steps >> step & 1 == 1 {
                lock.steps &= !(1u128 << step);
                lock.values[step] = UnitValue::ZERO;
            }
            lock.steps != 0
        });
    }

    /// Removes every lock on every step of `track`, freeing its pool slots.
    pub fn clear_track_locks(&mut self, track: usize) {
        if track >= NUM_TRACKS {
            return;
        }
        let t = track as u8;
        self.locks.retain(|lock| lock.track != t);
    }

    /// Removes every lock in the pattern, emptying the pool.
    pub fn clear_all_locks(&mut self) {
        self.locks.clear();
    }

    /// Number of occupied pool slots — distinct (track, destination) pairs
    /// locked anywhere in the pattern, `0..=MAX_LOCK_LANES`.
    pub fn lock_count(&self) -> usize {
        self.locks.len()
    }

    /// What `track`'s `step` would play against the track defaults: per
    /// parameter, the lock if set, else the default — plus which values came
    /// from locks. Pure data, useful for UI display as well as playback;
    /// allocation-free. `None` if either index is out of range.
    pub fn resolve_step(&self, track: usize, step: usize) -> Option<ResolvedStep> {
        if track >= NUM_TRACKS || step >= MAX_STEPS {
            return None;
        }
        let defaults = &self.tracks[track].defaults;
        let mut resolved = ResolvedStep {
            velocity: defaults.velocity.get(),
            lanes: std::array::from_fn(|i| defaults.lanes[i].get()),
            locked: 0,
            velocity_locked: false,
        };
        let t = track as u8;
        for lock in &self.locks {
            if lock.track != t || lock.steps >> step & 1 == 0 {
                continue;
            }
            if lock.dest == VELOCITY_DEST {
                resolved.velocity = lock.values[step].get();
                resolved.velocity_locked = true;
            } else if let Some(slot) = resolved.lanes.get_mut(usize::from(lock.dest)) {
                *slot = lock.values[step].get();
                resolved.locked |= 1 << lock.dest;
            }
        }
        Some(resolved)
    }

    /// Shared set path: validates indices, then writes into the existing
    /// slot for (track, dest) or allocates a new one.
    fn set_lock(
        &mut self,
        track: usize,
        step: usize,
        dest: u8,
        value: UnitValue,
    ) -> Result<(), LockError> {
        if track >= NUM_TRACKS || step >= MAX_STEPS {
            return Err(LockError::OutOfRange);
        }
        let t = track as u8;
        if let Some(lock) = self
            .locks
            .iter_mut()
            .find(|l| l.track == t && l.dest == dest)
        {
            lock.steps |= 1u128 << step;
            lock.values[step] = value;
            return Ok(());
        }
        if self.locks.len() >= MAX_LOCK_LANES {
            return Err(LockError::PoolFull);
        }
        let mut lock = LockLane::new(t, dest);
        lock.steps = 1u128 << step;
        lock.values[step] = value;
        self.locks.push(lock);
        Ok(())
    }

    /// Shared get path.
    fn get_lock(&self, track: usize, step: usize, dest: u8) -> Option<UnitValue> {
        if track >= NUM_TRACKS || step >= MAX_STEPS {
            return None;
        }
        let t = track as u8;
        self.locks
            .iter()
            .find(|l| l.track == t && l.dest == dest)
            .filter(|l| l.steps >> step & 1 == 1)
            .map(|l| l.values[step])
    }

    /// Shared clear path: clears the step's bit, restores the canonical
    /// zero value, and frees the slot when its last step is cleared
    /// (`Vec::remove` keeps insertion order, so serialization and equality
    /// stay deterministic for a given edit history).
    fn clear_lock(&mut self, track: usize, step: usize, dest: u8) {
        if track >= NUM_TRACKS || step >= MAX_STEPS {
            return;
        }
        let t = track as u8;
        if let Some(i) = self
            .locks
            .iter()
            .position(|l| l.track == t && l.dest == dest)
        {
            let lock = &mut self.locks[i];
            lock.steps &= !(1u128 << step);
            lock.values[step] = UnitValue::ZERO;
            if lock.steps == 0 {
                self.locks.remove(i);
            }
        }
    }
}
