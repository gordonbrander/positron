//! Slices 6–8: pulse timeline, micro-timing, and swing.

mod common;

use plock::{PULSES_PER_STEP, Sequencer};

#[test]
fn pulse_helpers() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    assert_eq!(track.pulse_length(), 16 * 24);
    track.set_length(12).unwrap();
    assert_eq!(track.pulse_length(), 12 * 24);
    assert_eq!(track.step_pulse(5), 120);
    assert_eq!(u32::from(PULSES_PER_STEP), 24);
}

#[test]
fn events_are_ordered_by_offset_then_track() {
    let mut seq = Sequencer::new(1);
    {
        let pattern = seq.current_pattern_mut();
        // Program out of track order; output must come back track-sorted
        // (all offsets are 0 so far).
        for t in [7, 2, 11, 0] {
            pattern.tracks[t].steps[0].trig = true;
        }
    }

    seq.play();
    let out = seq.tick();
    let order: Vec<u8> = out.iter().map(|e| e.track).collect();
    assert_eq!(order, vec![0, 2, 7, 11]);
    assert!(out.iter().all(|e| e.offset == 0));
}

#[test]
fn positive_micro_delays_within_the_window() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.steps[0].trig = true;
    track.steps[0].set_micro(5);

    seq.play();
    let out = seq.tick(); // window 0
    assert_eq!(out.len(), 1);
    assert_eq!(out.as_slice()[0].offset, 5);
}

#[test]
fn negative_micro_emits_in_the_previous_window() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.steps[4].trig = true;
    track.steps[4].set_micro(-3);

    seq.play();
    for tick in 0..16 {
        let out = seq.tick();
        if tick == 3 {
            assert_eq!(out.len(), 1, "step 4 plays at the end of window 3");
            assert_eq!(out.as_slice()[0].offset, 21); // 24 - 3
        } else {
            assert!(out.is_empty(), "tick {tick}");
        }
    }
}

#[test]
fn micro_setter_clamps() {
    let mut seq = Sequencer::new(1);
    let step = &mut seq.current_pattern_mut().tracks[0].steps[0];
    step.set_micro(100);
    assert_eq!(step.micro(), 23);
    step.set_micro(-100);
    assert_eq!(step.micro(), -23);
}

#[test]
fn negative_micro_on_step_zero_wraps_to_loop_end_and_is_silent_on_pass_one() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(4).unwrap();
    track.steps[0].trig = true;
    track.steps[0].set_micro(-2);

    seq.play();
    let mut fires = Vec::new();
    for tick in 0..16 {
        let out = seq.tick();
        if !out.is_empty() {
            assert_eq!(out.as_slice()[0].offset, 22);
            fires.push(tick);
        }
    }
    // Loop-0 instance would precede the transport start: silent. Each later
    // instance plays at the end of the loop before it (ticks 3, 7, 11, 15).
    assert_eq!(fires, vec![3, 7, 11, 15]);
}

#[test]
fn same_pulse_collision_emits_both_events() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.steps[4].trig = true;
    track.steps[4].set_micro(23);
    track.steps[5].trig = true;
    track.steps[5].set_micro(-1);

    seq.play();
    for _ in 0..4 {
        seq.tick();
    }
    let out = seq.tick(); // window 4
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|e| e.offset == 23 && e.track == 0));
}

#[test]
fn pre_follows_grid_order_not_audible_order() {
    use plock::{Condition, UnitValue};

    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::Percent(UnitValue::ONE);
    track.steps[0].set_micro(23); // audibly late...
    track.steps[1].trig = true;
    track.steps[1].condition = Condition::Pre;
    track.steps[1].set_micro(-5); // ...audibly *before* its upstream step

    seq.play();
    let out = seq.tick(); // window 0 examines step 0, then pulled step 1
    assert_eq!(out.len(), 2, "Pre saw the just-evaluated upstream result");
    let offsets: Vec<u8> = out.iter().map(|e| e.offset).collect();
    assert_eq!(offsets, vec![19, 23]); // sorted: Pre event audibly first
}

#[test]
fn first_on_negative_micro_step_zero_never_fires() {
    use plock::Condition;

    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(4).unwrap();
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::First;
    track.steps[0].set_micro(-1);

    seq.play();
    for _ in 0..32 {
        assert!(seq.tick().is_empty());
    }
}

#[test]
fn ratio_on_negative_micro_step_zero_stays_loop_aligned() {
    use plock::Condition;

    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(4).unwrap();
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::ratio(1, 2).unwrap();
    track.steps[0].set_micro(-1);

    seq.play();
    let mut fires = Vec::new();
    for tick in 0..32 {
        if !seq.tick().is_empty() {
            fires.push(tick);
        }
    }
    // Instances belong to loops 0, 2, 4, ...; loop 0 is the silent pass-one
    // case, loop 2k's event plays at tick 8k - 1.
    assert_eq!(fires, vec![7, 15, 23, 31]);
}

#[test]
fn micro_displacement_does_not_shift_other_tracks_randomness() {
    use plock::{Condition, UnitValue};

    let run = |with_micro: i8| -> Vec<bool> {
        let mut seq = Sequencer::new(0x00C0_FFEE);
        {
            let pattern = seq.current_pattern_mut();
            pattern.tracks[0].steps[0].trig = true;
            pattern.tracks[0].steps[0].condition = Condition::Percent(UnitValue::new(0.5));
            pattern.tracks[5].steps[3].trig = true;
            pattern.tracks[5].steps[3].set_micro(with_micro);
        }
        seq.play();
        (0..128)
            .map(|_| seq.tick().iter().any(|e| e.track == 0))
            .collect()
    };
    assert_eq!(run(0), run(10));
    assert_eq!(run(0), run(-10));
}

