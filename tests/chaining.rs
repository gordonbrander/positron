//! Slice 10: pattern chaining and quantized changes.

mod common;

use common::fired;
use plock::{Condition, Pattern, PatternId, Sequencer};
use std::num::NonZeroU16;

/// Pattern with a single trig on `track` 0 at `step`.
fn one_trig(step: usize) -> Pattern {
    let mut p = Pattern::default();
    p.tracks[0].steps[step].trig = true;
    p
}

#[test]
fn change_lands_exactly_on_the_boundary() {
    let mut seq = Sequencer::new(1);
    *seq.current_pattern_mut() = one_trig(0);
    let b = seq.add_pattern(one_trig(1)).unwrap();

    seq.play();
    for _ in 0..5 {
        seq.tick();
    }
    seq.queue_pattern(b).unwrap(); // queued mid-pattern: waits
    assert_eq!(seq.pending(), Some(b));

    let mut fires = Vec::new();
    for tick in 5..40 {
        if fired(&seq.tick())[0].is_some() {
            fires.push(tick);
        }
    }
    // A would fire at 16, 32; B (step 1) fires at 17, 33 after the switch
    // at tick 16.
    assert_eq!(fires, vec![17, 33]);
    assert_eq!(seq.pending(), None);
    assert_eq!(seq.current_pattern_id(), b);
}

#[test]
fn default_master_length_is_the_longest_track_under_polymeter() {
    let mut seq = Sequencer::new(1);
    {
        let a = seq.current_pattern_mut();
        a.tracks[0].steps[0].trig = true;
        a.tracks[1].set_length(12).unwrap();
        assert_eq!(a.master_length(), 16);
    }
    let b = seq.add_pattern(one_trig(0)).unwrap();

    seq.play();
    seq.tick(); // tick 0 (A fires)
    seq.queue_pattern(b).unwrap();
    // Boundary is at 16 (longest track), not 12.
    for tick in 1..16 {
        assert_eq!(seq.current_pattern_id(), PatternId(0), "tick {tick}");
        seq.tick();
    }
    seq.tick(); // tick 16: switch happens at its start
    assert_eq!(seq.current_pattern_id(), b);
}

#[test]
fn explicit_change_length_switches_mid_pattern() {
    let mut seq = Sequencer::new(1);
    {
        let a = seq.current_pattern_mut();
        a.tracks[0].steps[0].trig = true;
        a.change_length = Some(NonZeroU16::new(4).unwrap());
    }
    let b = seq.add_pattern(one_trig(0)).unwrap();

    seq.play();
    seq.tick(); // tick 0
    seq.queue_pattern(b).unwrap();
    seq.tick(); // 1
    seq.tick(); // 2
    seq.tick(); // 3
    assert_eq!(seq.current_pattern_id(), PatternId(0));
    let out = seq.tick(); // tick 4: boundary -> B starts at its step 0
    assert_eq!(seq.current_pattern_id(), b);
    assert!(fired(&out)[0].is_some(), "B's step 0 plays immediately");
}

#[test]
fn chain_plays_in_order_and_the_last_pattern_loops() {
    let mut seq = Sequencer::new(1);
    *seq.current_pattern_mut() = one_trig(0);
    let b = seq.add_pattern(one_trig(1)).unwrap();
    let c = seq.add_pattern(one_trig(2)).unwrap();
    seq.queue_chain(&[b, c]).unwrap();

    seq.play();
    let mut fires = Vec::new();
    for tick in 0..64 {
        if fired(&seq.tick())[0].is_some() {
            fires.push(tick);
        }
    }
    // master_step 0 is a boundary, so B is consumed on the very first tick
    // (A never plays — queue after play() to hear the current pattern
    // first). B runs ticks 0..16 firing at 1; C from tick 16, firing at
    // 18, 34, 50, and loops as the chain's last pattern.
    assert_eq!(fires, vec![1, 18, 34, 50]);
    assert_eq!(seq.current_pattern_id(), c);
}

