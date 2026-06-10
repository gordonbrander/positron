# elektronlike — a headless Elektron-style step sequencer in Rust

## Overview

`elektronlike` is a Rust library crate implementing the data model and behavior of an
Elektron-style step sequencer (Digitakt/Octatrack lineage): tracks of trigs with
parameter locks, probability, trig conditions, micro-timing, retrigs, swing, and
quantized pattern chaining. It is **headless**: pure state and
behavior, no UI, no audio, no MIDI, no clock, no threads, no I/O. A host (TUI, GUI,
audio engine, hardware bridge, test harness) owns time and presentation and drives the
sequencer by calling methods on it.

## Goals

1. **Headless / shearable.** The crate exposes a state machine only. All UI concerns
   (page navigation views, encoder mappings, LED state) live in the host. The API must
   be sufficient for a UI to render everything (full read access to pattern state) and
   mutate everything (full write access), but the engine never pushes to a UI.
2. **Deterministic.** Given the same pattern, seed, and sequence of calls, output is
   bit-identical. Randomness (probability trigs) comes from an internal seedable PRNG,
   never from global state or system entropy. This makes behavior unit-testable and
   replayable.
3. **Realtime-friendly.** `tick()` performs no heap allocation and no locking. State is
   fixed-size (`[Track; 16]`, fixed-capacity step arrays). The crate is suitable for
   calling from an audio callback.
4. **Live-editable.** All pattern mutation is legal while the transport is running, as
   on the hardware. Edits take effect the next time the affected step is evaluated.
5. **Small, dependency-light core.** Ideally zero required dependencies (a tiny
   embedded PRNG implemented in-crate or via `rand_core`-compatible crate). Optional
   `serde` support behind a feature flag for hosts that want pattern persistence.

## Non-goals (out of scope for this spec)

- Sound generation, MIDI I/O, tempo/clock generation (host supplies the clock).
- Per-track scale/time multipliers (2×, 3/2×, …).
- Song mode (arrangement rows with repeats/mutes). Pattern *chaining* is in scope;
  song arrangement is not.
- Inter-step interpolation of parameter-lane values (modulation sequencing). Lanes
  are event-scoped: values ride on fired trigs only.
