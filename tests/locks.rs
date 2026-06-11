//! Slices 3 + 12: parameter locks (velocity + lanes) in the pattern-level
//! lock pool.

mod common;

use common::fired;
use plock::{LockError, MAX_LOCK_LANES, NUM_PARAM_LANES, Sequencer, UnitValue};

#[test]
fn locked_step_emits_lock_value_unlocked_emit_default() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.defaults.velocity = UnitValue::new(0.8);
    track.steps[0].trig = true;
    track.steps[4].trig = true;
    seq.current_pattern_mut()
        .set_velocity_lock(0, 4, UnitValue::new(0.3))
        .unwrap();

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
    seq.current_pattern_mut()
        .set_lane_lock(0, 0, 2, UnitValue::new(0.1))
        .unwrap();

    seq.play();
    let out = seq.tick();
    let ev = out.as_slice()[0];
    assert_eq!(ev.lanes[2], 0.1); // locked
    assert_eq!(ev.lanes[7], 0.9); // default rides through
    assert_eq!(ev.lanes[0], 0.0);
}

#[test]
fn events_carry_the_locked_mask() {
    let mut seq = Sequencer::new(1);
    seq.current_pattern_mut().tracks[0].steps[0].trig = true;
    seq.current_pattern_mut().tracks[0].steps[1].trig = true;
    let p = seq.current_pattern_mut();
    p.set_lane_lock(0, 0, 2, UnitValue::new(0.1)).unwrap();
    p.set_lane_lock(0, 0, 63, UnitValue::new(0.6)).unwrap();
    p.set_velocity_lock(0, 0, UnitValue::new(0.4)).unwrap();

    seq.play();
    let out = seq.tick();
    let ev = out.as_slice()[0];
    assert_eq!(ev.locked, 1 << 2 | 1 << 63); // exactly the locked lanes
    assert!(ev.velocity_locked);

    // Step 1 has no locks: provenance is all-default.
    let out = seq.tick();
    let ev = out.as_slice()[0];
    assert_eq!(ev.locked, 0);
    assert!(!ev.velocity_locked);
}

#[test]
fn changing_track_default_does_not_affect_locked_steps() {
    let mut seq = Sequencer::new(1);
    seq.current_pattern_mut().tracks[0].steps[0].trig = true;
    seq.current_pattern_mut()
        .set_velocity_lock(0, 0, UnitValue::new(0.3))
        .unwrap();

    seq.current_pattern_mut().tracks[0].defaults.velocity = UnitValue::new(0.6);
    seq.play();
    assert_eq!(fired(&seq.tick())[0], Some(0.3));
}

#[test]
fn clearing_a_lock_reverts_to_default() {
    let mut seq = Sequencer::new(1);
    seq.current_pattern_mut().tracks[0].steps[0].trig = true;
    let p = seq.current_pattern_mut();
    p.set_velocity_lock(0, 0, UnitValue::new(0.3)).unwrap();
    p.set_lane_lock(0, 0, 5, UnitValue::new(0.4)).unwrap();
    p.clear_velocity_lock(0, 0);
    p.clear_lane_lock(0, 0, 5);
    assert_eq!(p.lock_count(), 0); // both slots freed

    seq.play();
    let out = seq.tick();
    let ev = out.as_slice()[0];
    assert_eq!(ev.velocity, 1.0);
    assert_eq!(ev.lanes[5], 0.0);
    assert_eq!(ev.locked, 0);
    assert!(!ev.velocity_locked);
}

#[test]
fn clear_track_locks_sweeps_the_track() {
    let mut seq = Sequencer::new(1);
    let p = seq.current_pattern_mut();
    for step in 0..16 {
        p.set_velocity_lock(0, step, UnitValue::new(0.2)).unwrap();
        p.set_lane_lock(0, step, 3, UnitValue::new(0.7)).unwrap();
    }
    p.set_lane_lock(1, 0, 0, UnitValue::new(0.5)).unwrap(); // another track
    assert_eq!(p.lock_count(), 3);

    p.clear_track_locks(0);
    assert_eq!(p.lock_count(), 1); // track 1's lock survives
    assert_eq!(p.velocity_lock(0, 0), None);
    assert_eq!(p.lane_lock(0, 0, 3), None);
    assert_eq!(p.lane_lock(1, 0, 0), Some(UnitValue::new(0.5)));
}

#[test]
fn clear_step_locks_clears_one_step_only() {
    let mut seq = Sequencer::new(1);
    let p = seq.current_pattern_mut();
    p.set_velocity_lock(0, 4, UnitValue::new(0.2)).unwrap();
    p.set_lane_lock(0, 4, 3, UnitValue::new(0.7)).unwrap();
    p.set_lane_lock(0, 5, 3, UnitValue::new(0.8)).unwrap();

    p.clear_step_locks(0, 4);
    assert_eq!(p.velocity_lock(0, 4), None);
    assert_eq!(p.lane_lock(0, 4, 3), None);
    assert_eq!(p.lane_lock(0, 5, 3), Some(UnitValue::new(0.8)));
    // The velocity slot's only step was cleared → freed; lane 3 still has
    // step 5 → kept.
    assert_eq!(p.lock_count(), 1);
}

