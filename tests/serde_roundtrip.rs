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
    t.steps[5].trig = true;
    t.steps[5].condition = Condition::ratio(3, 8).unwrap();
    t.steps[5].retrig = Some(Retrig::new(RetrigRate::R48, RetrigLength::Infinite, -0.4));
    p.tracks[7].set_length(1).unwrap();
    p.set_velocity_lock(0, 0, UnitValue::new(0.3)).unwrap();
    p.set_lane_lock(0, 0, 63, UnitValue::new(0.9)).unwrap(); // top of the lane range
    p.set_lane_lock(2, 8, 0, UnitValue::new(0.77)).unwrap();
    p
}

#[test]
fn pattern_round_trips_exactly() {
    let original = rich_pattern();
    let json = serde_json::to_string(&original).unwrap();
    let back: Pattern = serde_json::from_str(&json).unwrap();
    assert_eq!(original, back);
}

#[test]
fn set_then_cleared_locks_round_trip_exactly() {
    // Clearing must restore the canonical zero value and free empty slots,
    // or the round-trip (which rebuilds from zeroes) would compare unequal.
    let mut original = rich_pattern();
    original
        .set_lane_lock(5, 7, 10, UnitValue::new(0.4))
        .unwrap();
    original
        .set_lane_lock(5, 9, 10, UnitValue::new(0.6))
        .unwrap();
    original.clear_lane_lock(5, 9, 10); // slot kept, value slot zeroed
    original
        .set_velocity_lock(6, 0, UnitValue::new(0.2))
        .unwrap();
    original.clear_velocity_lock(6, 0); // slot freed

    let json = serde_json::to_string(&original).unwrap();
    let back: Pattern = serde_json::from_str(&json).unwrap();
    assert_eq!(original, back);
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
fn wrong_lane_count_is_rejected() {
    let mut value = serde_json::to_value(Track::default()).unwrap();
    let lanes = value["defaults"]["lanes"].as_array().unwrap()[..10].to_vec();
    value["defaults"]["lanes"] = lanes.into();
    assert!(serde_json::from_value::<Track>(value).is_err());
}

#[test]
fn out_of_range_scalars_reclamp() {
    let mut value = serde_json::to_value(rich_pattern()).unwrap();
    value["swing"] = 99.into();
    value["tracks"][0]["defaults"]["velocity"] = serde_json::json!(7.5);
    value["tracks"][0]["steps"][0]["micro"] = serde_json::json!(-99);
    value["locks"][0]["steps"][0][1] = serde_json::json!(7.5); // lock value
    let p: Pattern = serde_json::from_value(value).unwrap();
    assert_eq!(p.swing(), 80);
    assert_eq!(p.tracks[0].defaults.velocity, UnitValue::ONE);
    assert_eq!(p.tracks[0].steps[0].micro(), -23);
    assert_eq!(p.velocity_lock(0, 0), Some(UnitValue::ONE)); // re-clamped
}

#[test]
fn missing_locks_field_loads_as_empty_pool() {
    // Pre-Slice-12 files have no `locks` key (their per-step locks are
    // silently dropped — documented pre-1.0 format break).
    let mut value = serde_json::to_value(Pattern::default()).unwrap();
    value.as_object_mut().unwrap().remove("locks");
    let p: Pattern = serde_json::from_value(value).unwrap();
    assert_eq!(p.lock_count(), 0);
}

#[test]
fn structurally_broken_lock_pools_are_rejected() {
    let base = serde_json::to_value(rich_pattern()).unwrap();
    let entry = |track: u8, dest: u8, steps: serde_json::Value| serde_json::json!({ "track": track, "dest": dest, "steps": steps });

    // Track out of range.
    let mut v = base.clone();
    v["locks"] = serde_json::json!([entry(16, 0, serde_json::json!([[0, 0.5]]))]);
    assert!(serde_json::from_value::<Pattern>(v).is_err());

    // Destination neither a lane nor the velocity sentinel (255).
    let mut v = base.clone();
    v["locks"] = serde_json::json!([entry(0, 70, serde_json::json!([[0, 0.5]]))]);
    assert!(serde_json::from_value::<Pattern>(v).is_err());

    // Step index out of range.
    let mut v = base.clone();
    v["locks"] = serde_json::json!([entry(0, 0, serde_json::json!([[128, 0.5]]))]);
    assert!(serde_json::from_value::<Pattern>(v).is_err());

    // Duplicate step within one entry.
    let mut v = base.clone();
    v["locks"] = serde_json::json!([entry(0, 0, serde_json::json!([[3, 0.5], [3, 0.6]]))]);
    assert!(serde_json::from_value::<Pattern>(v).is_err());

    // Empty entry (a freed slot must be removed, not serialized).
    let mut v = base.clone();
    v["locks"] = serde_json::json!([entry(0, 0, serde_json::json!([]))]);
    assert!(serde_json::from_value::<Pattern>(v).is_err());

    // Duplicate (track, dest) across the pool.
    let mut v = base.clone();
    v["locks"] = serde_json::json!([
        entry(0, 0, serde_json::json!([[0, 0.5]])),
        entry(0, 0, serde_json::json!([[1, 0.5]])),
    ]);
    assert!(serde_json::from_value::<Pattern>(v).is_err());

    // Pool over capacity: 81 distinct destinations.
    let mut v = base;
    let over: Vec<serde_json::Value> = (0..81)
        .map(|i| entry(i % 16, i / 16, serde_json::json!([[0, 0.5]])))
        .collect();
    v["locks"] = serde_json::json!(over);
    assert!(serde_json::from_value::<Pattern>(v).is_err());
}