#[test]
fn queued_change_while_stopped_applies_on_the_first_tick() {
    let mut seq = Sequencer::new(1);
    *seq.current_pattern_mut() = one_trig(0);
    let b = seq.add_pattern(one_trig(0)).unwrap();
    seq.queue_pattern(b).unwrap();

    seq.play(); // play() must NOT clear the queue
    assert_eq!(seq.pending(), Some(b));
    let out = seq.tick();
    assert_eq!(seq.current_pattern_id(), b);
    assert!(fired(&out)[0].is_some(), "B's step 0 plays on tick 0");
}

#[test]
fn first_condition_fires_again_after_a_switch() {
    let mut seq = Sequencer::new(1);
    seq.current_pattern_mut().tracks[0].steps[0].trig = true;
    let mut b = Pattern::default();
    b.tracks[0].steps[0].trig = true;
    b.tracks[0].steps[0].condition = Condition::First;
    let b = seq.add_pattern(b).unwrap();

    seq.play();
    seq.tick(); // A tick 0
    seq.queue_pattern(b).unwrap();
    let mut fires = Vec::new();
    for tick in 1..64 {
        if fired(&seq.tick())[0].is_some() {
            fires.push(tick);
        }
    }
    // B starts at 16; its First trig fires once (tick 16) and never again.
    assert_eq!(fires, vec![16]);
}

#[test]
fn clear_queue_cancels_a_pending_change() {
    let mut seq = Sequencer::new(1);
    let b = seq.add_pattern(Pattern::default()).unwrap();

    seq.play();
    seq.tick();
    seq.queue_pattern(b).unwrap();
    seq.clear_queue();
    for _ in 1..40 {
        seq.tick();
    }
    assert_eq!(seq.current_pattern_id(), PatternId(0));
}

#[test]
fn out_of_range_ids_rejected_at_queue_time() {
    let mut seq = Sequencer::new(1);
    let bogus = PatternId(7);
    assert!(seq.queue_pattern(bogus).is_err());
    assert!(seq.queue_chain(&[PatternId(0), bogus]).is_err());
    assert_eq!(seq.pending(), None, "rejected chain queued nothing");

    assert!(seq.pattern(bogus).is_none());
    assert!(seq.pattern_mut(bogus).is_none());
    assert!(seq.pattern(PatternId(0)).is_some());
}

#[test]
fn switch_drops_in_flight_sub_step_events() {
    use plock::{Retrig, RetrigLength, RetrigRate};

    let mut seq = Sequencer::new(1);
    {
        let a = seq.current_pattern_mut();
        a.change_length = Some(NonZeroU16::new(2).unwrap());
        a.tracks[0].steps[1].trig = true;
        a.tracks[0].steps[1].retrig =
            Some(Retrig::new(RetrigRate::R96, RetrigLength::Infinite, 0.0));
    }
    let b = seq.add_pattern(Pattern::default()).unwrap();

    seq.play();
    seq.tick(); // tick 0: nothing
    seq.queue_pattern(b).unwrap();
    let out = seq.tick(); // tick 1: trig + train hits
    assert_eq!(out.len(), 6);
    // Tick 2 is a boundary: the switch clears the train before emission.
    assert!(seq.tick().is_empty());
    assert_eq!(seq.current_pattern_id(), b);
}

#[test]
fn next_boundary_uses_the_new_patterns_change_length() {
    let mut seq = Sequencer::new(1);
    *seq.current_pattern_mut() = one_trig(0); // A: master length 16
    let mut pb = Pattern::default();
    pb.change_length = Some(NonZeroU16::new(4).unwrap());
    let b = seq.add_pattern(pb).unwrap();
    let c = seq.add_pattern(one_trig(0)).unwrap();

    seq.play();
    seq.tick(); // tick 0 (A)
    seq.queue_chain(&[b, c]).unwrap();
    // A -> B at 16; B -> C four steps later, at 20.
    for tick in 1..20 {
        seq.tick();
        let expect = if tick <= 15 { PatternId(0) } else { b };
        assert_eq!(seq.current_pattern_id(), expect, "tick {tick}");
    }
    seq.tick(); // tick 20
    assert_eq!(seq.current_pattern_id(), c);
}
