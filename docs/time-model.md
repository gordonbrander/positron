# The time model: steps, pulses, and how a host drives the clock

How the sequencer models time, why it's designed that way, and concrete
patterns for integrating it into a host application. (See spec.md "Timing"
for the normative scheduling rules; this doc is the design rationale and
host-side guide.)

## The model

The engine never sees milliseconds, BPM, or a wall clock. Its entire notion
of time is two units:

- **Step** — the unit of `Sequencer::tick()`. One call advances one step (a
  sixteenth note at the host's tempo). The host decides when steps happen.
- **Pulse** — 1/24 of a step (`PULSES_PER_STEP = 24`, i.e. a 96 PPQN grid).
  All sub-step timing — micro-timing, swing, retrig intervals — is expressed
  in whole pulses. The host converts: `pulse_duration = step_duration / 24`.

The 24-pulse grid matches Elektron hardware, whose micro-timing moves trigs
in 1/384-note increments (a sixteenth = 24/384). It's also the least common
multiple that makes every retrig rate (1/16 through 1/64, straight and
triplet) a whole number of pulses.

So time is **step-quantized at the API boundary, pulse-quantized inside**:
the host calls `tick()` at step rate, and every event comes back stamped
with a pulse `offset` (`0..24`) from the start of that step window. Nothing
in the engine is ever finer than a pulse, and nothing is ever a float
duration.

## The event contract

Each `tick()` returns a `TickOutput`: the complete set of events that sound
during that step window, sorted by `(offset, track)`. Three guarantees make
this easy to consume:

1. **Offsets are always in `0..24`.** The host never receives an event
   outside the window it just ticked.
2. **Negative micro-timing never requires time travel.** A step with
   `micro < 0` plays *early* — near the end of the previous window. The
   engine handles this by examination order: in window `W`, each track
   examines step `W` (if its `micro >= 0`) and step `W+1` (if its
   `micro < 0`), emitting the pulled event at offset `24 + micro`. By the
   time the host receives an event, it is always in the future relative to
   the window start it just ticked.
3. **Spill is handled internally.** Swing (0–14 pulses on odd steps) can
   push a combined offset past 24; those events go into a per-track pending
   buffer and emerge at the start of the next window. The host never sees
   this machinery.

Micro-timing is capped at ±23 pulses (`Step::MAX_MICRO`) — one pulse short
of a full step in either direction — so a nudged trig can never land *on*
an adjacent step's grid position. Steps stay distinct as authored objects
even when their audible positions interleave.

## Rationale

**Why the host owns the clock.** Tempo, transport sync, jitter compensation,
and output-device scheduling are all host concerns that vary wildly between
a CLI prototype, a DAW plugin, and an embedded box. Keeping the engine
clock-free means:

- `tick()` is the *only* time source, so playback is fully deterministic:
  same seed, same pattern, same call sequence → identical output. This is
  what makes the golden tests possible, and it makes offline rendering free
  (just call `tick()` in a loop with no clock at all).
- Tempo changes are trivially the host's problem: change `step_duration`
  between ticks and the engine never knows.
- No threads, no timers, no floating-point time anywhere in the engine.

**Why whole pulses instead of fractional offsets.** Integer pulses on a
fixed grid keep the engine exactly reproducible (no float accumulation
drift), keep events `Copy` and compact, and match the hardware being
modeled. The host converts to its own time base — samples, nanoseconds,
MIDI ticks — exactly once, at the boundary.

**Why one-step lookahead, not a reactive API.** Because every event in a
window is known at the window's start (guarantee 2 above), the host can
call `tick()` at or just before each step boundary and schedule the whole
window's events with timestamps. Timing accuracy then depends on the
*output sink's* scheduler, not on when the host's loop happens to wake up.
A reactive "what fires right now?" API would tie audible timing to host
loop jitter for no gain.

**Why `tick()` is per-step rather than per-pulse.** A pulse-rate `tick()`
looks attractive for simple loop-driven hosts ("call it every pulse, fire
whatever comes back immediately"), but it wouldn't simplify the engine:
conditions (`Pre`/`Nei`, probability) are evaluated at *examination* time
in grid order, which deliberately differs from audible order when
micro-timing reorders things. A pulse-rate engine would still have to
examine steps at window boundaries and buffer results to emit at the right
pulse — i.e. exactly what `TickOutput` already is, just hidden inside, with
all the step-level bookkeeping (playheads, change boundaries, loop counts)
multiplied by 24. Instead, a per-pulse driving loop is a ~15-line host-side
adapter over the existing API (sketch below). The same engine serves both
driving models with zero changes — that's the shear line working.

**Why `tick()` is allocation-free.** `TickOutput` is a fixed-capacity array
returned by value, sized by a worst-case proof (`MAX_EVENTS_PER_TICK`), and
the tick path never touches the heap. This makes `tick()` safe to call
directly on an audio thread. (Pattern *mutation* can allocate — keep edits
off the audio thread or behind a message queue.)

## Host integration sketches

### 1. Audio-callback host (sample-accurate; the serious one)

Convert steps to samples, tick when the buffer crosses a step boundary, and
render each event at its exact sample position within the buffer.

```rust
struct AudioHost {
    seq: Sequencer,
    samples_per_step: f64,     // sample_rate * 60.0 / bpm / 4.0
    next_step_sample: f64,     // absolute sample position of the next boundary
    sample_pos: u64,           // absolute playback position
    window: TickOutput,        // events of the current window
    window_start: f64,         // sample position of the current window's start
    cursor: usize,             // next un-rendered event in `window`
}

impl AudioHost {
    fn process(&mut self, buf: &mut [f32]) {
        let buf_end = self.sample_pos as f64 + buf.len() as f64;

        // Tick every step boundary that falls inside this buffer.
        while self.next_step_sample < buf_end {
            // Render events of the *current* window that precede the boundary.
            self.render_events_until(self.next_step_sample, buf);
            self.window = self.seq.tick();
            self.cursor = 0;
            self.window_start = self.next_step_sample;
            self.next_step_sample += self.samples_per_step; // f64: no drift
        }
        self.render_events_until(buf_end, buf);
        self.sample_pos += buf.len() as u64;
    }

    fn render_events_until(&mut self, limit: f64, buf: &mut [f32]) {
        let spp = self.samples_per_step / f64::from(PULSES_PER_STEP);
        while self.cursor < self.window.len() {
            let ev = &self.window.as_slice()[self.cursor];
            let at = self.window_start + f64::from(ev.offset) * spp;
            if at >= limit { break; }
            let frame = (at - self.sample_pos as f64) as usize;
            self.trigger_voice(ev, frame, buf); // sample-accurate start
            self.cursor += 1;
        }
    }
}
```

Notes:

- Keep boundary arithmetic in `f64` samples (`samples_per_step` is rarely
  an integer); accumulate the *boundary position*, not a remainder, so
  rounding error doesn't drift.
- Tempo change: recompute `samples_per_step` between buffers; the next
  boundary lands wherever the new tempo puts it.
- `tick()` is allocation-free and fine on this thread; pattern edits are
  not — send them through a lock-free queue and apply between ticks.

### 2. Per-pulse loop (fire-immediately; great for prototypes and CLIs)

Tick once per window, hold the output, and drain it pulse by pulse. Events
arrive sorted by offset, so a single advancing cursor suffices:

```rust
let pulse_duration = step_duration / u32::from(PULSES_PER_STEP);
let start = Instant::now();
// TickOutput has no public constructor (only tick() makes one), so the
// not-yet-ticked state is an Option.
let mut window: Option<TickOutput> = None;
let mut cursor = 0;

for pulse in 0u32.. {
    let in_window = (pulse % 24) as u8;
    if in_window == 0 {
        window = Some(seq.tick());
        cursor = 0;
    }
    let events = window.as_ref().map_or(&[][..], TickOutput::as_slice);
    while cursor < events.len() && events[cursor].offset == in_window {
        fire(&events[cursor]); // immediately — no scheduling
        cursor += 1;
    }
    // Absolute deadline so error never accumulates:
    sleep_until(start + pulse_duration * (pulse + 1));
}
```

The tradeoff is **loop jitter, not CPU**: at 132 BPM a pulse is ~4.7 ms,
and default OS sleep jitter (1–10 ms) is the same order. Mitigate with
absolute deadlines (as above) and, if it matters, sleep-then-spin for the
last ~1 ms. The result — ~1–5 ms granularity — is in the same ballpark as
classic hardware grooveboxes. Respectable for a groove; not sample-tight.

### 3. Timestamped-sink host (CoreMIDI / ALSA / OSC bundles)

If the output API accepts future timestamps, combine the simplicity of a
step-rate loop with scheduler-grade accuracy: wake once per step (jitter is
harmless — only *computation* time moves), convert each offset to an
absolute timestamp, and hand the events to the sink.

```rust
let mut window_start = now();
loop {
    let out = seq.tick();
    for ev in &out {
        let at = window_start + ev.offset as u64 * pulse_nanos;
        midi_send_at(ev, at); // sink's scheduler handles precision
    }
    window_start += step_nanos;
    sleep_until(window_start); // can even wake a little early or late
}
```

For extra safety margin, run one window ahead of real time (tick for window
`N+1` while window `N` plays) — the engine doesn't care, since `tick()` is
the clock.

### 4. Offline render / tests

No clock at all: `tick()` in a loop, accumulate events, convert offsets to
whatever time base the output needs (`step * 24 + offset` gives an absolute
pulse timeline). Determinism means a seeded render is exactly reproducible.

### Syncing to MIDI clock (incoming)

MIDI clock is 24 PPQN; the internal grid is 96 PPQN — one step = 6 MIDI
clocks, one MIDI clock = 4 pulses. Drive `tick()` every 6th incoming clock
and interpolate sub-clock event times from the measured inter-clock period
(which is what hardware does too). The engine needs no changes; clock
smoothing is host policy like everything else.

## Decision rule

| Output sink | Driving pattern | Timing accuracy |
| --- | --- | --- |
| Audio callback | Sketch 1: tick at buffer-crossed boundaries | Sample-accurate |
| Timestamped MIDI/OSC | Sketch 3: step-rate loop + timestamps | Sink-scheduler-accurate |
| Fire-immediately (synth call, GPIO) | Sketch 2: per-pulse loop | Loop-jitter-accurate (~1–5 ms) |
| File / test / bounce | Sketch 4: no clock | Exact |

The linchpin in every case is the one-step lookahead: a discrete step-rate
API whose events are always in the near future, so the host converts pulses
to real time however suits it — and the engine stays free of clocks,
threads, and floating-point time.
