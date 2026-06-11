//! Slice 9: retrig trains.

mod common;

use plock::{Condition, Retrig, RetrigLength, RetrigRate, Sequencer, TickOutput, UnitValue};

fn offsets(out: &TickOutput, track: u8) -> Vec<u8> {
    out.iter()
        .filter(|e| e.track == track)
        .map(|e| e.offset)
        .collect()
}

#[test]
fn hit_spacing_matches_the_rate_table() {
    for (rate, interval) in [
        (RetrigRate::R4, 96u32),
        (RetrigRate::R6, 64),
        (RetrigRate::R8, 48),
        (RetrigRate::R12, 32),
        (RetrigRate::R16, 24),
        (RetrigRate::R24, 16),
        (RetrigRate::R32, 12),
        (RetrigRate::R48, 8),
        (RetrigRate::R64, 6),
        (RetrigRate::R96, 4),
    ] {
        assert_eq!(u32::from(rate.interval()), interval);

        let mut seq = Sequencer::new(1);
        let track = &mut seq.current_pattern_mut().tracks[0];
        track.set_length(128).unwrap(); // one long pass, no re-trigger
        track.steps[0].trig = true;
        track.steps[0].retrig = Some(Retrig::new(rate, RetrigLength::Infinite, 0.0));

        seq.play();
        let mut pulses = Vec::new();
        for window in 0..8u32 {
            for off in offsets(&seq.tick(), 0) {
                pulses.push(window * 24 + u32::from(off));
            }
        }
        let expected: Vec<u32> = (0..192).step_by(interval as usize).collect();
        assert_eq!(pulses, expected, "rate {rate:?}");
    }
}

#[test]
fn finite_train_hit_count_and_window_spanning() {
    // length 24 pulses at R32 (12): hits at pulses 0, 12, 24 -> 3 hits,
    // the last one in the second window.
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(128).unwrap();
    track.steps[0].trig = true;
    track.steps[0].retrig = Some(Retrig::new(RetrigRate::R32, RetrigLength::pulses(24), 0.0));

    seq.play();
    assert_eq!(offsets(&seq.tick(), 0), vec![0, 12]); // window 0
    assert_eq!(offsets(&seq.tick(), 0), vec![0]); // window 1: pulse 24
    for _ in 2..8 {
        assert!(seq.tick().is_empty()); // train exhausted
    }
}

#[test]
fn single_hit_train_plays_at_base_velocity() {
    // length < interval -> exactly the trig's own hit, ramp ignored.
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(128).unwrap();
    track.defaults.velocity = UnitValue::new(0.6);
    track.steps[0].trig = true;
    track.steps[0].retrig = Some(Retrig::new(RetrigRate::R32, RetrigLength::pulses(8), -1.0));

    seq.play();
    let out = seq.tick();
    assert_eq!(out.len(), 1);
    assert_eq!(out.as_slice()[0].velocity, 0.6);
    assert!(seq.tick().is_empty());
}

#[test]
fn infinite_train_runs_until_the_next_fired_trig() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(8).unwrap();
    track.steps[0].trig = true;
    track.steps[0].retrig = Some(Retrig::new(RetrigRate::R96, RetrigLength::Infinite, 0.0));
    track.steps[4].trig = true; // plain trig cuts the train

    seq.play();
    for window in 0..4 {
        assert_eq!(
            offsets(&seq.tick(), 0),
            vec![0, 4, 8, 12, 16, 20],
            "window {window}"
        );
    }
    // Window 4: the plain trig at pulse 0 truncates everything at >= 0.
    assert_eq!(offsets(&seq.tick(), 0), vec![0]);
    for _ in 5..8 {
        assert!(seq.tick().is_empty(), "train is dead");
    }
}

#[test]
fn replacement_mid_window_truncates_at_the_new_trigs_pulse() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(128).unwrap();
    track.steps[0].trig = true;
    track.steps[0].retrig = Some(Retrig::new(RetrigRate::R48, RetrigLength::Infinite, 0.0));
    track.steps[1].trig = true;
    track.steps[1].set_micro(4); // window 1, pulse 4

    seq.play();
    assert_eq!(offsets(&seq.tick(), 0), vec![0, 8, 16]); // window 0
    // Window 1: old train would hit at 0, 8, 16; only the hit before
    // pulse 4 survives, then the new (plain) trig kills the train.
    assert_eq!(offsets(&seq.tick(), 0), vec![0, 4]);
    assert!(seq.tick().is_empty());
}

