# plock

A headless Elektron-like step sequencer in Rust.

The host owns time and presentation; the sequencer is a deterministic state
machine you drive one step at a time. [`spec.md`](spec.md) is the full design
document with normative semantics; this README is the developer tour.

```rust
use plock::{Condition, Sequencer, UnitValue};

let mut seq = Sequencer::new(0xDEAD_BEEF); // seed for probability trigs

let track = &mut seq.current_pattern_mut().tracks[0];
track.steps[0].trig = true;
track.steps[4].trig = true;
track.steps[8].trig = true;
track.steps[8].condition = Condition::Percent(UnitValue::new(0.5));
seq.current_pattern_mut()
    .set_velocity_lock(0, 4, UnitValue::new(0.3)) // p-lock; Err when the
    .unwrap();                                    // pool is full

seq.play();
let out = seq.tick(); // call once per sixteenth note
for ev in &out {
    // ev.track fired with ev.velocity (0..=1), ev.offset pulses into this
    // step window, carrying 64 resolved lane values in ev.lanes (with
    // ev.locked marking which came from locks).
}
```

A complete host lives in [`examples/cli_host.rs`](examples/cli_host.rs)
(`cargo run --example cli_host`), including a lane → synth-parameter mapping
table and tempo handling.

## Feature summary

| Feature | Shape |
|---|---|
| Tracks | 16, each 1–128 steps (16 steps/page × up to 8 pages) |
| Polymeter | Per-track lengths; tracks wrap and count loops independently |
| Parameter lanes | 64 generic, unlabeled `0..1` values per event; host defines meaning |
| Parameter locks | Per-step overrides of velocity and any lane, in a per-pattern pool of ≤ 80 distinct (track, destination) lock lanes — the Elektron lock budget |
| Probability trigs | `Condition::Percent(p)`, seeded PRNG, bit-identical replays |
| Conditional trigs | `FILL`/`!FILL`, `PRE`/`!PRE`, `NEI`/`!NEI`, `1ST`/`!1ST`, `A:B` ratios |
| Micro-timing | Per-step nudge, −23..=+23 pulses (24 pulses = 1 step) |
| Swing | Per-pattern 50–80%, delays odd-indexed steps |
| Retrigs | Per-step trains: rate (1/4..1/96), length, velocity ramp |
| Pattern chaining | Queued changes quantized to a per-pattern change length |
| Output | Per tick: sorted, fixed-capacity list of timed events |

## Architecture: the shear line

```
┌─────────────────────────────────────────────┐
│ Host (UI, audio engine, MIDI bridge, tests) │
│   owns: clock, tempo, rendering, input      │
└───────────────┬─────────────────────────────┘
                │ tick() / play() / set_fill() / queue_pattern() / edits
                ▼
┌─────────────────────────────────────────────┐
│ Sequencer  (transport + playback state)     │
│   playheads, loop counts, PRE/NEI state,    │
│   PRNG, fill flag, change queue, retrig     │
│   trains, pending events                    │
├─────────────────────────────────────────────┤
│ Patterns  (pure data: what to play)         │
│   N × Pattern { swing, change_length,       │
│     lock pool (≤ 80 lock lanes),            │
│     16 × Track { length, defaults,          │
│                  steps: [Step; 128] } }     │
└─────────────────────────────────────────────┘
```

`Pattern` is plain data — `Clone`, `PartialEq`, optionally `Serialize`. It
contains nothing about playback position, so a UI can hold, diff, copy, and
persist patterns without touching the running engine. `Sequencer` wraps the
patterns with **all** runtime state. Everything UI-ish (what page is shown,
how encoders map, momentary-vs-latched fill buttons) belongs to the host.

All pattern mutation is legal while the transport runs, as on the hardware;
edits take effect the next time the affected step is evaluated.

### Parameter lanes & the lock pool

The engine never knows what a lane means. `Params` carries a distinguished
`velocity` plus `lanes: [UnitValue; 64]` as track defaults; each step can lock
any of them; each fired event carries the resolved values (`lock if set, else
track default`) plus provenance — `Event::locked` is a bitmask of which lanes
came from locks, and `Event::velocity_locked` covers velocity. The host owns
the mapping table and unit scaling, e.g.:

```text
lanes 0–5   → FM operator levels
lanes 6–9   → ADSR attack/decay/sustain/release
lanes 10–17 → performance macros
lanes 18–63 → yours to assign
```

Lanes are **event-scoped**: values ride only on fired trigs, so a lane lock
behind a 50% probability trig lands only when that trig actually fires.
Velocity is not lane 0 because it has engine-side semantics: it gates output
and drives the retrig velocity ramp.

