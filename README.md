# elektronlike

A headless Elektron-style step sequencer in Rust: pure state and behavior —
no UI, no audio, no MIDI, no clock, no threads, no I/O.

The host owns time and presentation; the sequencer is a deterministic state
machine you drive one step at a time. [`spec.md`](spec.md) is the full design
document with normative semantics; this README is the developer tour.

```rust
use elektronlike::{Condition, Sequencer, UnitValue};

let mut seq = Sequencer::new(0xDEAD_BEEF); // seed for probability trigs

let track = &mut seq.current_pattern_mut().tracks[0];
track.steps[0].trig = true;
track.steps[4].trig = true;
track.steps[4].locks.velocity = Some(UnitValue::new(0.3)); // p-lock
track.steps[8].trig = true;
track.steps[8].condition = Condition::Percent(UnitValue::new(0.5));

seq.play();
let out = seq.tick(); // call once per sixteenth note
for ev in &out {
    // ev.track fired with ev.velocity (0..=1), ev.offset pulses into this
    // step window, carrying 18 resolved lane values in ev.lanes.
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
| Parameter lanes | 18 generic, unlabeled `0..1` values per event; host defines meaning |
| Parameter locks | Per-step overrides of velocity and any lane |
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

### Parameter lanes

The engine never knows what a lane means. `Params` carries a distinguished
`velocity` plus `lanes: [UnitValue; 18]` as track defaults; each step can lock
any of them; each fired event carries the resolved values (`lock if set, else
track default`). The host owns the mapping table and unit scaling, e.g.:

```text
lanes 0–5   → FM operator levels
lanes 6–9   → ADSR attack/decay/sustain/release
lanes 10–17 → performance macros
```

Lanes are **event-scoped**: values ride only on fired trigs, so a lane lock
behind a 50% probability trig lands only when that trig actually fires.
Velocity is not lane 0 because it has engine-side semantics: it gates output
and drives the retrig velocity ramp.

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
t.steps[0].locks.lanes[4] = Some(UnitValue::new(0.9));
t.steps[0].retrig = Some(Retrig::new(RetrigRate::R32, RetrigLength::pulses(24), -0.5));
t.steps[0].condition = Condition::ratio(3, 8)?; // validated constructor
```

Invariants are guarded at the API boundary: `UnitValue` clamps (NaN → 0),
`set_micro`/`set_swing`/`set_vel_ramp` clamp, `set_length`/`set_page_count`/
`queue_pattern` return `Result`, page accessors return `Option`. **No public
API sequence panics** — enforced by a property fuzzer (see Testing).

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

| Type | Size (approx.) | Where it lives |
|---|---|---|
| `Step` | ~156 B | 19 optional locks (`Option<UnitValue>` is 8 B — no niche) |
| `Track` | ~20 KB | 128 steps + defaults |
| `Pattern` | **~320 KB** | 16 tracks |
| `Sequencer` | ~few hundred B + heap | patterns are heap-allocated (`Vec<Pattern>`) |
| `TickOutput` | ~12.8 KB | returned **by value** from `tick()`; capacity 160 events |

Practical guidance:

- **`Sequencer` keeps patterns on the heap**, allocated at construction and
  edit time only — never inside `tick()`. The struct itself is small and
  cheap to move.
- **Avoid placing a bare `Pattern` on a small thread's stack in debug
  builds.** Unoptimized builds don't elide moves, so chained construction
  (`Pattern::default()` → struct literal → return) can briefly stack several
  ~320 KB copies. This crate sets `[profile.dev] opt-level = 1` (debug
  assertions stay on) specifically because the un-elided copies overflowed
  the default 2 MiB test-thread stack; dependents compile this crate with
  their own profile, so if you build at `opt-level = 0` and construct
  patterns on worker threads, give those threads headroom
  (`std::thread::Builder::stack_size`) or build patterns on the main thread
  (8 MiB by default).
- **serde deserialization of a full `Pattern` holds a few ~320 KB
  temporaries at once** (the in-progress array in the visitor, the returned
  value, your binding). On the main thread that's fine; on a default 2 MiB
  spawned thread in a debug build it can overflow. The crate's own round-trip
  tests run on an explicitly sized 8 MiB thread for exactly this reason —
  do the same if you deserialize patterns off-thread.
- **`TickOutput` is a ~12.8 KB by-value return.** That's a trivial stack copy
  per tick, but if your audio callback's stack budget is tight, hold it in a
  pre-allocated slot rather than nesting it deep in a call chain.
- The per-step lock representation trades memory for simplicity (a bitmask +
  packed-values encoding could shrink it ~4× without changing the API);
  that's noted in the spec as a future compression.

## serde support

```toml
elektronlike = { version = "0.1", features = ["serde"] }
```

`Pattern` and everything inside it implement `Serialize`/`Deserialize`;
`Sequencer` runtime state deliberately does not. Deserialization re-validates
every invariant, in the same spirit as the live API: clampable scalars
re-clamp (velocities, lanes, micro, swing, `vel_ramp` — hand-edited files are
tolerated), while structural breakage errors out (track length 0 or > 128,
step arrays that aren't exactly 128 long). The 128-step array round-trips as
a plain sequence via a hand-rolled `serde(with)` module, since serde lacks
built-in impls for arrays longer than 32.

## Testing

```sh
cargo test                       # all suites
cargo test --features serde      # + serialization round-trips
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo run --example cli_host     # hear-it-in-your-head smoke test
```

88 tests across eleven integration suites (plus in-module unit tests and a
doctest):

- `tests/transport.rs`, `polymeter.rs`, `locks.rs` — core engine behavior,
  including a proptest property that any track length cycles exactly
  `0..length`.
- `tests/conditions.rs`, `determinism.rs` — every condition rule (incl. NEI
  transparency, grid-order PRE) and the determinism contract (same seed ⇒
  identical 1000-tick output; `Always` steps consume no draws).
- `tests/timing.rs`, `retrig.rs` — micro-timing windows and wrap behavior,
  swing parity under polymeter, spill-over, train truncation, ramp math,
  and the worst-case capacity bound (16 tracks × `R96`).
- `tests/chaining.rs` — boundary exactness, chains, queue semantics.
- `tests/fuzz.rs` — proptest fuzzer applying arbitrary public-API call
  sequences (including invalid `Ratio` literals and out-of-range ids):
  asserts no panics and all outputs within `0.0..=1.0`.
- `tests/golden.rs` — the 512-tick snapshot. After an *intentional* semantic
  change, regenerate with `UPDATE_GOLDEN=1 cargo test --test golden` and
  review the diff like source code.

## Project layout

```
src/
  lib.rs        crate docs, constants, re-exports
  unit.rs       UnitValue (clamped 0..=1 newtype)
  step.rs       Step, ParamLocks
  condition.rs  Condition + validated ratio()
  retrig.rs     Retrig, RetrigRate, RetrigLength
  track.rs      Track, Params, pages, pulse helpers
  pattern.rs    Pattern, PatternId, swing, change_length
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