- Live recording modes.
- `no_std` support (keep it in mind structurally, but don't commit to it).

These are all plausible future extensions; the data model should not actively prevent
them, but no code is written for them now.

## Functional requirements

| Feature | Requirement |
|---|---|
| Tracks | Exactly 16 per sequencer |
| Steps | 16 steps per page, 1–8 pages per track → track length 1–128 steps |
| Track length | Independent per track (polymeter); default 16 |
| Parameter lanes | 18 generic, unlabeled 0..1 values per track (defaults) and per fired event (resolved); the host decides what they mean (e.g. lanes 0–5 → FM operators, 6–9 → ADSR, 10–17 → macros) and scales 0..1 to real units |
| Parameter locks | Per-step override of track parameters: velocity and any of the 18 lanes |
| Probability trigs | Per-step chance 0–100% that an enabled trig fires |
| Conditional trigs | Elektron-style conditions: `FILL`, `!FILL`, `PRE`, `!PRE`, `NEI`, `!NEI`, `1ST`, `!1ST`, `A:B` ratios |
| Fill mode | Host-controlled flag (`set_fill`) read by the `FILL`/`!FILL` conditions; momentary/latched/one-shot press behavior is host policy |
| Micro-timing | Per-step nudge of −23..=+23 pulses (1 step = 24 pulses; 96 PPQN grid) |
| Retrigs | Per-step retrig train: rate, length, velocity ramp |
| Swing | Per-pattern amount 50–80%, delays odd-indexed steps |
| Pattern chaining | Multiple patterns; queued changes quantized to a per-pattern change length |
| Output | Per step window: timed events `{track, velocity 0.0..=1.0, pulse offset}` |

## Architecture

### Layering

```
┌─────────────────────────────────────────────┐
│ Host (UI, audio engine, MIDI bridge, tests) │
│   owns: clock, tempo, rendering, input      │
└───────────────┬─────────────────────────────┘
                │ tick() / play() / stop() / set_fill() / queue_pattern() / mutation
                ▼
┌─────────────────────────────────────────────┐
│ Sequencer  (transport + playback state)     │
│   playheads, loop counters, PRE/NEI state,  │
│   PRNG, fill flag, change queue, retrig     │
│   trains, pending events                    │
├─────────────────────────────────────────────┤
│ Patterns  (pure data: what to play)         │
│   N × Pattern { swing, change_length,       │
│     16 × Track { length, default params,    │
│                  steps: [Step; 128] } }     │
└─────────────────────────────────────────────┘
```

`Pattern` is plain data — `Clone`, `PartialEq`, (optionally) `Serialize`. It contains
nothing about playback position. `Sequencer` wraps a set of `Pattern`s plus all
runtime state.
This split is what makes the crate shearable: a UI can hold, diff, copy, and persist
`Pattern` values without touching playback.

### Time model

Two units of time:

- **Step** — a sixteenth note. The host calls `Sequencer::tick()` once per step. The
  sequencer never sees milliseconds, BPM, or a wall clock.
- **Pulse** — 1/24 of a step (96 PPQN). All sub-step timing — micro-timing, swing,
  retrig intervals — is expressed in whole pulses. The host converts:
  `pulse_duration = step_duration / 24`. This grid matches Elektron hardware, whose
  micro-timing moves trigs in 1/384-note increments (a sixteenth = 24/384).

Each `tick()` covers the **window** `[step, step + 1)` and returns every event
scheduled inside that window, stamped with its pulse offset from the window start.
The host turns offsets into sample-accurate scheduling (or ignores them):

```rust
pub const PULSES_PER_STEP: u8 = 24;
pub const NUM_PARAM_LANES: usize = 18;

pub struct Event {
    pub track: u8,     // 0..16
    pub velocity: f32, // 0.0..=1.0
    pub offset: u8,    // pulses after window start, 0..24
    /// Resolved parameter-lane values: per lane, the step's lock if set,
    /// else the track default. See "Parameter lanes".
    pub lanes: [f32; NUM_PARAM_LANES], // each 0.0..=1.0
}

/// Fixed-capacity event list (capacity 160 — provably sufficient, see
/// "Timing semantics"). No heap allocation in tick().
pub struct TickOutput { /* push/iter over [Event; 160] + len */ }
```

`tick()` — normative phase order, **evaluate then advance**:

1. If stopped, return an empty output; nothing advances.
2. If `master_step` is on a change boundary, consume at most one queued pattern
   change and perform the switch resets (see Timing semantics).
3. Consume pending events that spilled into this window from the previous one.
4. For each track in order 0→15, with its playhead as the window index `W`: examine
   the steps that schedule events into this window (step `W`, plus step `W + 1` when
   its negative micro-timing pulls it in) and emit retrig-train hits that fall
   inside the window.
5. Sort events by `(offset, track)` — stable, preserving push order (pending, step
   `W`, step `W + 1`, train hits) for same-pulse ties.
6. Advance every track's playhead by one step, wrapping at its own length
   (polymeter) and incrementing its loop count on wrap; increment `master_step`.

Convention: `play()` sets every playhead to 0, so the first `tick()` after `play()`
evaluates step 0; each subsequent tick evaluates the next step.

Staging note: `Event`, `TickOutput`, and the constants are defined in Slice 1 in
their final shape; through Slice 6 every event has `offset == 0` and at most one
event per track exists per window. Slice 6 adds the pulse helpers, pending-buffer
plumbing, and ordering guarantees before any timing feature lands.

### Data model

```rust
pub const NUM_TRACKS: usize = 16;
pub const STEPS_PER_PAGE: usize = 16;
pub const MAX_PAGES: usize = 8;
pub const MAX_STEPS: usize = STEPS_PER_PAGE * MAX_PAGES; // 128

/// A value clamped to 0.0..=1.0. Constructors clamp; stored value is always valid.
pub struct UnitValue(f32);

/// Index into the sequencer's pattern list.
pub struct PatternId(pub u8);

pub struct Pattern {
    pub tracks: [Track; NUM_TRACKS],
    /// Swing amount, percent 50..=80. 50 = straight. Setter clamps. Default 50.
    swing: u8,
    /// Pattern-change quantization in steps. None = master length
    /// (the longest track length in this pattern).
    pub change_length: Option<NonZeroU16>,
}

pub struct Track {
    /// Active length in steps, 1..=128. Setter clamps. Default 16.
    length: u8,
    /// Default parameters used when a step has no lock.
    pub defaults: Params,
    pub steps: [Step; MAX_STEPS],
}

/// Track-level parameters. Velocity is distinguished — it gates output and
/// drives the retrig velocity ramp. The lanes are generic, unlabeled 0..1
/// values: the engine never knows what they control; the host owns the
/// mapping (e.g. FM operator levels, ADSR times, macros) and any scaling
/// from 0..1 to real units. "Adding a parameter" = picking a lane index.
pub struct Params {
    pub velocity: UnitValue,                 // default 1.0
    pub lanes: [UnitValue; NUM_PARAM_LANES], // defaults 0.0
}

pub struct Step {
    /// Is there a trig on this step at all?
    pub trig: bool,
    /// Per-step parameter overrides ("p-locks"). None = use track default.
    pub locks: ParamLocks,
    /// When the trig fires. Defaults to Condition::Always.
    pub condition: Condition,
    /// Micro-timing nudge in pulses, −23..=+23. Setter clamps. Default 0.
    micro: i8,
    /// Retrig: rapid repeats of this trig. None = single hit.
    pub retrig: Option<Retrig>,
}

pub struct ParamLocks {
    pub velocity: Option<UnitValue>,
    pub lanes: [Option<UnitValue>; NUM_PARAM_LANES],
}

pub struct Retrig {
    /// Interval between hits (see rate table in "Timing semantics").
    pub rate: RetrigRate,
    /// How long the train runs. Infinite = until the next fired trig on the track.
    pub length: RetrigLength,
    /// Velocity ramp across the train: last hit's velocity =
    /// clamp(first + vel_ramp, 0, 1). 0.0 = flat. Setter clamps to −1.0..=1.0.
    vel_ramp: f32,
}

/// Note-value hit rates representable as whole pulses on the 96 PPQN grid.
pub enum RetrigRate { R4, R6, R8, R12, R16, R24, R32, R48, R64, R96 } // 1/4 .. 1/96

pub enum RetrigLength { Pulses(NonZeroU16), Infinite }

pub enum Condition {
    /// No condition: always fires. Does NOT update PRE/NEI state.
    Always,
    /// Fires with the given probability. (On Elektron hardware, probability
    /// and logical conditions share one parameter slot — they are mutually
    /// exclusive per step, which is why this is one enum, not two fields.)
    Percent(UnitValue),
    /// Fires iff fill mode is active (or not, for the negated form).
    Fill,
    NotFill,
    /// Fires iff the most recently evaluated *conditional* step on the SAME
    /// track passed (PRE) / failed (NotPre).
    Pre,
    NotPre,
    /// Like Pre/NotPre but reads the neighbor track's state (track N-1).
    /// On track 0, Nei evaluates false (and NotNei true).
    Nei,
    NotNei,
    /// Fires only on the track's first loop after transport start.
    First,
    NotFirst,
    /// Fires on loop `a` of every `b`-loop cycle, 1 <= a <= b <= 8.
    /// e.g. Ratio { a: 1, b: 4 } fires on loops 0, 4, 8, ... (0-indexed).
    Ratio { a: u8, b: u8 },
}
```

### Runtime state (inside `Sequencer`, not part of `Pattern`)

```rust
pub struct Sequencer {
    patterns: Vec<Pattern>,      // >= 1; PatternId indexes into this. Edit-time alloc only.
    current: PatternId,
    queue: VecDeque<PatternId>,  // pending pattern changes (edit-time alloc only)
    master_step: u32,            // steps since the current pattern started
    transport: Transport,        // Stopped | Playing
    fill: bool,                  // host-controlled fill mode flag
    rng: Pcg32,                  // seedable, owned, deterministic
    track_state: [TrackState; NUM_TRACKS],
}

struct TrackState {
    playhead: u8,        // 0..track.length; next step to evaluate
    loop_count: u32,     // completed loops since play(), for 1ST and A:B
    last_cond: bool,     // result of most recent conditional evaluation, for PRE/NEI
    train: Option<ActiveTrain>,        // in-flight retrig train
    pending: PendingEvents,            // fixed-cap buffer for events that spill
                                       // past the current window (swing/micro/retrig)
}
```

### Condition semantics (normative)

A step **fires** iff `step.trig` is true AND its condition **passes**. Evaluation per
tick, per track, in track order 0→15 (track order matters only for `Nei`, which must
read the neighbor's result *from the same tick* when the neighbor's step was evaluated
this tick — matching hardware, where neighbor state is the most recent evaluation,
which on a shared step grid is usually the same tick).

- `Always` — passes. **Does not update** `last_cond` (matches hardware: PRE refers to
  the previous *conditional* trig, unconditioned trigs are transparent to it).
- `Percent(p)` — draw `x` uniform in `[0,1)` from the sequencer PRNG; passes iff
  `x < p`. Updates `last_cond`. A draw happens only when an enabled trig with a
  `Percent` condition is evaluated (so edits don't desync replay of untouched tracks
  any more than necessary).
- `Fill` / `NotFill` — passes iff `fill` flag is set / unset. Updates `last_cond`.
- `Pre` / `NotPre` — passes iff `last_cond` of the same track is true / false. Does
  **not** update `last_cond` (chains of PRE all follow the same upstream condition).
- `Nei` / `NotNei` — same as Pre/NotPre but reads track `N-1`'s `last_cond`. Track 0:
  `Nei` fails, `NotNei` passes. Does not update `last_cond`.
- `First` / `NotFirst` — passes iff `loop_count == 0` / `> 0`. Updates `last_cond`.
- `Ratio { a, b }` — passes iff `loop_count % b == a - 1`. Updates `last_cond`.
  The validated constructor `Condition::ratio(a, b)` enforces `1 <= a <= b <= 8`;
  because the variant's fields stay public for ergonomic literals, evaluation is
  defensive — `b` is clamped to `1..=8` and `a` to `1..=b` before use, so no input
  can panic.

Initial `last_cond` after `play()` is `false`.

### Timing semantics (normative)

**Scheduling.** A trig on step `S` with micro-timing `m` and swing delay `w` is
scheduled at pulse `(S * 24 + m + w) mod (track_length * 24)` on the track's cyclic
pulse timeline. Events are emitted in whichever window contains their scheduled
pulse: `m < 0` places the event near the end of window `S − 1` (wrapping to the end
of the track's loop for step 0). Whenever a computed in-window offset is ≥ 24 (e.g.
`m + w >= 24`, or a pulled-in negative-micro step pushed back by swing,
`24 + m + w >= 24`), the event routes through the per-track **pending buffer** into
the next window at `offset − 24`. The maximum combined displacement is 37 pulses
(`23 + 14`), so events spill **at most one window**; the pending buffer holds only
such displaced one-shot step events — at most 2 per window (capacity 4, with a debug
assertion). Retrig trains are never buffered ahead; they are generative runtime
state emitting hits on demand.

**Examination time.** Each step is *examined* (trig present? condition passes?
probability drawn?) exactly once per track loop, in the window where its event lands
or originates: in window `W`, each track examines step `W` (if its `micro >= 0`) and
then step `W + 1` (if its `micro < 0`). Tracks are examined in order 0→15, so
`PRE`/`NEI` follow **grid order**, not audible order — a `PRE` step whose upstream
neighbor is delayed by `micro = +23` still reads that neighbor's already-evaluated
result. A negative-micro trig on step 0 is examined in the *last* window of the
track's loop; on the very first loop after `play()` that window hasn't occurred, so
the trig is silent on pass one (matches hardware; no special case is needed — the
window simply never exists). When the examined index wraps this way, the event
belongs to the *next* loop of the track, so loop-scoped conditions (`First`,
`Ratio`) evaluate against `loop_count + 1`. Documented consequence: `First` on a
negative-micro step 0 never fires — its only loop-0 instance is the silent one.

Two events on the same track may land on the same pulse (e.g. step `S` at `+23` and
step `S + 1` at `−1`); both are emitted and hosts treat them as simultaneous hits.

**Swing.** `Pattern::swing` is a percent in `50..=80`; 50 = straight. Odd-indexed
steps (1, 3, 5, … by index within the track) are delayed by
`(48 * swing + 50) / 100 − 24` pulses (integer round-half-up): 0 at 50%, 8 at 66%
(triplet feel), 12 at 75%, 14 at 80% — quantized to the pulse grid. Swing is per-pattern and applies to all
tracks; under polymeter, parity is the step's index within its own track, by
definition. Swing composes additively with micro-timing.

**Retrigs.** When a trig with `retrig: Some(r)` fires, it starts a **train**: hits
every `interval(r.rate)` pulses starting at the trig's scheduled pulse, lasting
`r.length` pulses (`Infinite` = until replaced). The trig emits exactly **one**
event at its scheduled pulse — that event *is* train hit `k = 0`; there is no
double emission. Hit `k` of an `n`-hit train has velocity
`clamp(v0 + r.vel_ramp * k / (n − 1), 0, 1)`, where `v0` is the trig's resolved
velocity (p-lock or track default); a single-hit train (`n = 1`) plays at `v0`, and
`Infinite` trains ignore `vel_ramp`. The trig's resolved lane values ride unchanged
on every hit of the train. The trig's condition gates the entire train (evaluated
once, at examination). A track has at most one active train: any newly fired trig
on the track — retrigging or not — replaces it, truncating the old train's hits at
pulses at or after the new trig's scheduled pulse. `stop()`, `reset()`, `play()`,
and pattern changes clear trains and pending events.