#[test]
fn velocity_ramp_interpolates_and_clamps() {
    // 5 hits (R96 x 16 pulses), ramp -1: velocities 1, .75, .5, .25, 0.
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(128).unwrap();
    track.steps[0].trig = true;
    track.steps[0].retrig = Some(Retrig::new(RetrigRate::R96, RetrigLength::pulses(16), -1.0));

    seq.play();
    let out = seq.tick();
    let velocities: Vec<f32> = out.iter().map(|e| e.velocity).collect();
    assert_eq!(velocities, vec![1.0, 0.75, 0.5, 0.25, 0.0]);

    // Upward ramp from 0.5 clamps at 1.0.
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(128).unwrap();
    track.defaults.velocity = UnitValue::new(0.5);
    track.steps[0].trig = true;
    track.steps[0].retrig = Some(Retrig::new(RetrigRate::R96, RetrigLength::pulses(8), 1.0));

    seq.play();
    let out = seq.tick();
    let velocities: Vec<f32> = out.iter().map(|e| e.velocity).collect();
    assert_eq!(velocities, vec![0.5, 1.0, 1.0]);
}

#[test]
fn lanes_ride_unchanged_on_every_hit() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(128).unwrap();
    track.defaults.lanes[2] = UnitValue::new(0.3);
    track.steps[0].trig = true;
    track.steps[0].locks.lanes[7] = Some(UnitValue::new(0.9));
    track.steps[0].retrig = Some(Retrig::new(RetrigRate::R96, RetrigLength::pulses(20), -0.5));

    seq.play();
    let out = seq.tick();
    assert_eq!(out.len(), 6);
    for ev in &out {
        assert_eq!(ev.lanes[2], 0.3);
        assert_eq!(ev.lanes[7], 0.9);
    }
}

#[test]
fn worst_case_stays_within_capacity() {
    // All 16 tracks at the fastest rate: 6 hits per track per window = 96
    // events; must not overflow (debug_assert would abort).
    let mut seq = Sequencer::new(1);
    for track in &mut seq.current_pattern_mut().tracks {
        track.set_length(1).unwrap();
        track.steps[0].trig = true;
        track.steps[0].retrig = Some(Retrig::new(RetrigRate::R96, RetrigLength::Infinite, 0.0));
    }

    seq.play();
    for _ in 0..16 {
        let out = seq.tick();
        assert_eq!(out.len(), 96);
    }
}

#[test]
fn condition_gates_the_whole_train() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(16).unwrap();
    track.steps[0].trig = true;
    track.steps[0].condition = Condition::ratio(2, 2).unwrap(); // loop 1, 3, ...
    track.steps[0].retrig = Some(Retrig::new(RetrigRate::R96, RetrigLength::pulses(8), 0.0));

    seq.play();
    let mut per_loop = Vec::new();
    for _ in 0..4 {
        let mut hits = 0;
        for _ in 0..16 {
            hits += seq.tick().len();
        }
        per_loop.push(hits);
    }
    assert_eq!(per_loop, vec![0, 3, 0, 3]);
}

#[test]
fn probability_gated_retrig_is_deterministic() {
    let run = || -> Vec<usize> {
        let mut seq = Sequencer::new(0xBEEF);
        let track = &mut seq.current_pattern_mut().tracks[0];
        track.set_length(4).unwrap();
        track.steps[0].trig = true;
        track.steps[0].condition = Condition::Percent(UnitValue::new(0.5));
        track.steps[0].retrig = Some(Retrig::new(RetrigRate::R96, RetrigLength::pulses(8), 0.0));
        seq.play();
        (0..256).map(|_| seq.tick().len()).collect()
    };
    let a = run();
    assert_eq!(a, run());
    assert!(a.contains(&3) && a.contains(&0));
}

#[test]
fn stop_and_play_clear_trains() {
    let mut seq = Sequencer::new(1);
    let track = &mut seq.current_pattern_mut().tracks[0];
    track.set_length(128).unwrap();
    track.steps[0].trig = true;
    track.steps[0].retrig = Some(Retrig::new(RetrigRate::R96, RetrigLength::Infinite, 0.0));

    seq.play();
    let first = seq.tick();
    seq.stop();
    assert!(seq.tick().is_empty()); // stopped, and the train is gone

    seq.play();
    let restarted = seq.tick();
    assert_eq!(first, restarted, "fresh start, no leftover train hits");
}
