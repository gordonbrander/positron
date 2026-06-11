//! Slice 11: golden-file snapshot guarding against semantic drift.
//!
//! Run with `UPDATE_GOLDEN=1 cargo test --test golden` to regenerate the
//! snapshot after an *intentional* semantic change.

mod common;

use plock::{Condition, Pattern, Retrig, RetrigLength, RetrigRate, Sequencer, UnitValue};
use std::fmt::Write as _;
use std::num::NonZeroU16;

/// A project exercising every feature: conditions, locks, polymeter,
/// micro-timing, swing, retrigs, and a two-pattern chain.
fn build() -> (Sequencer, plock::PatternId) {
    let mut seq = Sequencer::new(0x0E1E_C7E0);
    {
        let a = seq.current_pattern_mut();
        a.set_swing(66);
        a.change_length = Some(NonZeroU16::new(32).unwrap());

        let t0 = &mut a.tracks[0]; // four-on-the-floor with a velocity lock
        for s in [0, 4, 8, 12] {
            t0.steps[s].trig = true;
        }

        let t1 = &mut a.tracks[1]; // polymeter + probability
        t1.set_length(12).unwrap();
        t1.steps[2].trig = true;
        t1.steps[2].condition = Condition::Percent(UnitValue::new(0.5));
        t1.steps[7].trig = true;
        t1.steps[7].condition = Condition::Pre;

        let t2 = &mut a.tracks[2]; // micro-timing, both directions
        t2.steps[3].trig = true;
        t2.steps[3].set_micro(9);
        t2.steps[8].trig = true;
        t2.steps[8].set_micro(-7);

        let t3 = &mut a.tracks[3]; // retrig with ramp, gated by ratio
        t3.steps[0].trig = true;
        t3.steps[0].condition = Condition::ratio(1, 2).unwrap();
        t3.steps[0].retrig = Some(Retrig::new(RetrigRate::R32, RetrigLength::pulses(36), -0.6));

        let t4 = &mut a.tracks[4]; // neighbor of the probability track
        t4.steps[2].trig = true;
        t4.steps[2].condition = Condition::Nei;

        let t5 = &mut a.tracks[5]; // first-loop accent + fill
        t5.steps[0].trig = true;
        t5.steps[0].condition = Condition::First;
        t5.steps[10].trig = true;
        t5.steps[10].condition = Condition::Fill;
    }
    let a = seq.current_pattern_mut();
    a.set_velocity_lock(0, 12, UnitValue::new(0.5)).unwrap();
    a.set_lane_lock(2, 8, 0, UnitValue::new(0.77)).unwrap();
    let mut b = Pattern::default();
    b.tracks[6].set_length(5).unwrap();
    b.tracks[6].steps[0].trig = true;
    b.tracks[6].steps[0].retrig = Some(Retrig::new(RetrigRate::R96, RetrigLength::Infinite, 0.0));
    b.tracks[6].defaults.lanes[9] = UnitValue::new(0.33);
    let b = seq.add_pattern(b).unwrap();

    seq.play();
    (seq, b)
}

#[test]
fn golden_512_ticks() {
    let (mut seq, b) = build();
    let mut rendered = String::new();
    for tick in 0..512 {
        if tick == 40 {
            seq.set_fill(true);
        }
        if tick == 56 {
            seq.set_fill(false);
        }
        if tick == 100 {
            // Mid-run host action, part of the scripted take: queue the
            // chain to pattern B (applies at the next change boundary,
            // master step 128). Queued mid-run — not before the first tick
            // — so pattern A actually plays; master step 0 is itself a
            // boundary, and a pre-queued change would switch immediately.
            seq.queue_pattern(b).unwrap();
        }
        for ev in &seq.tick() {
            // Row: tick,track,offset,velocity,velocity_locked(0/1),
            // locked-mask(hex),lane0..lane63.
            write!(
                rendered,
                "{tick},{},{},{:.4},{},{:016x}",
                ev.track,
                ev.offset,
                ev.velocity,
                u8::from(ev.velocity_locked),
                ev.locked
            )
            .unwrap();
            for lane in ev.lanes {
                write!(rendered, ",{lane:.4}").unwrap();
            }
            rendered.push('\n');
        }
    }

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden_512.txt");
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(path, &rendered).unwrap();
    }
    let expected = std::fs::read_to_string(path)
        .expect("golden snapshot missing; run with UPDATE_GOLDEN=1 once");
    assert_eq!(rendered, expected, "output diverged from the snapshot");
}