#[test]
fn lock_on_non_trig_step_emits_nothing() {
    let mut seq = Sequencer::new(1);
    let p = seq.current_pattern_mut();
    p.set_velocity_lock(0, 0, UnitValue::new(0.3)).unwrap();
    p.set_lane_lock(0, 0, 0, UnitValue::new(0.7)).unwrap();
    // No trig: the locks are inert.

    seq.play();
    assert!(seq.tick().is_empty());
}

#[test]
fn locks_survive_transport_changes() {
    let mut seq = Sequencer::new(1);
    seq.current_pattern_mut().tracks[0].steps[0].trig = true;
    seq.current_pattern_mut()
        .set_velocity_lock(0, 0, UnitValue::new(0.3))
        .unwrap();

    seq.play();
    assert_eq!(fired(&seq.tick())[0], Some(0.3));
    seq.stop();
    seq.play();
    assert_eq!(fired(&seq.tick())[0], Some(0.3));
}

#[test]
fn pool_holds_80_distinct_destinations_and_rejects_the_81st() {
    let mut seq = Sequencer::new(1);
    let p = seq.current_pattern_mut();
    // 16 tracks × 5 lanes = exactly MAX_LOCK_LANES distinct destinations.
    for track in 0..16 {
        for lane in 0..5 {
            p.set_lane_lock(track, 0, lane, UnitValue::new(0.5))
                .unwrap();
        }
    }
    assert_eq!(p.lock_count(), MAX_LOCK_LANES);

    // An 81st distinct destination fails…
    assert_eq!(
        p.set_lane_lock(0, 0, 5, UnitValue::new(0.5)),
        Err(LockError::PoolFull)
    );
    assert_eq!(
        p.set_velocity_lock(0, 0, UnitValue::new(0.5)),
        Err(LockError::PoolFull)
    );
    // …but more steps of an existing destination are free.
    p.set_lane_lock(0, 99, 0, UnitValue::new(0.9)).unwrap();
    assert_eq!(p.lock_count(), MAX_LOCK_LANES);
}

#[test]
fn freeing_a_slot_makes_room_for_a_new_destination() {
    let mut seq = Sequencer::new(1);
    let p = seq.current_pattern_mut();
    for track in 0..16 {
        for lane in 0..5 {
            p.set_lane_lock(track, 0, lane, UnitValue::new(0.5))
                .unwrap();
        }
    }
    // One destination locked on two steps: clearing one keeps the slot…
    p.set_lane_lock(0, 1, 0, UnitValue::new(0.5)).unwrap();
    p.clear_lane_lock(0, 1, 0);
    assert_eq!(p.lock_count(), MAX_LOCK_LANES);
    assert_eq!(
        p.set_lane_lock(0, 0, 5, UnitValue::new(0.5)),
        Err(LockError::PoolFull)
    );
    // …clearing the last step frees it, and a new destination fits.
    p.clear_lane_lock(0, 0, 0);
    assert_eq!(p.lock_count(), MAX_LOCK_LANES - 1);
    p.set_lane_lock(0, 0, 5, UnitValue::new(0.5)).unwrap();
}

#[test]
fn one_destination_costs_one_slot_no_matter_how_many_steps() {
    let mut seq = Sequencer::new(1);
    let p = seq.current_pattern_mut();
    for step in 0..128 {
        p.set_lane_lock(0, step, 0, UnitValue::new(0.5)).unwrap();
    }
    assert_eq!(p.lock_count(), 1);
    // Velocity on the same track is a distinct destination.
    p.set_velocity_lock(0, 0, UnitValue::new(0.5)).unwrap();
    assert_eq!(p.lock_count(), 2);
}

#[test]
fn out_of_range_indices_never_panic() {
    let mut seq = Sequencer::new(1);
    let p = seq.current_pattern_mut();
    assert_eq!(
        p.set_lane_lock(16, 0, 0, UnitValue::new(0.5)),
        Err(LockError::OutOfRange)
    );
    assert_eq!(
        p.set_lane_lock(0, 128, 0, UnitValue::new(0.5)),
        Err(LockError::OutOfRange)
    );
    assert_eq!(
        p.set_lane_lock(0, 0, NUM_PARAM_LANES, UnitValue::new(0.5)),
        Err(LockError::OutOfRange)
    );
    assert_eq!(
        p.set_velocity_lock(99, 0, UnitValue::new(0.5)),
        Err(LockError::OutOfRange)
    );
    assert_eq!(p.lane_lock(16, 0, 0), None);
    assert_eq!(p.lane_lock(0, 128, 0), None);
    assert_eq!(p.lane_lock(0, 0, NUM_PARAM_LANES), None);
    assert_eq!(p.velocity_lock(16, 0), None);
    p.clear_lane_lock(16, 0, 0); // no-ops, no panic
    p.clear_velocity_lock(0, 128);
    p.clear_step_locks(16, 0);
    p.clear_track_locks(16);
    assert_eq!(p.resolve_step(99, 0), None);
    assert_eq!(p.resolve_step(0, 128), None);
    assert_eq!(p.lock_count(), 0);
}
