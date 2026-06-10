//! Shared helpers for integration tests.
#![allow(dead_code)]

use elektronlike::{NUM_TRACKS, Sequencer, TickOutput};

/// Projects a tick's events onto per-track velocities — the "did each track
/// fire, and how hard" view that most tests assert against.
pub fn fired(out: &TickOutput) -> [Option<f32>; NUM_TRACKS] {
    let mut v = [None; NUM_TRACKS];
    for ev in out {
        v[ev.track as usize] = Some(ev.velocity);
    }
    v
}

/// Ticks `n` times, collecting the per-track velocity projection of each tick.
pub fn fired_n(seq: &mut Sequencer, n: usize) -> Vec<[Option<f32>; NUM_TRACKS]> {
    (0..n).map(|_| fired(&seq.tick())).collect()
}

/// The tick indices (within `n` ticks) on which `track` fired.
pub fn fire_ticks(seq: &mut Sequencer, track: usize, n: usize) -> Vec<usize> {
    fired_n(seq, n)
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f[track].map(|_| i))
        .collect()
}