#[test]
fn swing_50_is_a_no_op() {
    let run = |set: Option<u8>| -> Vec<plock::TickOutput> {
        let mut seq = Sequencer::new(5);
        let pattern = seq.current_pattern_mut();
        for s in 0..8 {
            pattern.tracks[0].steps[s].trig = true;
        }
        if let Some(p) = set {
            pattern.set_swing(p);
        }
        seq.play();
        (0..32).map(|_| seq.tick()).collect()
    };
    assert_eq!(run(None), run(Some(50)));
}

#[test]
fn swing_delays_odd_steps_only() {
    let mut seq = Sequencer::new(1);
    {
        let pattern = seq.current_pattern_mut();
        for s in 0..16 {
            pattern.tracks[0].steps[s].trig = true;
        }
        pattern.set_swing(75);
    }

    seq.play();
    for tick in 0..32 {
        let out = seq.tick();
        assert_eq!(out.len(), 1);
        let expected = if tick % 2 == 1 { 12 } else { 0 };
        assert_eq!(out.as_slice()[0].offset, expected, "tick {tick}");
    }
}

#[test]
fn swing_80_delays_by_14_and_clamps() {
    let mut seq = Sequencer::new(1);
    {
        let pattern = seq.current_pattern_mut();
        pattern.tracks[0].steps[1].trig = true;
        pattern.set_swing(80);
        assert_eq!(pattern.swing(), 80);
        pattern.set_swing(99);
        assert_eq!(pattern.swing(), 80);
        pattern.set_swing(10);
        assert_eq!(pattern.swing(), 50);
        pattern.set_swing(80);
    }

    seq.play();
    seq.tick();
    let out = seq.tick(); // window 1
    assert_eq!(out.as_slice()[0].offset, 14);
}

#[test]
fn swing_plus_micro_spills_into_the_next_window() {
    let mut seq = Sequencer::new(1);
    {
        let pattern = seq.current_pattern_mut();
        pattern.set_swing(75); // +12 on odd steps
        let track = &mut pattern.tracks[0];
        track.steps[1].trig = true;
        track.steps[1].set_micro(15); // 15 + 12 = 27 -> next window, offset 3
    }

    seq.play();
    assert!(seq.tick().is_empty()); // window 0
    assert!(seq.tick().is_empty()); // window 1: event displaced out
    let out = seq.tick(); // window 2: spilled event arrives
    assert_eq!(out.len(), 1);
    assert_eq!(out.as_slice()[0].offset, 3);
}

#[test]
fn swing_pushes_a_pulled_negative_micro_event_back_into_its_own_window() {
    let mut seq = Sequencer::new(1);
    {
        let pattern = seq.current_pattern_mut();
        pattern.set_swing(75);
        let track = &mut pattern.tracks[0];
        track.steps[1].trig = true;
        track.steps[1].set_micro(-1); // pulled: 24 - 1 + 12 = 35 -> spill
    }

    seq.play();
    assert!(seq.tick().is_empty()); // window 0 examines it, spills
    let out = seq.tick(); // window 1, offset 35 - 24 = 11
    assert_eq!(out.len(), 1);
    assert_eq!(out.as_slice()[0].offset, 11);
}

#[test]
fn swing_parity_is_per_track_index_under_polymeter() {
    let mut seq = Sequencer::new(1);
    {
        let pattern = seq.current_pattern_mut();
        pattern.set_swing(75);
        let track = &mut pattern.tracks[0];
        track.set_length(15).unwrap();
        for s in 0..15 {
            track.steps[s].trig = true;
        }
    }

    seq.play();
    // Across loops of an odd-length track, wall-clock parity flips but
    // step-index parity must not.
    for tick in 0..45 {
        let out = seq.tick();
        let index = tick % 15;
        let expected = if index % 2 == 1 { 12 } else { 0 };
        assert_eq!(out.as_slice()[0].offset, expected, "tick {tick}");
    }
}

#[test]
fn behavior_unchanged_by_pulse_plumbing() {
    // A fixed multi-feature pattern (conditions, locks, polymeter) keeps its
    // step-level behavior: this guards the slice 6 refactor.
    use plock::{Condition, UnitValue};

    let mut seq = Sequencer::new(99);
    {
        let pattern = seq.current_pattern_mut();
        pattern.tracks[0].steps[0].trig = true;
        pattern.tracks[0].steps[8].trig = true;
        pattern
            .set_velocity_lock(0, 8, UnitValue::new(0.4))
            .unwrap();
        pattern.tracks[1].set_length(12).unwrap();
        pattern.tracks[1].steps[0].trig = true;
        pattern.tracks[1].steps[0].condition = Condition::ratio(1, 2).unwrap();
        pattern.tracks[2].steps[3].trig = true;
        pattern.tracks[2].steps[3].condition = Condition::Fill;
    }

    seq.play();
    for tick in 0..96 {
        let f = common::fired(&seq.tick());
        assert_eq!(
            f[0].is_some(),
            tick % 16 == 0 || tick % 16 == 8,
            "t0 {tick}"
        );
        if tick % 16 == 8 {
            assert_eq!(f[0], Some(0.4));
        }
        assert_eq!(f[1].is_some(), tick % 24 == 0, "t1 {tick}");
        assert!(f[2].is_none(), "fill is off");
    }
}
