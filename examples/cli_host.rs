//! A minimal host proving the headless API is sufficient: it owns the clock
//! (`std::thread::sleep`), maps parameter lanes to "synth parameters", and
//! prints what a sound engine would receive.
//!
//! Run: `cargo run --example cli_host`

use elektronlike::{
    Condition, PULSES_PER_STEP, Retrig, RetrigLength, RetrigRate, Sequencer, UnitValue,
};
use std::time::Duration;

/// The host's side of the shear line: the engine has 18 unlabeled lanes;
/// this table decides what they mean.
const LANE_NAMES: [&str; elektronlike::NUM_PARAM_LANES] = [
    "fm.op1", "fm.op2", "fm.op3", "fm.op4", "fm.op5", "fm.op6", // 0-5: FM operators
    "env.a", "env.d", "env.s", "env.r", // 6-9: ADSR
    "macro1", "macro2", "macro3", "macro4", "macro5", "macro6", "macro7",
    "macro8", // 10-17: macros
];

const BPM: f64 = 132.0;

fn main() {
    let mut seq = Sequencer::new(0xE1EC_7204);

    // A small demo groove on three tracks.
    {
        let pattern = seq.current_pattern_mut();
        pattern.set_swing(62);

        let kick = &mut pattern.tracks[0];
        for s in [0, 4, 8, 12] {
            kick.steps[s].trig = true;
        }
        kick.steps[12].locks.velocity = Some(UnitValue::new(0.6));

        let hat = &mut pattern.tracks[1];
        for s in 0..16 {
            hat.steps[s].trig = true;
            hat.steps[s].condition = if s % 2 == 1 {
                Condition::Percent(UnitValue::new(0.7))
            } else {
                Condition::Always
            };
        }
        hat.defaults.velocity = UnitValue::new(0.4);
        hat.defaults.lanes[7] = UnitValue::new(0.2); // short env.d

        let lead = &mut pattern.tracks[2];
        lead.set_length(12).unwrap(); // polymeter against the others
        lead.steps[0].trig = true;
        lead.steps[0].locks.lanes[0] = Some(UnitValue::new(0.8)); // fm.op1 jump
        lead.steps[6].trig = true;
        lead.steps[6].condition = Condition::ratio(2, 2).unwrap();
        lead.steps[6].retrig = Some(Retrig::new(RetrigRate::R32, RetrigLength::pulses(24), -0.5));
    }

    let step_duration = Duration::from_secs_f64(60.0 / BPM / 4.0);
    let pulse_duration = step_duration / u32::from(PULSES_PER_STEP);

    println!("playing 32 steps at {BPM} bpm; pulse = {pulse_duration:?}\n");
    seq.play();
    for tick in 0..32 {
        let out = seq.tick();
        for ev in &out {
            // A real host would schedule each event `ev.offset` pulses into
            // this step window, sample-accurately. We just annotate it.
            let changed: Vec<String> = ev
                .lanes
                .iter()
                .enumerate()
                .filter(|(_, v)| **v > 0.0)
                .map(|(i, v)| format!("{}={v:.2}", LANE_NAMES[i]))
                .collect();
            println!(
                "step {tick:>3}  track {:>2}  vel {:.2}  +{:>2} pulses  {}",
                ev.track,
                ev.velocity,
                ev.offset,
                changed.join(" ")
            );
        }
        std::thread::sleep(step_duration);
    }
}
