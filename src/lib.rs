//! A headless Elektron-style step sequencer: pure state and behavior, no UI,
//! no audio, no clock.
//!
//! The host owns time and presentation; it drives the sequencer by calling
//! [`Sequencer::tick`] once per step (a sixteenth note at the host's tempo)
//! and scheduling the returned [`Event`]s. See `spec.md` for the full design.
//!
//! ```
//! use plock::Sequencer;
//!
//! let mut seq = Sequencer::new(0xDEAD_BEEF);
//! let track = &mut seq.current_pattern_mut().tracks[0];
//! track.steps[0].trig = true;
//! track.steps[8].trig = true;
//!
//! seq.play();
//! let out = seq.tick(); // step 0
//! assert_eq!(out.len(), 1);
//! assert_eq!(out.as_slice()[0].track, 0);
//! assert_eq!(out.as_slice()[0].velocity, 1.0);
//! ```

mod condition;
mod output;
mod pattern;
mod retrig;
mod rng;
mod sequencer;
mod step;
mod track;
mod unit;

pub use condition::{Condition, RatioError};
pub use output::{Event, MAX_EVENTS_PER_TICK, TickOutput};
pub use pattern::{Pattern, PatternId};
pub use retrig::{Retrig, RetrigLength, RetrigRate};
pub use sequencer::{InvalidPatternId, Sequencer};
pub use step::Step;
pub use track::{LengthError, Params, Track};
pub use unit::UnitValue;

/// Number of tracks in a pattern.
pub const NUM_TRACKS: usize = 16;
/// Steps per page; also the default track length.
pub const STEPS_PER_PAGE: usize = 16;
/// Maximum pages per track.
pub const MAX_PAGES: usize = 8;
/// Maximum track length in steps.
pub const MAX_STEPS: usize = STEPS_PER_PAGE * MAX_PAGES;
/// Pulses per step: the sub-step timing resolution (96 PPQN). One pulse =
/// `step_duration / 24` of host time.
pub const PULSES_PER_STEP: u8 = 24;
/// Number of generic parameter lanes carried per track and per fired event.
pub const NUM_PARAM_LANES: usize = 18;
