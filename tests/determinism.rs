//! Slice 4: the determinism contract around probability trigs.

mod common;

use elektronlike::{Condition, Pattern, Sequencer, TickOutput, UnitValue};

/// A pattern with probability trigs on several tracks.
fn probabilistic_pattern() -> Pattern {
    let mut pattern = Pattern::default();
    for (i, track) in pattern.tracks.iter_mut().enumerate().take(4) {
        track.steps[i * 2].trig = true;
        track.steps[i * 2].condition = Condition::Percent(UnitValue::new(0.5));
        track.steps[7].trig = true; // unconditional
    }
    pattern
}

fn run(seed: u64, ticks: usize) -> Vec<TickOutput> {
    let mut seq = Sequencer::new(seed);
    *seq.current_pattern_mut() = probabilistic_pattern();
    seq.play();
    (0..ticks).map(|_| seq.tick()).collect()
}

#[test]
fn same_seed_same_output_over_1000_ticks() {
    assert_eq!(run(42, 1000), run(42, 1000));
}

#[test]
fn different_seeds_diverge() {
    assert_ne!(run(1, 1000), run(2, 1000));
}

#[test]
fn reseeding_replays_a_take() {
    let mut seq = Sequencer::new(9);
    *seq.current_pattern_mut() = probabilistic_pattern();

    seq.seed(123);
    seq.play();
    let first: Vec<TickOutput> = (0..500).map(|_| seq.tick()).collect();

    seq.seed(123);
    seq.play();
    let second: Vec<TickOutput> = (0..500).map(|_| seq.tick()).collect();

    assert_eq!(first, second);
}

#[test]
fn always_steps_do_not_consume_draws() {
    // Two sequencers, same seed. The second has an extra unconditional trig
    // on another track; track 0's random fire pattern must be unaffected.
    let collect_track0 = |with_extra: bool| -> Vec<bool> {
        let mut seq = Sequencer::new(77);
        let pattern = seq.current_pattern_mut();
        pattern.tracks[0].steps[0].trig = true;
        pattern.tracks[0].steps[0].condition = Condition::Percent(UnitValue::new(0.5));
        if with_extra {
            pattern.tracks[5].steps[3].trig = true; // Always condition
        }
        seq.play();
        (0..256)
            .map(|_| seq.tick().iter().any(|e| e.track == 0))
            .collect()
    };
    assert_eq!(collect_track0(false), collect_track0(true));
}