| Rate | Note value | Pulse interval |
|---|---|---|
| `R4`  | 1/4  | 96 |
| `R6`  | 1/6  | 64 |
| `R8`  | 1/8  | 48 |
| `R12` | 1/12 | 32 |
| `R16` | 1/16 | 24 |
| `R24` | 1/24 | 16 |
| `R32` | 1/32 | 12 |
| `R48` | 1/48 | 8  |
| `R64` | 1/64 | 6  |
| `R96` | 1/96 | 4  |

(Hardware also offers 1/80, which is not a whole number of pulses at 96 PPQN; the
nearest representable rate is `R96`. Documented divergence.)

**Parameter lanes.** Each fired event carries the track's `NUM_PARAM_LANES` lane
values, resolved at examination time: per lane, the step's lock if set, else the
track default. Lanes are **event-scoped** — values ride only on fired trigs, so a
lane lock on a probability/conditional trig lands only when that trig actually
fires, and non-fired steps emit nothing. The engine never interprets lane values;
mapping lanes to synth parameters (and scaling 0..1 to Hz/seconds/ratios) is host
territory, on the UI side of the shear line. Memory note: per-step lane locks cost
a few hundred KB per pattern at 18 lanes — fine for desktop; a bitmask + packed
representation is a possible future compression that would not change the API.