Locks are stored Elektron-style, in a **per-pattern pool**: each distinct
(track, destination) pair locked anywhere in the pattern occupies one of
`MAX_LOCK_LANES = 80` slots (velocity counts as a destination). Locking more
steps of an already-locked destination is free; locking an 81st distinct
destination returns `LockError::PoolFull` — show your "max locks reached"
message; clearing a destination's last locked step frees its slot. Edits go
through `Pattern`'s lock API (`set_lane_lock`, `set_velocity_lock`,
`lane_lock`, `clear_*`, `lock_count`, `resolve_step`); allocation happens
only there, never in `tick()`.

## Time model

Two units of time:

- **Step** — a sixteenth note. The host calls `tick()` once per step. The
  engine never sees milliseconds, BPM, or a wall clock.
- **Pulse** — 1/24 of a step (96 PPQN). All sub-step timing — micro-timing,
  swing, retrig intervals — is whole pulses:
  `pulse_duration = step_duration / 24`.

Each `tick()` covers the window `[step, step + 1)` and returns every event
scheduled inside it, stamped with its pulse offset from the window start and
sorted by `(offset, track)`. An audio host converts offsets to sample
positions for sample-accurate scheduling; a simple host ignores them.

`tick()` follows a fixed phase order (normative in the spec): stopped-check →
pattern-change boundary → spilled events from the previous window → per-track
step examination and retrig hits (tracks in order 0→15) → sort → advance
playheads. The first tick after `play()` evaluates step 0.

Two timing behaviors worth knowing as a host author:

- **Negative micro-timing emits early.** A step with `micro = -3` plays near
  the end of the *previous* window (offset 21). Consequently a negative-micro
  trig on step 0 plays at the end of the loop — and is silent on the very
  first pass after `play()`, because that window never existed. Matches the
  hardware.
- **Conditions evaluate in grid order, not audible order.** `PRE`/`NEI`
  relationships follow step/track order even when micro-timing reorders the
  audible events.

## API tour

### Transport & runtime

```rust
let mut seq = Sequencer::new(seed);   // stopped, one empty pattern
seq.play();                            // (re)start from step 0
seq.stop();                            // freeze; tick() returns empty
seq.reset();                           // rewind without changing transport
seq.seed(123);                         // reseed the PRNG (replayable takes)
seq.set_fill(true);                    // FILL conditions read this flag
let heads: [u8; 16] = seq.playheads(); // cursor positions, for UIs
```

Fill is deliberately just a flag: momentary (hold), latched (toggle), or
one-shot press behaviors are host policy layered on `set_fill`.

### Editing

```rust
let p = seq.current_pattern_mut();
p.set_swing(62);                              // 50..=80, clamped
let t = &mut p.tracks[3];
t.set_length(48)?;                            // 1..=128, polymeter per track
t.set_page_count(4)?;                         // == set_length(64)
let page: &[Step] = t.page(1).unwrap();       // Option, never panics
t.defaults.velocity = UnitValue::new(0.8);
t.steps[0].trig = true;
t.steps[0].set_micro(-11);                    // clamps to -23..=23
t.steps[0].retrig = Some(Retrig::new(RetrigRate::R32, RetrigLength::pulses(24), -0.5));
t.steps[0].condition = Condition::ratio(3, 8)?; // validated constructor
p.set_lane_lock(3, 0, 4, UnitValue::new(0.9))?; // p-lock: track 3, step 0, lane 4
p.lane_lock(3, 0, 4);                           // -> Some(UnitValue)
p.clear_lane_lock(3, 0, 4);                     // frees the pool slot
```

Invariants are guarded at the API boundary: `UnitValue` clamps (NaN → 0),
`set_micro`/`set_swing`/`set_vel_ramp` clamp, `set_length`/`set_page_count`/
`queue_pattern` return `Result`, page accessors return `Option`, and lock
setters return `Result` (`LockError::PoolFull` at the 81st distinct
destination, `LockError::OutOfRange` for bad indices — getters and clears
just return `None`/no-op). **No public API sequence panics** — enforced by a
property fuzzer (see Testing).

### Trig conditions

Probability and logical conditions share one slot per step (a single enum),
exactly like the hardware. The `PRE`/`NEI` machinery reads a per-track memory
of the most recent *state-writing* conditional:

