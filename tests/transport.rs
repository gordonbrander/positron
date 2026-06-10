//! Slice 1: basic trigs, transport, and wrap behavior.

mod common;

use common::{fire_ticks, fired};
use elektronlike::{Sequencer, UnitValue};

#[test]
fn programmed_steps_fire_on_the_right_ticks() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[3];
    track.steps[0].trig = true;
    track.steps[4].trig = true;
    track.steps[15].trig = true;

    seq.play();
    assert_eq!(fire_ticks(&mut seq, 3, 16), vec![0, 4, 15]);
}

#[test]
fn wraps_after_sixteen_ticks() {
    let mut seq = Sequencer::new(1);
    seq.current_pattern_mut().tracks[0].steps[2].trig = true;

    seq.play();
    assert_eq!(fire_ticks(&mut seq, 0, 48), vec![2, 18, 34]);
}

#[test]
fn tick_while_stopped_is_a_no_op() {
    let mut seq = Sequencer::new(1);
    seq.current_pattern_mut().tracks[0].steps[0].trig = true;

    assert!(!seq.is_playing());
    assert!(seq.tick().is_empty());
    assert!(seq.tick().is_empty());

    // Stopping mid-pattern freezes the position; only play() resets it.
    seq.play();
    seq.tick(); // step 0 fires
    seq.stop();
    assert!(seq.tick().is_empty());
}

#[test]
fn play_restarts_from_step_zero() {
    let mut seq = Sequencer::new(1);
    seq.current_pattern_mut().tracks[0].steps[0].trig = true;

    seq.play();
    assert_eq!(fired(&seq.tick())[0], Some(1.0)); // step 0
    seq.tick(); // step 1
    seq.tick(); // step 2

    seq.play(); // restart while playing
    assert_eq!(fired(&seq.tick())[0], Some(1.0)); // step 0 again
}

#[test]
fn velocity_output_equals_track_default() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[5];
    track.defaults.velocity = UnitValue::new(0.7);
    track.steps[0].trig = true;

    seq.play();
    assert_eq!(fired(&seq.tick())[5], Some(0.7));
}

#[test]
fn lanes_default_to_track_defaults() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.defaults.lanes[3] = UnitValue::new(0.25);
    track.steps[0].trig = true;

    seq.play();
    let out = seq.tick();
    let ev = out.as_slice()[0];
    assert_eq!(ev.lanes[3], 0.25);
    assert_eq!(ev.lanes[0], 0.0);
}

#[test]
fn edits_while_playing_take_effect_on_next_pass() {
    let mut seq = Sequencer::new(1);
    seq.play();
    seq.tick(); // step 0, no trig yet

    seq.current_pattern_mut().tracks[0].steps[0].trig = true;
    // steps 1..=15 of this pass, then step 0 of the next pass fires.
    assert_eq!(fire_ticks(&mut seq, 0, 16), vec![15]);
}

#[test]
fn all_sixteen_tracks_fire_independently() {
    let mut seq = Sequencer::new(1);
    for (i, track) in seq.current_pattern_mut().tracks.iter_mut().enumerate() {
        track.steps[i].trig = true;
    }

    seq.play();
    for step in 0..16 {
        let f = fired(&seq.tick());
        for (t, v) in f.iter().enumerate() {
            assert_eq!(v.is_some(), t == step, "tick {step}, track {t}");
        }
    }
}