**Output capacity.** Per track per window: at most 1 spilled event arrives from the
previous window (only one of any two adjacent steps is odd-indexed, and only swing
on odd steps can displace an offset past the window), plus at most 2 step events,
plus train hits at the minimum interval of 4 pulses — even with a mid-window train
replacement, trigs and hits interleave to at most 8 events (e.g. 6 old-train hits
before a pulse-23 trig collision). 9 × 16 tracks = 144; `TickOutput`'s capacity of
160 therefore cannot overflow. This is asserted in debug builds, not handled at
runtime.

**Pattern changes.** `queue_pattern(id)` appends to the change queue. A **change
boundary** occurs whenever `master_step % change_length == 0`, where `master_step`
counts steps since the current pattern started and `change_length` defaults to the
pattern's master length (its longest track length). At each boundary, the queue head
(if any) becomes the current pattern: playheads, loop counts, `last_cond`, retrig
trains, pending events, and `master_step` all reset — so `1ST` fires again, as on
hardware — while the PRNG state and the fill flag carry over (not-yet-emitted
pending events are dropped by the switch; already-emitted events stand). Because
`master_step == 0` is a boundary and `play()` does not clear the queue, a change
queued while stopped applies on the very first tick after `play()`. At most one
queue entry is consumed per boundary. An empty queue means the current pattern
keeps looping. Chaining = queueing several ids; the last pattern
in a chain loops. To loop a whole chain, the host re-queues it (deliberate
simplification; a `loop_chain` flag is a possible future addition).