| Condition | Fires when… | Writes state? |
|---|---|---|
| `Always` | always | no (transparent) |
| `Percent(p)` | a PRNG draw < `p` | yes |
| `Fill` / `NotFill` | fill flag set / unset | yes |
| `Pre` / `NotPre` | last state-writing conditional on this track passed / failed | no |
| `Nei` / `NotNei` | same, but reads track N−1 (track 0: `Nei` never fires) | no |
| `First` / `NotFirst` | track's first loop since `play()` / any later loop | yes |
| `Ratio { a, b }` | loop `a` of every `b`-loop cycle (1-based, `1 <= a <= b <= 8`) | yes |

Notes:

- Tracks evaluate 0→15 within a tick, so `Nei` sees its neighbor's result
  from the *same* tick.
- `Nei` chains are not transitive: a `Nei` step doesn't write state, so a
  watcher of that track reads its last state-writing conditional instead.
- `Ratio` fields are public for ergonomic literals; evaluation defensively
  clamps out-of-range values (e.g. `b: 0`) instead of panicking. Use
  `Condition::ratio(a, b)` when you want validation.
- Loop counts are per track: under polymeter, `1ST` and `A:B` follow each
  track's own loop, not wall-clock bars.
- A PRNG draw is consumed **only** when an enabled `Percent` trig is
  evaluated, so unrelated edits don't shift other tracks' random sequences.

### Retrigs

A fired trig with `retrig: Some(..)` becomes hit 0 of a **train**: repeats at
the rate's pulse interval, for `length` pulses (or `Infinite`). The trig's
condition gates the whole train (evaluated once); lane values ride unchanged
on every hit; velocity ramps linearly from the trig's resolved velocity to
`velocity + vel_ramp`, clamped per hit. A track has at most one train: any
newly fired trig replaces it, truncating remaining hits at or after the new
trig's pulse. Rates are the note values representable in whole pulses
(`R4`=96 … `R96`=4; the hardware's 1/80 doesn't exist on a 96 PPQN grid —
documented divergence).

### Pattern chaining

```rust
let b = seq.add_pattern(Pattern::default()).unwrap(); // -> PatternId
seq.queue_pattern(b)?;            // applies at the next change boundary
seq.queue_chain(&[b, c])?;        // chains queue several changes
seq.pending();                    // Option<PatternId>
seq.clear_queue();
```

A change boundary occurs every `change_length` steps of the current pattern
(default: its **master length**, the longest track length). On switch:
playheads, loop counts, `PRE`/`NEI` state, retrig trains, and in-flight
spilled events reset (so `1ST` fires again); the PRNG and fill flag carry
over. The last pattern of a chain loops; to loop a whole chain, re-queue it.

⚠️ Two behaviors that surprise hosts:

- `master_step == 0` *is* a boundary and `play()` does **not** clear the
  queue — a change queued while stopped applies on the very first tick.
- The *current* pattern's `change_length` decides when the *next* pattern
  starts (Elektron semantics).

## Determinism

Given the same seed, pattern, and call sequence, output is bit-identical.
Randomness comes from an in-crate PCG32 — no global state, no system entropy.
`seed()` reseeds mid-session for reproducible takes. A 512-tick golden
snapshot ([`tests/golden_512.txt`](tests/golden_512.txt)) exercising every
feature pins the semantics; CI-style gates fail on any drift.

## Memory & stack-size notes

Fixed-size storage is a design goal (`tick()` never allocates or locks), and
it has consequences you should know about:

| Type | Size (measured, 64-bit) | Where it lives |
|---|---|---|
| `Step` | 20 B | trig, condition, micro, retrig — locks live in the pool |
| `Track` | ~2.8 KB | 128 steps + 64-lane defaults |
| `Pattern` | **~44 KB** + lock pool | 16 tracks inline; the pool adds ~544 B of heap per occupied slot (≤ ~43 KB at the 80-slot cap) |
| `Sequencer` | ~22 KB + heap | patterns are heap-allocated (`Vec<Pattern>`); the inline part is pending buffers and per-track state |
| `Event` | 272 B | velocity + 64 lanes + lock-provenance mask |
| `TickOutput` | ~42.5 KB | returned **by value** from `tick()`; capacity 160 events |

Practical guidance:

- **Patterns are cheap to keep around and snapshot.** At ~44 KB + a typically
  small pool, holding a 128-pattern bank in RAM costs a few MB, and
  clone-per-edit undo for performance modes is a non-event. (The dense
  per-step representation this replaced was ~345 KB per pattern at 18 lanes,
  and over 1 MB at 64.)
- **`Sequencer` keeps patterns on the heap**, allocated at construction and
  edit time only — never inside `tick()`. Lock edits may allocate (a new pool
  slot); `tick()`, including lock resolution, does not.
