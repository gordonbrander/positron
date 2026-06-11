//! Slice 11: serde round-trips and deserialization validation.
#![cfg(feature = "serde")]

use plock::{Condition, Pattern, Retrig, RetrigLength, RetrigRate, Track, UnitValue};
use std::num::NonZeroU16;

fn rich_pattern() -> Pattern {
    let mut p = Pattern::default();
    p.set_swing(66);
    p.change_length = Some(NonZeroU16::new(12).unwrap());
    let t = &mut p.tracks[0];
    t.set_length(48).unwrap();
    t.defaults.velocity = UnitValue::new(0.8);
    t.defaults.lanes[3] = UnitValue::new(0.5);
    t.steps[0].trig = true;
    t.steps[0].condition = Condition::Percent(UnitValue::new(0.7));
    t.steps[0].set_micro(-11);
    t.steps[0].locks.velocity = Some(UnitValue::new(0.3));
    t.steps[0].locks.lanes[17] = Some(UnitValue::new(0.9));
    t.steps[5].trig = true;
    t.steps[5].condition = Condition::ratio(3, 8).unwrap();
    t.steps[5].retrig = Some(Retrig::new(RetrigRate::R48, RetrigLength::Infinite, -0.4));
    p.tracks[7].set_length(1).unwrap();
    p
}

/// Deserializing a whole `Pattern` keeps a few ~320 KB temporaries alive at
/// once — more than the default 2 MiB test-thread stack in debug builds.
/// Hosts normally deserialize on the main thread (8 MiB); tests get an
/// explicitly sized one.
fn with_big_stack(f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn pattern_round_trips_exactly() {
    with_big_stack(|| {
        let original = rich_pattern();
        let json = serde_json::to_string(&original).unwrap();
        let back: Pattern = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    });
}

#[test]
fn invalid_track_length_is_rejected() {
    let mut value = serde_json::to_value(Track::default()).unwrap();
    value["length"] = 0.into();
    assert!(serde_json::from_value::<Track>(value.clone()).is_err());
    value["length"] = 200.into();
    assert!(serde_json::from_value::<Track>(value).is_err());
}

#[test]
fn wrong_step_count_is_rejected() {
    let mut value = serde_json::to_value(Track::default()).unwrap();
    let steps = value["steps"].as_array().unwrap()[..100].to_vec();
    value["steps"] = steps.into();
    assert!(serde_json::from_value::<Track>(value).is_err());
}

#[test]
fn out_of_range_scalars_reclamp() {
    with_big_stack(|| {
        let mut value = serde_json::to_value(rich_pattern()).unwrap();
        value["swing"] = 99.into();
        value["tracks"][0]["defaults"]["velocity"] = serde_json::json!(7.5);
        value["tracks"][0]["steps"][0]["micro"] = serde_json::json!(-99);
        let p: Pattern = serde_json::from_value(value).unwrap();
        assert_eq!(p.swing(), 80);
        assert_eq!(p.tracks[0].defaults.velocity, UnitValue::ONE);
        assert_eq!(p.tracks[0].steps[0].micro(), -23);
    });
}
