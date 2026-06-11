//! Slice 3: parameter locks (velocity + lanes).

mod common;

use common::fired;
use plock::{Sequencer, UnitValue};

#[test]
fn locked_step_emits_lock_value_unlocked_emit_default() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.defaults.velocity = UnitValue::new(0.8);
    track.steps[0].trig = true;
    track.steps[4].trig = true;
    track.steps[4].locks.velocity = Some(UnitValue::new(0.3));

    seq.play();
    assert_eq!(fired(&seq.tick())[0], Some(0.8)); // step 0: default
    for _ in 1..4 {
        seq.tick();
    }
    assert_eq!(fired(&seq.tick())[0], Some(0.3)); // step 4: lock
}

#[test]
fn lane_locks_override_individual_lanes_only() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.defaults.lanes[2] = UnitValue::new(0.5);
    track.defaults.lanes[7] = UnitValue::new(0.9);
    track.steps[0].trig = true;
    track.steps[0].locks.lanes[2] = Some(UnitValue::new(0.1));

    seq.play();
    let out = seq.tick();
    let ev = out.as_slice()[0];
    assert_eq!(ev.lanes[2], 0.1); // locked
    assert_eq!(ev.lanes[7], 0.9); // default rides through
    assert_eq!(ev.lanes[0], 0.0);
}

#[test]
fn changing_track_default_does_not_affect_locked_steps() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.steps[0].trig = true;
    track.steps[0].locks.velocity = Some(UnitValue::new(0.3));

    seq.current_pattern_mut().tracks[0].defaults.velocity = UnitValue::new(0.6);
    seq.play();
    assert_eq!(fired(&seq.tick())[0], Some(0.3));
}

#[test]
fn clearing_a_lock_reverts_to_default() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.steps[0].trig = true;
    track.steps[0].locks.velocity = Some(UnitValue::new(0.3));
    track.steps[0].locks.lanes[5] = Some(UnitValue::new(0.4));
    track.steps[0].clear_locks();

    seq.play();
    let out = seq.tick();
    let ev = out.as_slice()[0];
    assert_eq!(ev.velocity, 1.0);
    assert_eq!(ev.lanes[5], 0.0);
}

#[test]
fn clear_all_locks_sweeps_the_track() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    for step in &mut track.steps {
        step.locks.velocity = Some(UnitValue::new(0.2));
    }
    track.clear_all_locks();
    assert!(track.steps.iter().all(|s| s.locks.velocity.is_none()));
}

#[test]
fn lock_on_non_trig_step_emits_nothing() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.steps[0].locks.velocity = Some(UnitValue::new(0.3));
    track.steps[0].locks.lanes[0] = Some(UnitValue::new(0.7));
    // No trig: the locks are inert.

    seq.play();
    assert!(seq.tick().is_empty());
}

#[test]
fn locks_survive_transport_changes() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.steps[0].trig = true;
    track.steps[0].locks.velocity = Some(UnitValue::new(0.3));

    seq.play();
    assert_eq!(fired(&seq.tick())[0], Some(0.3));
    seq.stop();
    seq.play();
    assert_eq!(fired(&seq.tick())[0], Some(0.3));
}