- **`TickOutput` is a ~42.5 KB by-value return.** Still a single memcpy per
  tick, but if your audio callback's stack budget is tight, hold it in a
  pre-allocated slot rather than nesting it deep in a call chain.
- The old warnings about ~320 KB `Pattern` temporaries overflowing 2 MiB
  thread stacks in debug builds no longer apply at the new sizes. The crate
  keeps `[profile.dev] opt-level = 1` anyway (un-elided `TickOutput` copies
  are still ~42 KB each, and the engine is meant to be run, not stepped
  through).

## serde support

```toml
plock = { version = "0.1", features = ["serde"] }
```

`Pattern` and everything inside it implement `Serialize`/`Deserialize`;
`Sequencer` runtime state deliberately does not. Deserialization re-validates
every invariant, in the same spirit as the live API: clampable scalars
re-clamp (velocities, lanes, lock values, micro, swing, `vel_ramp` —
hand-edited files are tolerated), while structural breakage errors out (track
length 0 or > 128, step arrays that aren't exactly 128 long, lane arrays that
aren't exactly 64 long, and any malformed lock pool: bad track/destination/
step indices, duplicate destinations or steps, empty entries, more than 80
slots). The long arrays round-trip as plain sequences via hand-rolled
`serde(with)` modules, since serde lacks built-in impls for arrays longer
than 32.

The lock pool serializes **sparsely** — one `{track, dest, steps: [[step,
value], …]}` entry per occupied slot, locked steps only — so files stay small
no matter the lane count. A pattern with no `locks` field (any pre-pool file)
loads with an empty pool: old files open fine, but their per-step locks are
silently dropped (pre-1.0 format break, this one time).

## Testing

```sh
cargo test                       # all suites
cargo test --features serde      # + serialization round-trips
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run --example cli_host     # hear-it-in-your-head smoke test
```

98 tests across twelve integration suites (plus in-module unit tests and a
doctest):

- `tests/transport.rs`, `polymeter.rs`, `locks.rs` — core engine behavior,
  including a proptest property that any track length cycles exactly
  `0..length`, plus the lock-pool rules: exhaustion at the 80th distinct
  destination, slot freeing and reuse, one-slot-per-destination accounting,
  locked-mask provenance, and out-of-range safety.
- `tests/conditions.rs`, `determinism.rs` — every condition rule (incl. NEI
  transparency, grid-order PRE) and the determinism contract (same seed ⇒
  identical 1000-tick output; `Always` steps consume no draws).
- `tests/timing.rs`, `retrig.rs` — micro-timing windows and wrap behavior,
  swing parity under polymeter, spill-over, train truncation, ramp math,
  and the worst-case capacity bound (16 tracks × `R96`).
- `tests/chaining.rs` — boundary exactness, chains, queue semantics.
- `tests/fuzz.rs` — proptest fuzzer applying arbitrary public-API call
  sequences (including invalid `Ratio` literals, out-of-range ids, and lock
  set/clear churn against the pool cap): asserts no panics, all outputs
  within `0.0..=1.0`, and `lock_count() <= MAX_LOCK_LANES` throughout.
- `tests/golden.rs` — the 512-tick snapshot. After an *intentional* semantic
  change, regenerate with `UPDATE_GOLDEN=1 cargo test --test golden` and
  review the diff like source code.

## Project layout

```
src/
  lib.rs        crate docs, constants, re-exports
  unit.rs       UnitValue (clamped 0..=1 newtype)
  step.rs       Step (trig, condition, micro, retrig)
  locks.rs      LockLane (pool slot), LockError, ResolvedStep
  condition.rs  Condition + validated ratio()
  retrig.rs     Retrig, RetrigRate, RetrigLength
  track.rs      Track, Params, pages, pulse helpers
  pattern.rs    Pattern, PatternId, swing, change_length, lock pool API
  rng.rs        private PCG32 (~30 lines)
  output.rs     Event, TickOutput (fixed-capacity, stable sort)
  sequencer.rs  Sequencer: transport, examination, trains, chaining
```

The pure-data layer (`step`/`condition`/`retrig`/`track`/`pattern`) never
references playback state; the runtime layer (`sequencer`/`output`/`rng`)
never renders or schedules. [`spec.md`](spec.md) §"Implementation TODO" is
the authoritative checklist the code was built against, slice by slice.

## Non-goals (today)

Sound generation, MIDI I/O, clock/tempo, per-track speed multipliers, song
arrangement (chaining is in; songs are not), inter-step lane interpolation,
live recording, `no_std`. See the spec for rationale and extension notes.

## License

MIT OR Apache-2.0.
