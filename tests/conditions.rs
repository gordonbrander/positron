//! Slice 4+: trig condition behavior.

mod common;

use common::fired;
use plock::{Condition, Sequencer, UnitValue};

#[test]
fn percent_one_always_fires() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(1).unwrap();
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::Percent(UnitValue::ONE);

    seq.play();
    for _ in 0..100 {
        assert!(fired(&seq.tick())[0].is_some());
    }
}

#[test]
fn percent_zero_never_fires() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(1).unwrap();
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::Percent(UnitValue::ZERO);

    seq.play();
    for _ in 0..100 {
        assert!(fired(&seq.tick())[0].is_none());
    }
}

#[test]
fn percent_half_fires_about_half_the_time() {
    let mut seq = Sequencer::new(0xFEED);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(1).unwrap();
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::Percent(UnitValue::new(0.5));

    seq.play();
    let fires = (0..10_000)
        .filter(|_| fired(&seq.tick())[0].is_some())
        .count();
    // Generous tolerance band; the contract is "roughly p", not exact.
    assert!((4500..=5500).contains(&fires), "fired {fires}/10000");
}

#[test]
fn fill_fires_only_while_flag_set() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(2).unwrap();
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::Fill;
    track.steps[1].trig = true;
    track.steps[1].condition = Condition::NotFill;

    seq.play();
    // Fill off: only the NotFill step fires.
    assert!(fired(&seq.tick())[0].is_none()); // step 0 (Fill)
    assert!(fired(&seq.tick())[0].is_some()); // step 1 (NotFill)

    // Toggle mid-playback: takes effect immediately.
    seq.set_fill(true);
    assert!(fired(&seq.tick())[0].is_some()); // step 0 (Fill)
    assert!(fired(&seq.tick())[0].is_none()); // step 1 (NotFill)
    seq.set_fill(false);
    assert!(fired(&seq.tick())[0].is_none()); // step 0 again
}

#[test]
fn pre_mirrors_the_previous_conditional_on_the_same_track() {
    let mut seq = Sequencer::new(0xABCD);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(4).unwrap();
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::Percent(UnitValue::new(0.5));
    track.steps[1].trig = true; // Always — must not disturb Pre
    track.steps[2].trig = true;
    track.steps[2].condition = Condition::Pre;
    track.steps[3].trig = true;
    track.steps[3].condition = Condition::NotPre;

    seq.play();
    let mut saw_both = (false, false);
    for _ in 0..64 {
        let upstream = fired(&seq.tick())[0].is_some(); // step 0
        assert!(fired(&seq.tick())[0].is_some()); // step 1 (Always)
        assert_eq!(fired(&seq.tick())[0].is_some(), upstream); // step 2 (Pre)
        assert_eq!(fired(&seq.tick())[0].is_some(), !upstream); // step 3 (NotPre)
        if upstream {
            saw_both.0 = true;
        } else {
            saw_both.1 = true;
        }
    }
    assert!(saw_both.0 && saw_both.1, "seed must exercise both outcomes");
}

#[test]
fn nei_mirrors_the_neighbor_track() {
    let mut seq = Sequencer::new(0x5150);
    {
        let pattern = seq.current_pattern_mut();
        for t in 0..3 {
            pattern.tracks[t].set_length(1).unwrap();
            pattern.tracks[t].steps[0].trig = true;
        }
        pattern.tracks[0].steps[0].condition = Condition::Percent(UnitValue::new(0.5));
        pattern.tracks[1].steps[0].condition = Condition::Nei;
        pattern.tracks[2].steps[0].condition = Condition::NotNei;
    }

    seq.play();
    for _ in 0..64 {
        let f = fired(&seq.tick());
        // Track 1 mirrors track 0's outcome from this same tick.
        assert_eq!(f[1].is_some(), f[0].is_some());
        // Track 2's NotNei reads track 1's state — but Nei is transparent
        // and track 1 has no state-writing conditional, so its state stays
        // false and NotNei always passes.
        assert!(f[2].is_some());
    }
}

#[test]
fn nei_is_transparent_to_state() {
    // Track 2 watching track 1 (whose only condition is Nei) must read
    // track 1's last *state-writing* conditional — there is none, so false.
    let mut seq = Sequencer::new(0x5150);
    {
        let pattern = seq.current_pattern_mut();
        for t in 0..3 {
            pattern.tracks[t].set_length(1).unwrap();
            pattern.tracks[t].steps[0].trig = true;
        }
        pattern.tracks[0].steps[0].condition = Condition::Percent(UnitValue::ONE);
        pattern.tracks[1].steps[0].condition = Condition::Nei;
        pattern.tracks[2].steps[0].condition = Condition::Nei;
    }

    seq.play();
    for _ in 0..16 {
        let f = fired(&seq.tick());
        assert!(f[0].is_some()); // Percent(1.0)
        assert!(f[1].is_some()); // mirrors track 0
        assert!(f[2].is_none()); // track 1 never wrote state
    }
}

