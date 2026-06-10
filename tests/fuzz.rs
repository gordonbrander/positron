//! Slice 11: property fuzzing — arbitrary public-API call sequences never
//! panic and never emit values outside the unit interval.

mod common;

use elektronlike::{
    Condition, Pattern, PatternId, Retrig, RetrigLength, RetrigRate, Sequencer, UnitValue,
};
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Op {
    Play,
    Stop,
    Reset,
    Tick(u8),
    SetFill(bool),
    Seed(u64),
    SetSwing(u8),
    SetTrig {
        track: u8,
        step: u8,
        on: bool,
    },
    SetLength {
        track: u8,
        len: u8,
    },
    SetMicro {
        track: u8,
        step: u8,
        micro: i8,
    },
    SetVelLock {
        track: u8,
        step: u8,
        v: f32,
    },
    SetLaneLock {
        track: u8,
        step: u8,
        lane: u8,
        v: f32,
    },
    SetCondition {
        track: u8,
        step: u8,
        pick: u8,
        a: u8,
        b: u8,
        p: f32,
    },
    SetRetrig {
        track: u8,
        step: u8,
        rate: u8,
        len: u16,
        ramp: f32,
    },
    Page(u8),
    AddPattern,
    QueuePattern(u8),
    ClearQueue,
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(Op::Play),
        Just(Op::Stop),
        Just(Op::Reset),
        (1u8..32).prop_map(Op::Tick),
        any::<bool>().prop_map(Op::SetFill),
        any::<u64>().prop_map(Op::Seed),
        any::<u8>().prop_map(Op::SetSwing),
        (any::<u8>(), any::<u8>(), any::<bool>()).prop_map(|(track, step, on)| Op::SetTrig {
            track,
            step,
            on
        }),
        (any::<u8>(), any::<u8>()).prop_map(|(track, len)| Op::SetLength { track, len }),
        (any::<u8>(), any::<u8>(), any::<i8>()).prop_map(|(track, step, micro)| Op::SetMicro {
            track,
            step,
            micro
        }),
        (any::<u8>(), any::<u8>(), -10.0f32..10.0).prop_map(|(track, step, v)| Op::SetVelLock {
            track,
            step,
            v
        }),
        (any::<u8>(), any::<u8>(), any::<u8>(), -10.0f32..10.0).prop_map(
            |(track, step, lane, v)| Op::SetLaneLock {
                track,
                step,
                lane,
                v
            }
        ),
        (
            any::<u8>(),
            any::<u8>(),
            any::<u8>(),
            any::<u8>(),
            any::<u8>(),
            -1.0f32..2.0
        )
            .prop_map(|(track, step, pick, a, b, p)| Op::SetCondition {
                track,
                step,
                pick,
                a,
                b,
                p
            }),
        (
            any::<u8>(),
            any::<u8>(),
            any::<u8>(),
            any::<u16>(),
            -3.0f32..3.0
        )
            .prop_map(|(track, step, rate, len, ramp)| Op::SetRetrig {
                track,
                step,
                rate,
                len,
                ramp
            }),
        any::<u8>().prop_map(Op::Page),
        Just(Op::AddPattern),
        any::<u8>().prop_map(Op::QueuePattern),
        Just(Op::ClearQueue),
    ]
}

fn rate(pick: u8) -> RetrigRate {
    use RetrigRate::*;
    [R4, R6, R8, R12, R16, R24, R32, R48, R64, R96][usize::from(pick) % 10]
}

fn condition(pick: u8, a: u8, b: u8, p: f32) -> Condition {
    match pick % 12 {
        0 => Condition::Always,
        1 => Condition::Percent(UnitValue::new(p)),
        2 => Condition::Fill,
        3 => Condition::NotFill,
        4 => Condition::Pre,
        5 => Condition::NotPre,
        6 => Condition::Nei,
        7 => Condition::NotNei,
        8 => Condition::First,
        9 => Condition::NotFirst,
        // Raw literals on purpose: evaluation must clamp, never panic.
        _ => Condition::Ratio { a, b },
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn arbitrary_api_sequences_never_panic_or_escape_the_unit_interval(
        ops in proptest::collection::vec(op_strategy(), 1..120)
    ) {
        let mut seq = Sequencer::new(0xF422);
        for op in ops {
            match op {
                Op::Play => seq.play(),
                Op::Stop => seq.stop(),
                Op::Reset => seq.reset(),
                Op::SetFill(f) => seq.set_fill(f),
                Op::Seed(s) => seq.seed(s),
                Op::SetSwing(s) => seq.current_pattern_mut().set_swing(s),
                Op::Tick(n) => {
                    for _ in 0..n {
                        for ev in &seq.tick() {
                            prop_assert!((0.0..=1.0).contains(&ev.velocity));
                            prop_assert!(ev.lanes.iter().all(|l| (0.0..=1.0).contains(l)));
                            prop_assert!(ev.offset < 24);
                            prop_assert!(usize::from(ev.track) < elektronlike::NUM_TRACKS);
                        }
                    }
                }
                Op::SetTrig { track, step, on } => {
                    let t = &mut seq.current_pattern_mut().tracks[usize::from(track) % 16];
                    t.steps[usize::from(step) % 128].trig = on;
                }
                Op::SetLength { track, len } => {
                    let t = &mut seq.current_pattern_mut().tracks[usize::from(track) % 16];
                    let _ = t.set_length(len); // out-of-range errors are fine
                }
                Op::SetMicro { track, step, micro } => {
                    let t = &mut seq.current_pattern_mut().tracks[usize::from(track) % 16];
                    t.steps[usize::from(step) % 128].set_micro(micro);
                }
                Op::SetVelLock { track, step, v } => {
                    let t = &mut seq.current_pattern_mut().tracks[usize::from(track) % 16];
                    t.steps[usize::from(step) % 128].locks.velocity = Some(UnitValue::new(v));
                }
                Op::SetLaneLock { track, step, lane, v } => {
                    let t = &mut seq.current_pattern_mut().tracks[usize::from(track) % 16];
                    t.steps[usize::from(step) % 128].locks.lanes
                        [usize::from(lane) % elektronlike::NUM_PARAM_LANES] =
                        Some(UnitValue::new(v));
                }
                Op::SetCondition { track, step, pick, a, b, p } => {
                    let t = &mut seq.current_pattern_mut().tracks[usize::from(track) % 16];
                    t.steps[usize::from(step) % 128].condition = condition(pick, a, b, p);
                }
                Op::SetRetrig { track, step, rate: r, len, ramp } => {
                    let t = &mut seq.current_pattern_mut().tracks[usize::from(track) % 16];
                    t.steps[usize::from(step) % 128].retrig =
                        Some(Retrig::new(rate(r), RetrigLength::pulses(len % 512), ramp));
                }
                Op::Page(i) => {
                    // Out-of-range pages are a None, never a panic.
                    let _ = seq.current_pattern().tracks[0].page(usize::from(i));
                }
                Op::AddPattern => {
                    let _ = seq.add_pattern(Pattern::default());
                }
                Op::QueuePattern(id) => {
                    let _ = seq.queue_pattern(PatternId(id)); // may be invalid
                }
                Op::ClearQueue => seq.clear_queue(),
            }
        }
    }
}