### Transport semantics

- `play()` — resets all playheads to step 0 (pending first tick), zeroes loop counts
  and `master_step`, clears `last_cond`, retrig trains, and pending events. Calling
  it while already playing restarts from step 0. Does **not** reseed the PRNG and
  does not clear the pattern-change queue. (Note: `last_cond = false` means `!PRE`
  initially passes.)
- `stop()` — halts; `tick()` while stopped returns an empty output and advances
  nothing. Clears retrig trains and pending events.
- `reset()` — like the position-reset part of `play()` but keeps current transport
  state (useful for host-side song-position-pointer handling).
- `set_fill(bool)` / `fill()` — host toggles fill mode at any time; read at each
  examination. This is **the** fill trigger: momentary (hold), latched (toggle), or
  one-shot (arm for a single loop) press behaviors are host policy layered on this
  flag — the engine deliberately doesn't pick one.
- `seed(u64)` — reseeds the PRNG (e.g. for reproducible takes).
- `queue_pattern(PatternId)` / `queue_chain(&[PatternId])` / `clear_queue()` /
  `pending() -> Option<PatternId>` / `current_pattern_id()` — pattern-change queue;
  changes apply at the next change boundary (see Timing semantics).

### Public API sketch

```rust
let mut seq = Sequencer::new(0xDEADBEEF);

// Pattern editing (all valid while playing)
let t = &mut seq.current_pattern_mut().tracks[0];
t.set_length(64).unwrap();                 // 4 pages
t.defaults.velocity = UnitValue::new(0.8);
t.steps[0].trig = true;
t.steps[4].trig = true;
t.steps[4].locks.velocity = Some(UnitValue::new(0.3));   // p-lock
t.steps[8].trig = true;
t.steps[8].condition = Condition::Percent(UnitValue::new(0.5));
t.steps[12].trig = true;
t.steps[12].condition = Condition::Ratio { a: 1, b: 2 }; // every other loop
t.steps[4].set_micro(-3);                                // 3 pulses early
t.steps[8].retrig = Some(Retrig::new(RetrigRate::R32, RetrigLength::pulses(24), -0.5));

// Pattern-level feel & chaining
seq.current_pattern_mut().set_swing(62);
let b = seq.add_pattern(Pattern::default());             // -> PatternId
seq.queue_pattern(b);                                    // switches at next boundary

// Page helpers for UI hosts (pure conveniences over `length`/`steps`)
let t = &seq.current_pattern().tracks[0];
assert_eq!(t.page_count(), 4);
let page2: &[Step] = t.page(1).unwrap(); // Option: out-of-range never panics

// Playback
seq.play();
seq.set_fill(true);          // e.g. host's FILL key pressed
let out = seq.tick();
for ev in out.iter() {
    // ev.track fired with ev.velocity, ev.offset pulses into this step window
}
```

### Error handling & invariants

- Invalid values are prevented at construction wherever cheap: `UnitValue` clamps,
  `Track::set_length` rejects 0 and >128, `Condition::ratio(a, b)` validates bounds,
  `Step::set_micro` clamps to −23..=23, `Pattern::set_swing` clamps to 50..=80,
  `Retrig::set_vel_ramp` clamps to −1..=1, `queue_pattern` rejects out-of-range ids,
  `page`/`page_mut` return `Option` rather than panicking.
- No method panics on any sequence of public API calls (enforced by a fuzz/property
  test in the final slice).

---