#[test]
fn nei_on_track_zero_never_fires() {
    let mut seq = Sequencer::new(1);
    {
        let pattern = seq.current_pattern_mut();
        pattern.tracks[0].set_length(2).unwrap();
        pattern.tracks[0].steps[0].trig = true;
        pattern.tracks[0].steps[0].condition = Condition::Nei;
        pattern.tracks[0].steps[1].trig = true;
        pattern.tracks[0].steps[1].condition = Condition::NotNei;
    }

    seq.play();
    for _ in 0..8 {
        assert!(fired(&seq.tick())[0].is_none()); // Nei
        assert!(fired(&seq.tick())[0].is_some()); // NotNei
    }
}

#[test]
fn first_fires_only_on_the_first_loop_per_track() {
    let mut seq = Sequencer::new(1);
    {
        let pattern = seq.current_pattern_mut();
        pattern.tracks[0].set_length(4).unwrap();
        pattern.tracks[0].steps[0].trig = true;
        pattern.tracks[0].steps[0].condition = Condition::First;
        // Polymeter neighbor proves loop counting is per-track.
        pattern.tracks[1].set_length(6).unwrap();
        pattern.tracks[1].steps[0].trig = true;
        pattern.tracks[1].steps[0].condition = Condition::NotFirst;
    }

    seq.play();
    for tick in 0..24 {
        let f = fired(&seq.tick());
        assert_eq!(f[0].is_some(), tick == 0, "tick {tick}");
        // NotFirst on a 6-step track: fires at 6, 12, 18 but not 0.
        assert_eq!(f[1].is_some(), tick % 6 == 0 && tick != 0, "tick {tick}");
    }

    // play() resets loop counts: First fires again.
    seq.play();
    assert!(fired(&seq.tick())[0].is_some());
}

#[test]
fn ratio_conditions_select_loops() {
    let mut seq = Sequencer::new(1);
    {
        let pattern = seq.current_pattern_mut();
        pattern.tracks[0].set_length(4).unwrap();
        pattern.tracks[0].steps[0].trig = true;
        pattern.tracks[0].steps[0].condition = Condition::ratio(1, 2).unwrap();
        pattern.tracks[1].set_length(4).unwrap();
        pattern.tracks[1].steps[0].trig = true;
        pattern.tracks[1].steps[0].condition = Condition::ratio(2, 2).unwrap();
        pattern.tracks[2].set_length(1).unwrap();
        pattern.tracks[2].steps[0].trig = true;
        pattern.tracks[2].steps[0].condition = Condition::ratio(8, 8).unwrap();
    }

    seq.play();
    for tick in 0..32 {
        let f = fired(&seq.tick());
        assert_eq!(f[0].is_some(), tick % 8 == 0, "1:2 tick {tick}");
        assert_eq!(f[1].is_some(), tick % 8 == 4, "2:2 tick {tick}");
        assert_eq!(f[2].is_some(), tick % 8 == 7, "8:8 tick {tick}");
    }
}

#[test]
fn invalid_ratios_rejected_and_raw_literals_never_panic() {
    assert!(Condition::ratio(0, 2).is_err());
    assert!(Condition::ratio(3, 2).is_err());
    assert!(Condition::ratio(1, 9).is_err());
    assert!(Condition::ratio(1, 1).is_ok());
    assert!(Condition::ratio(8, 8).is_ok());

    // Raw out-of-range literals are defensively clamped, never a panic.
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(1).unwrap();
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::Ratio { a: 5, b: 0 };

    seq.play();
    for _ in 0..16 {
        seq.tick(); // must not panic
    }
}

#[test]
fn probability_gates_locks_too() {
    // A p-lock only lands when its trig actually fires.
    let mut seq = Sequencer::new(3);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(1).unwrap();
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::Percent(UnitValue::new(0.5));
    seq.current_pattern_mut()
        .set_lane_lock(0, 0, 4, UnitValue::new(0.9))
        .unwrap();

    seq.play();
    let mut fired_count = 0;
    for _ in 0..1000 {
        let out = seq.tick();
        for ev in &out {
            assert_eq!(ev.lanes[4], 0.9); // present iff fired
            fired_count += 1;
        }
    }
    assert!(fired_count > 0);
}
