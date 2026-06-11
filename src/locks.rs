//! The pattern-level parameter-lock pool.
//!
//! Locks live per pattern, not per step, as on the hardware: each distinct
//! (track, destination) pair locked anywhere in the pattern occupies one pool
//! slot, and a pattern holds at most [`MAX_LOCK_LANES`](crate::MAX_LOCK_LANES)
//! slots. Locking more steps of an already-locked destination is free;
//! locking a new destination when the pool is full fails with
//! [`LockError::PoolFull`]; clearing a destination's last locked step frees
//! its slot. The pool is private to [`Pattern`](crate::Pattern) and edited
//! only through its lock API.

use crate::{MAX_STEPS, NUM_PARAM_LANES, UnitValue};

/// Sentinel destination meaning "velocity" in pool entries.
pub(crate) const VELOCITY_DEST: u8 = u8::MAX;

/// One pool slot: every lock the pattern holds for a single
/// (track, destination) pair, across all 128 steps.
///
/// Invariants, maintained by the `Pattern` lock API:
/// - at most one entry per distinct `(track, dest)` in a pool;
/// - an entry whose `steps` mask is empty is removed (its slot is freed);
/// - value slots whose mask bit is clear are canonically `UnitValue::ZERO`,
///   so the derived `PartialEq` stays honest after clears.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LockLane {
    /// Which track, `0..NUM_TRACKS`.
    pub(crate) track: u8,
    /// Destination: `0..NUM_PARAM_LANES` = lane index; `VELOCITY_DEST` =
    /// the track's velocity.
    pub(crate) dest: u8,
    /// Bit `i` set = step `i` holds a lock.
    pub(crate) steps: u128,
    /// Per-step lock values; only slots whose mask bit is set are meaningful.
    pub(crate) values: [UnitValue; MAX_STEPS],
}

impl LockLane {
    /// An empty lane for `(track, dest)` — no steps locked yet.
    pub(crate) fn new(track: u8, dest: u8) -> Self {
        Self {
            track,
            dest,
            steps: 0,
            values: [UnitValue::ZERO; MAX_STEPS],
        }
    }
}

/// Error returned by the [`Pattern`](crate::Pattern) lock-editing API.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockError {
    /// The pattern already holds [`MAX_LOCK_LANES`](crate::MAX_LOCK_LANES)
    /// distinct (track, destination) locks. Locking more steps of an
    /// already-locked destination still succeeds; freeing any destination
    /// (clearing its last locked step) makes room for a new one.
    PoolFull,
    /// Track, step, or lane index out of range.
    OutOfRange,
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PoolFull => write!(
                f,
                "lock pool is full ({} distinct destinations)",
                crate::MAX_LOCK_LANES
            ),
            Self::OutOfRange => write!(f, "track, step, or lane index out of range"),
        }
    }
}

impl std::error::Error for LockError {}

/// What a step would play against its track defaults — velocity, lane
/// values, and which of them came from locks rather than defaults.
///
/// Returned by value from [`Pattern::resolve_step`](crate::Pattern::resolve_step).
/// Pure data — useful for UI display as well as playback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedStep {
    /// The step's velocity: lock if set, else the track default.
    pub velocity: f32,
    /// Per lane, the step's lock if set, else the track default.
    pub lanes: [f32; NUM_PARAM_LANES],
    /// Bit `i` set ⇔ `lanes[i]` came from a lock. Lanes only; velocity has
    /// its own flag (64 lanes use the whole mask, and velocity is
    /// distinguished everywhere else in the crate too).
    pub locked: u64,
    /// True iff `velocity` came from a velocity lock.
    pub velocity_locked: bool,
}