## Implementation TODO

Organized as standalone vertical slices. Each slice leaves the crate compiling, tested,
and usable by a host; each subsequent slice only adds capability. Within a slice, items
are roughly ordered.

### Slice 1 — Walking skeleton: 16 tracks × 16 steps, trig on/off, transport

*Outcome: a host can program basic 16-step patterns on 16 tracks and drive playback,
getting velocity output (track defaults only).*

- [x] `cargo init --lib`, set edition, add `#![warn(missing_docs)]`, CI-friendly
      `cargo fmt`/`clippy` config
- [x] `UnitValue` newtype: clamping constructor, `get() -> f32`, `Default` (define
      default as 1.0 for velocity use), `PartialEq`, `Clone`, `Copy`, `Debug`
- [x] Constants: `NUM_TRACKS`, `STEPS_PER_PAGE`, `MAX_PAGES`, `MAX_STEPS`,
      `PULSES_PER_STEP`, `NUM_PARAM_LANES`
- [x] `Step` with just `trig: bool` for now (locks/conditions arrive in later slices);
      `Params { velocity, lanes }`; `Track { length (fixed 16 for this slice),
      defaults, steps: [Step; MAX_STEPS] }`; `Pattern { tracks }` — all `Clone +
      PartialEq + Debug + Default`
- [x] `Event` and `TickOutput` in their final shape (fixed-capacity event buffer;
      offsets all 0 until Slice 7; lanes resolved from track defaults until Slice 3)
- [x] `Sequencer` with private pattern storage exposed only via `current_pattern()` /
      `current_pattern_mut()` (so Slice 10's multi-pattern storage swap is
      non-breaking); `new()`, `play()`, `stop()`, `reset()`, `is_playing()`
- [x] `tick()`: evaluate-then-advance — examine the step at each playhead (fires iff
      `step.trig`), emit events with the track's default velocity and lanes, then
      advance and wrap playheads at length 16; while stopped, return empty and don't
      advance
- [x] Tests (via a `fired()` helper in `tests/common/mod.rs` that projects
      `TickOutput` to `[Option<f32>; NUM_TRACKS]`): programmed steps fire on the
      right ticks; wrap after 16 ticks; tick-while-stopped is a no-op; `play()`
      restarts from step 0; velocity output equals track default; editing `trig`
      while playing takes effect on next pass
- [x] Doc comments on all public items; crate-level doc with a runnable example

### Slice 2 — Pages and per-track length (polymeter)

*Outcome: tracks can be 1–128 steps (1–8 pages) with independent lengths; UI hosts get
page-oriented read/write helpers.*

- [x] `Track::set_length(steps: u8) -> Result<(), LengthError>` validating `1..=128`;
      `Track::length()` getter (field becomes private to protect the invariant)
- [x] Playhead wrapping uses per-track length; increment per-track `loop_count` on wrap
      (needed by Slice 5, cheap to add now)
- [x] Behavior decision (document it): when length is shortened below the current
      playhead position, the playhead wraps into range (`playhead % length`) on the
      next tick rather than panicking, **without** incrementing `loop_count` (an
      edit artifact is not a completed loop)
- [x] Page helpers: `page_count()`, `page(i) -> Option<&[Step]>`,
      `page_mut(i) -> Option<&mut [Step]>`, `set_page_count(n) -> Result` (sets
      length to `n * 16`)
- [x] Tests: two tracks with lengths 16 and 12 drift against each other and realign at
      step 48 (polymeter); length-1 track fires every tick; shortening length while
      playing rewraps safely; page helpers index correctly; out-of-range page/length
      rejected
- [x] Property test (or table test): for any length 1..=128, playhead visits exactly
      steps `0..length` cyclically

### Slice 3 — Parameter locks

*Outcome: per-step overrides of velocity and all 18 parameter lanes — the Elektron
p-lock mechanic.*

- [x] `ParamLocks { velocity: Option<UnitValue>, lanes: [Option<UnitValue>; 18] }`
      on `Step`, `Default` = no locks
- [x] Resolution helper `resolve(step, &track.defaults) -> (velocity, lanes)`:
      per field, lock if set, else track default (reused by Slice 9 for train base
      values)
