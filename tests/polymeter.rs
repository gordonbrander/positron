//! Slice 2: per-track length, polymeter, and pages.

mod common;

use common::{fire_ticks, fired};
use plock::{MAX_STEPS, Sequencer, UnitValue};
use proptest::prelude::*;

#[test]
fn tracks_with_different_lengths_drift_and_realign() {
    let mut seq = Sequencer::new(1);
    let pattern = seq.current_pattern_mut();
    pattern.tracks[0].steps[0].trig = true; // length 16
    pattern.tracks[1].set_length(12).unwrap();
    pattern.tracks[1].steps[0].trig = true;

    seq.play();
    let mut together = Vec::new();
    for tick in 0..48 {
        let f = fired(&seq.tick());
        if f[0].is_some() && f[1].is_some() {
            together.push(tick);
        }
    }
    // lcm(16, 12) = 48: both fire at 0, then drift until tick 48 (excluded).
    assert_eq!(together, vec![0]);
    let realigned = fired(&seq.tick()); // tick 48
    assert_eq!(realigned[0], Some(1.0));
    assert_eq!(realigned[1], Some(1.0));
}

#[test]
fn both_tracks_refire_together_at_lcm() {
    let mut seq = Sequencer::new(1);
    let pattern = seq.current_pattern_mut();
    pattern.tracks[1].set_length(12).unwrap();
    pattern.tracks[0].steps[0].trig = true;
    pattern.tracks[1].steps[0].trig = true;

    seq.play();
    for tick in 0..96 {
        let f = fired(&seq.tick());
        assert_eq!(f[0].is_some(), tick % 16 == 0, "track 0, tick {tick}");
        assert_eq!(f[1].is_some(), tick % 12 == 0, "track 1, tick {tick}");
    }
}

#[test]
fn length_one_track_fires_every_tick() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[2];
    track.set_length(1).unwrap();
    track.steps[0].trig = true;

    seq.play();
    assert_eq!(fire_ticks(&mut seq, 2, 8), vec![0, 1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn shortening_length_below_playhead_rewraps_safely() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(32).unwrap();
    track.steps[4].trig = true; // 20 % 16 == 4

    seq.play();
    for _ in 0..20 {
        seq.tick();
    }
    assert_eq!(seq.playheads()[0], 20);

    seq.current_pattern_mut().tracks[0].set_length(16).unwrap();
    // Next tick wraps 20 -> 4, which carries the trig.
    assert_eq!(fired(&seq.tick())[0], Some(1.0));
    assert_eq!(seq.playheads()[0], 5);
}

#[test]
fn invalid_lengths_rejected() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    assert!(track.set_length(0).is_err());
    assert!(track.set_length(129).is_err());
    assert!(track.set_length(MAX_STEPS as u8).is_ok());
    assert_eq!(track.length(), 128);
}

#[test]
fn page_helpers_index_correctly() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(64).unwrap();
    assert_eq!(track.page_count(), 4);

    track.steps[16].trig = true; // first step of page 1
    let page1 = track.page(1).unwrap();
    assert!(page1[0].trig);
    assert_eq!(page1.len(), 16);

    assert!(track.page(4).is_none());
    assert!(track.page_mut(4).is_none());

    // Partial last page rounds up and is returned in full.
    track.set_length(20).unwrap();
    assert_eq!(track.page_count(), 2);
    assert_eq!(track.page(1).unwrap().len(), 16);
}

#[test]
fn page_count_setter() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_page_count(8).unwrap();
    assert_eq!(track.length(), 128);
    assert!(track.set_page_count(0).is_err());
    assert!(track.set_page_count(9).is_err());

    // Default velocity untouched by length edits.
    assert_eq!(track.defaults.velocity, UnitValue::ONE);
}

proptest! {
    /// For any length, the playhead visits exactly the steps `0..length`,
    /// cyclically, from step 0.
    #[test]
    fn playhead_cycles_through_exactly_the_active_steps(length in 1u8..=128) {
        let mut seq = Sequencer::new(1);
        seq.current_pattern_mut().tracks[0].set_length(length).unwrap();
        seq.play();
        for tick in 0..(usize::from(length) * 2 + 3) {
            prop_assert_eq!(
                usize::from(seq.playheads()[0]),
                tick % usize::from(length)
            );
            seq.tick();
        }
    }
}