- [x] Convenience: `Step::clear_locks()`; `Track::clear_all_locks()`
- [x] Tests: locked step emits lock value, unlocked steps emit default — for
      velocity and for individual lanes; changing the track default doesn't affect
      locked steps; clearing a lock reverts to default; lane locks ride only fired
      trigs (a lock on a non-trig step emits nothing); lock survives
      `play()`/`stop()` (it's pattern data, not runtime state)
- [x] Doc note for future maintainers: adding a parameter = picking a lane index;
      the engine never changes

### Slice 4 — Probability trigs

*Outcome: per-step fire probability with deterministic, seedable randomness.*

- [x] Introduce `Condition` enum with only `Always` and `Percent(UnitValue)` variants,
      `#[non_exhaustive]` from day one; add `condition: Condition` to `Step`
      (`Default` = `Always`)
- [x] Embed a small deterministic PRNG (PCG32 or xoshiro — implement in-crate or via a
      `rand_core` dependency; keep it swappable behind a private type alias)
- [x] `Sequencer::new(seed: u64)` and `Sequencer::seed(u64)`
- [x] Evaluation: `Percent(p)` draws from the PRNG only when an enabled trig with that
      condition is evaluated; fires iff `draw < p`
- [x] Tests: `Percent(1.0)` always fires, `Percent(0.0)` never fires; same seed + same
      pattern ⇒ identical output over 1000 ticks (determinism); different seeds
      diverge; statistical sanity check (p=0.5 over 10k evaluations fires within a
      generous tolerance band); draws only consumed by Percent steps (adding an
      unrelated Always trig doesn't shift another track's random sequence)

### Slice 5 — Conditional trigs

*Outcome: the full Elektron condition set: FILL, PRE, NEI, 1ST, A:B.*

- [x] Extend `Condition` with `Fill`, `NotFill`, `Pre`, `NotPre`, `Nei`, `NotNei`,
      `First`, `NotFirst`, `Ratio { a, b }`; validated `Condition::ratio(a, b)`
      constructor
- [x] `Sequencer::set_fill(bool)` / `fill()` flag
- [x] Per-track `last_cond: bool` in `TrackState`; implement the normative update
      rules from the spec (Always/Pre/Nei don't update it; Percent/Fill/First/Ratio do)
- [x] Evaluate tracks in order 0→15 so `Nei` on track N sees track N-1's result from
      the current tick
- [x] `First`/`Ratio` read the per-track `loop_count` added in Slice 2; reset on
      `play()`/`reset()`
- [x] Tests, one per rule at minimum:
  - [x] `Fill` fires only while fill flag set; `NotFill` is its complement; toggling
        mid-playback takes effect immediately
  - [x] `Pre` mirrors a preceding `Percent` step's outcome on the same track (seeded
        so both outcomes occur); `NotPre` is the complement; `Always` steps in between
        don't disturb it
  - [x] `Nei` mirrors the neighbor track's most recent *state-writing* conditional;
        `Nei` on track 0 never fires; NEI is transparent — a `Nei` step does not
        write `last_cond`, so track 2 watching track 1 (whose step is itself `Nei`)
        reads track 1's last state-writing conditional, not its `Nei` outcome
  - [x] `First` fires only during loop 0 of its own track — verify against a
        polymeter neighbor to prove per-track loop counting
  - [x] `Ratio{1,2}`/`Ratio{2,2}` alternate loops; `Ratio{8,8}` fires once per 8 loops;
        invalid ratios rejected
  - [x] `play()` resets loop counts and `last_cond` (a `First` trig fires again)

### Slice 6 — Pulse timeline and pending plumbing

*Outcome: the sub-step machinery micro-timing, swing, and retrigs will build on.
Behavior is unchanged after this slice (every offset is still 0); `Event` and
`TickOutput` already exist in final shape from Slice 1.*

- [x] Pulse-position helpers on `Track` (step → pulse, cyclic pulse length =
      `length * 24`)
- [x] Per-track `PendingEvents` buffer (capacity 4, debug-asserted; stays empty
      until later slices); consumed at the start of each window; cleared on
      `play()`/`stop()`/`reset()`
- [x] Stable insertion sort of `TickOutput` by `(offset, track)`, preserving push
      order for ties; document the capacity proof from the Timing semantics section
      and debug-assert on push
- [x] Tests: ordering guarantee; all offsets still 0; a fixed multi-track pattern
      produces output identical to its Slice 5 behavior (behavior-unchanged check)

### Slice 7 — Micro-timing

*Outcome: per-step ±23-pulse nudge, Elektron micro-timing.*

- [x] `Step::set_micro(i8)` clamping to −23..=23; `micro()` getter; `Default` = 0
- [x] Window examination rule: in window `W`, examine step `W` (if `micro >= 0`, emit
      at offset `m`) then step `W + 1` (if `micro < 0`, emit at offset `24 + m`)
- [x] Wrap behavior: negative micro on step 0 lands at the end of the loop; silent on
      the first pass after `play()`
- [x] Conditions/probability evaluated at examination time, grid order
- [x] Tests: positive micro delays within the window; negative micro emits in the
      previous window; step-0 wrap case (silent pass one, audible thereafter); same-
      pulse collision (`S` at +23, `S+1` at −1) emits both events; `PRE` reading an
      upstream step that has `micro = +23` still sees its result (grid order);
      determinism golden comparison: all-zero micro ⇒ bit-identical to Slice 6 output

### Slice 8 — Swing

*Outcome: per-pattern swing, 50–80%.*

- [x] `Pattern::set_swing(u8)` clamping to 50..=80; `swing()` getter; default 50
- [x] Delay odd-indexed steps by `round(48 * swing / 100) − 24` pulses, additive with
      micro-timing
- [x] Spill-over: combined offset ≥ 24 routes through the pending buffer into the
      next window
- [x] Tests: swing 50 is bit-identical to no swing; 75 delays odd steps by exactly 12
      pulses and leaves even steps untouched; 80 → 14 pulses; swing + micro spill
      into the next window arrives correctly; polymeter parity is per-track step
      index (15-step track verified)

### Slice 9 — Retrigs

*Outcome: per-step retrig trains with rate, length, and velocity ramp.*

- [x] `Retrig`, `RetrigRate` (with the pulse-interval table), `RetrigLength`;
      `Step.retrig: Option<Retrig>`; validated constructors
- [x] `ActiveTrain` per-track runtime state: starts when a retrig trig fires; hits
      emitted across subsequent windows via the pending machinery
- [x] Replacement rule: any newly fired trig on the track replaces the active train,
      truncating the old train's hits at pulses ≥ the new trig's scheduled pulse;
      `stop()`/`reset()`/`play()`/pattern-change clears it
- [x] The fired trig's event *is* hit `k = 0` — exactly one event at the scheduled
      pulse, no double emission
- [x] Velocity ramp: linear from resolved trig velocity, hit `k` of `n` =
      `clamp(v0 + ramp * k/(n−1))`; `n = 1` plays at `v0`; `Infinite` ignores ramp
- [x] Condition gates the whole train (one evaluation); velocity/lane p-locks set
      the train's base values; lane values ride unchanged on every hit
- [x] Tests: hit spacing matches the table for every rate; a train spans window
      boundaries; finite train hit count = `length / interval + 1`; single-hit train
      (`length < interval`) plays at `v0`; `Infinite` runs until the next fired
      trig; mid-train replacement truncates at the new trig's pulse; ramp endpoints
      and clamping; lanes identical across hits; worst case (16 tracks × `R96`)
      stays within `TickOutput` capacity; probability-gated retrig is deterministic
      under a fixed seed

### Slice 10 — Pattern chaining

*Outcome: multiple patterns with queued, quantized changes — the Elektron
pattern-change/chain mechanic.*

- [x] `Sequencer` holds `Vec<Pattern>` + `current: PatternId`; `new()` starts with one
      default pattern; `add_pattern`, `pattern(id)`, `pattern_mut(id)`,
      `current_pattern()`, `current_pattern_mut()`, `current_pattern_id()`
- [x] `Pattern.change_length: Option<NonZeroU16>`; master length = longest track
      length when `None`
- [x] `master_step` counter; change boundary = `master_step % change_length == 0`
- [x] `queue_pattern(id)` (validates id), `queue_chain(&[id])`, `clear_queue()`,
      `pending()`; queue head consumed at each boundary
- [x] Switch semantics: reset playheads, loop counts, `last_cond`, trains, pending
      events, `master_step`; PRNG and fill flag persist
- [x] Tests: change lands exactly on the boundary no matter when it was queued;
      default master length under polymeter (tracks 16/12 ⇒ boundary at 16); explicit
      `change_length = 4` switches mid-pattern; chain of 3 plays in order, then the
      last loops; `1ST` fires again after a switch; `clear_queue` cancels a pending
      change; out-of-range id rejected at queue time; `play()` does not clear the
      queue

### Slice 11 — Persistence, hardening, example host

*Outcome: crate is releasable: serializable patterns, fuzz-clean, documented, with a
reference host demonstrating the shear line.*

- [x] `serde` feature flag: `Serialize`/`Deserialize` for `Pattern` and everything in
      it, including `Retrig`, swing, and `change_length` (not `Sequencer` runtime
      state); round-trip test; feature-gated in CI
- [x] Property/fuzz test: arbitrary sequence of public API calls (edits including
      raw `Ratio { a, b }` literals and out-of-range page indices, transport, ticks,
      seeds, queueing) never panics and never emits a velocity or lane value outside
      `0.0..=1.0`
- [x] Golden-file test: a richly-featured project (all condition types, locks,
      polymeter, micro-timing, swing, retrigs, and a two-pattern chain) ticked 512
      times with a fixed seed, output snapshot committed — guards against accidental
      semantic changes
- [x] `examples/cli_host.rs`: minimal host that programs a demo pattern, runs a
      tokio-free loop with `std::thread::sleep` as its "clock", prints fired steps,
      and demonstrates a lane → synth-parameter mapping table (lanes 0–5 "FM ops",
      6–9 "ADSR", 10–17 "macros") — proves the headless API is sufficient and the
      shear line holds
- [x] README with the architecture diagram, semantics table, and quickstart
- [x] Final API review: every public item documented, `#[non_exhaustive]` present on
      `Condition` (introduced in Slice 4), audit `pub` fields vs. accessors against
      the invariants
