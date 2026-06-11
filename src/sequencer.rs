//! The runtime layer: transport and playback state around a pattern.

use crate::rng::Pcg32;
use crate::{
    Condition, Event, NUM_PARAM_LANES, NUM_TRACKS, Pattern, PatternId, RetrigLength, TickOutput,
};
use std::collections::VecDeque;

/// Error for a [`PatternId`] that doesn't exist in this sequencer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidPatternId(pub PatternId);

impl std::fmt::Display for InvalidPatternId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no pattern with id {}", self.0.0)
    }
}

impl std::error::Error for InvalidPatternId {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Transport {
    Stopped,
    Playing,
}

/// Events displaced past their examination window (by swing or combined
/// micro+swing offsets ≥ 24 pulses), waiting for the next window. Spill is
/// at most one window deep and at most 2 events per window (see spec.md);
/// capacity 4 leaves margin.
#[derive(Clone, Copy, Debug, Default)]
struct PendingEvents {
    events: [Event; 4],
    len: u8,
}

impl PendingEvents {
    fn push(&mut self, event: Event) {
        debug_assert!((self.len as usize) < self.events.len(), "pending overflow");
        if (self.len as usize) < self.events.len() {
            self.events[self.len as usize] = event;
            self.len += 1;
        }
    }
}

/// An in-flight retrig train. Generative runtime state: hits are produced
/// on demand each window, never buffered ahead (an `Infinite` train would
/// need unbounded buffering otherwise).
#[derive(Clone, Copy, Debug)]
struct ActiveTrain {
    /// Pulses between hits.
    interval: u16,
    /// Hits emitted so far; the fired trig's own event is hit 0.
    emitted: u16,
    /// Total hits for a finite train (`length / interval + 1`), or `None`
    /// for `Infinite`.
    total: Option<u16>,
    /// Pulse position of the next hit relative to the current window's
    /// start; re-based by −24 as each window passes.
    next_offset: i16,
    /// The trig's resolved velocity — the ramp's starting point.
    velocity: f32,
    /// Velocity ramp over the train (see [`crate::Retrig::vel_ramp`]).
    vel_ramp: f32,
    /// The trig's resolved lanes, riding unchanged on every hit.
    lanes: [f32; NUM_PARAM_LANES],
    /// Which lanes came from locks — captured once with the lanes and
    /// riding unchanged on every hit, like them.
    locked: u64,
    /// Whether the trig's velocity came from a velocity lock.
    velocity_locked: bool,
}

impl ActiveTrain {
    fn done(&self) -> bool {
        self.total.is_some_and(|n| self.emitted >= n)
    }

    /// Velocity of hit `k`: linear from the trig's velocity to
    /// `velocity + vel_ramp`, clamped per hit. Single-hit and `Infinite`
    /// trains play flat.
    fn hit_velocity(&self, k: u16) -> f32 {
        match self.total {
            Some(n) if n > 1 => {
                (self.velocity + self.vel_ramp * f32::from(k) / f32::from(n - 1)).clamp(0.0, 1.0)
            }
            _ => self.velocity,
        }
    }
}

/// Per-track playback state. Runtime only — never part of pattern data.
#[derive(Clone, Copy, Debug, Default)]
struct TrackState {
    /// Next step to evaluate (the window index of the next tick).
    playhead: u8,
    /// Completed loops of this track since `play()` — drives the `1ST` and
    /// `A:B` trig conditions. Per-track because polymeter means tracks loop
    /// independently.
    loop_count: u32,
    /// Result of the most recent *state-writing* conditional evaluated on
    /// this track — what `Pre` (same track) and `Nei` (next track) read.
    last_cond: bool,
    /// Events spilled past the previous window, emitted at the start of the
    /// next one.
    pending: PendingEvents,
    /// The track's in-flight retrig train, if any. At most one: any newly
    /// fired trig replaces it.
    train: Option<ActiveTrain>,
}

/// The sequencer state machine: a pattern plus all playback state.
///
/// The host owns time: call [`tick()`](Self::tick) once per step (a sixteenth
/// note at the host's tempo) and schedule the returned events. All pattern
/// mutation is legal while playing, as on the hardware.
#[derive(Clone, Debug)]
pub struct Sequencer {
    /// Heap storage (a pattern is a few tens of KB, plus its lock pool),
    /// allocated at construction/edit time only — never inside `tick()`.
    /// Always holds at least one pattern; `current` is always in range.
    patterns: Vec<Pattern>,
    current: usize,
    /// Queued pattern changes; the head is consumed at each change boundary.
    queue: VecDeque<PatternId>,
    /// Steps since the current pattern started — drives change boundaries.
    master_step: u32,
    transport: Transport,
    fill: bool,
    rng: Pcg32,
    track_state: [TrackState; NUM_TRACKS],
}

impl Sequencer {
    /// Creates a stopped sequencer with one default (empty) pattern.
    ///
    /// The seed drives the deterministic PRNG behind probability trigs; the
    /// same seed, pattern, and call sequence always produce identical output.
    pub fn new(seed: u64) -> Self {
        Self {
            patterns: vec![Pattern::default()],
            current: 0,
            queue: VecDeque::new(),
            master_step: 0,
            transport: Transport::Stopped,
            fill: false,
            rng: Pcg32::new(seed),
            track_state: [TrackState::default(); NUM_TRACKS],
        }
    }

    /// Adds a pattern, returning its id. `None` if the sequencer is full
    /// (256 patterns).
    pub fn add_pattern(&mut self, pattern: Pattern) -> Option<PatternId> {
        let id = u8::try_from(self.patterns.len()).ok()?;
        self.patterns.push(pattern);
        Some(PatternId(id))
    }

    /// Number of patterns held.
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    /// The pattern with the given id, if it exists.
    pub fn pattern(&self, id: PatternId) -> Option<&Pattern> {
        self.patterns.get(usize::from(id.0))
    }

    /// Mutable access to the pattern with the given id, if it exists.
    pub fn pattern_mut(&mut self, id: PatternId) -> Option<&mut Pattern> {
        self.patterns.get_mut(usize::from(id.0))
    }

    /// The id of the pattern currently playing.
    pub fn current_pattern_id(&self) -> PatternId {
        PatternId(self.current as u8)
    }

    /// Queues a pattern change, applied at the next change boundary (every
    /// `change_length` steps of the current pattern, by default its master
    /// length). Queued changes accumulate into a chain; the last pattern of
    /// a chain loops.
    ///
    /// # Errors
    /// Rejects ids that don't exist (so the queue never holds a dangling id).
    pub fn queue_pattern(&mut self, id: PatternId) -> Result<(), InvalidPatternId> {
        if usize::from(id.0) >= self.patterns.len() {
            return Err(InvalidPatternId(id));
        }
        self.queue.push_back(id);
        Ok(())
    }

    /// Queues several pattern changes at once — a chain. All ids are
    /// validated before any is queued.
    ///
    /// # Errors
    /// Rejects the whole chain if any id doesn't exist.
    pub fn queue_chain(&mut self, ids: &[PatternId]) -> Result<(), InvalidPatternId> {
        for id in ids {
            if usize::from(id.0) >= self.patterns.len() {
                return Err(InvalidPatternId(*id));
            }
        }
        self.queue.extend(ids);
        Ok(())
    }

    /// Drops all queued pattern changes; the current pattern keeps looping.
    pub fn clear_queue(&mut self) {
        self.queue.clear();
    }

    /// The next queued pattern change, if any.
    pub fn pending(&self) -> Option<PatternId> {
        self.queue.front().copied()
    }

    /// Sets fill mode, read by the `Fill`/`NotFill` conditions at each
    /// evaluation. This is *the* fill trigger: momentary (hold), latched
    /// (toggle), or one-shot press behaviors are host policy layered on this
    /// flag.
    pub fn set_fill(&mut self, fill: bool) {
        self.fill = fill;
    }

    /// Current fill mode.
    pub fn fill(&self) -> bool {
        self.fill
    }

    /// Reseeds the PRNG (e.g. to replay a take). Transport, position, and
    /// pattern are untouched.
    pub fn seed(&mut self, seed: u64) {
        self.rng = Pcg32::new(seed);
    }

    /// The pattern currently being played (and edited).
    pub fn current_pattern(&self) -> &Pattern {
        &self.patterns[self.current]
    }

    /// Mutable access to the current pattern. Edits while playing are legal
    /// and take effect the next time the affected step is evaluated.
    pub fn current_pattern_mut(&mut self) -> &mut Pattern {
        &mut self.patterns[self.current]
    }

    /// Starts playback from step 0. Calling this while already playing
    /// restarts from step 0. Does not reseed the PRNG.
    pub fn play(&mut self) {
        self.transport = Transport::Playing;
        self.reset();
    }

    /// Halts playback. While stopped, `tick()` returns an empty output and
    /// advances nothing. The position is kept (only `play()`/`reset()` move
    /// it); in-flight sub-step events (spilled/retrig) are dropped.
    pub fn stop(&mut self) {
        self.transport = Transport::Stopped;
        for state in &mut self.track_state {
            state.pending = PendingEvents::default();
            state.train = None;
        }
    }

    /// Resets the playback position to step 0 without changing the transport
    /// state (useful for host-side song-position handling). Like `play()`,
    /// this does not clear the pattern-change queue.
    pub fn reset(&mut self) {
        for state in &mut self.track_state {
            *state = TrackState::default();
        }
        self.master_step = 0;
    }

    /// `true` while the transport is running.
    pub fn is_playing(&self) -> bool {
        self.transport == Transport::Playing
    }

    /// Each track's playhead: the step index the next tick will evaluate.
    /// A read-only view for UI hosts drawing the running cursor.
    pub fn playheads(&self) -> [u8; NUM_TRACKS] {
        std::array::from_fn(|i| self.track_state[i].playhead)
    }

    /// Advances one step: evaluates the step window under each track's
    /// playhead, then advances the playheads (wrapping per track).
    ///
    /// The first tick after [`play()`](Self::play) evaluates step 0.
    pub fn tick(&mut self) -> TickOutput {
        let mut out = TickOutput::new();
        if self.transport != Transport::Playing {
            return out;
        }
        // Change boundary: consume at most one queued pattern change.
        // master_step == 0 is a boundary, so a change queued while stopped
        // applies on the very first tick after play().
        if self.master_step % self.current_pattern().effective_change_length() == 0 {
            if let Some(id) = self.queue.pop_front() {
                self.current = usize::from(id.0);
                // New pattern, fresh positional state: playheads, loop
                // counts, Pre/Nei state, pending events, and trains reset
                // (1ST fires again); the PRNG and fill flag carry over.
                for state in &mut self.track_state {
                    *state = TrackState::default();
                }
                self.master_step = 0;
            }
        }
        // Tracks are examined in order 0..16, so `Nei` on track N reads
        // track N−1's state from *this* tick.
        for i in 0..NUM_TRACKS {
            // Events spilled into this window by the previous one (their
            // conditions were already evaluated at examination time).
            for k in 0..usize::from(self.track_state[i].pending.len) {
                out.push(self.track_state[i].pending.events[k]);
            }
            self.track_state[i].pending.len = 0;

            let length = self.patterns[self.current].tracks[i].length();
            // A live edit may have shortened the track below the playhead;
            // wrap into range without counting a loop (edit artifact).
            if self.track_state[i].playhead >= length {
                self.track_state[i].playhead %= length;
            }
            let window = self.track_state[i].playhead;
            // Step W plays in this window unless its micro-timing is
            // negative (then it already played at the end of the previous
            // window).
            self.examine_step(i, window, false, &mut out);
            // Step W+1 plays at the end of *this* window when its
            // micro-timing pulls it early. Wrapping to step 0 means the
            // event belongs to the track's next loop.
            self.examine_step(i, (window + 1) % length, true, &mut out);
            // Retrig-train hits falling inside this window (including a
            // train started by one of the examinations above).
            self.emit_train_hits(i, &mut out);
            // Natural advance; wrapping here is a completed loop.
            let state = &mut self.track_state[i];
            state.playhead += 1;
            if state.playhead >= length {
                state.playhead = 0;
                state.loop_count += 1;
            }
        }
        self.master_step += 1;
        out.sort();
        out
    }

    /// Examines one step for the current window. `pulled` selects the pass:
    /// the un-pulled pass takes the window's own step when its `micro >= 0`
    /// (event at offset `micro`), the pulled pass takes the *next* step when
    /// its `micro < 0` (event at offset `24 + micro`). Each step is thus
    /// examined exactly once per loop. Conditions and probability evaluate
    /// here, at examination time — in grid order, even when micro-timing
    /// makes the audible order differ.
    fn examine_step(&mut self, track: usize, index: u8, pulled: bool, out: &mut TickOutput) {
        let step = self.patterns[self.current].tracks[track].steps[usize::from(index)];
        if !step.trig || pulled != (step.micro() < 0) {
            return;
        }
        // A pulled examination that wrapped to step 0 belongs to the track's
        // next loop: loop-scoped conditions evaluate against loop_count + 1.
        // (Consequence, per spec: `First` on a negative-micro step 0 never
        // fires — its loop-0 instance falls before the transport started.)
        let mut loop_count = self.track_state[track].loop_count;
        if pulled && index == 0 {
            loop_count += 1;
        }
        if self.eval_condition(track, step.condition, loop_count) {
            // Owned copy out of the pool (ResolvedStep is Copy) — the
            // pattern borrow must end here, before the &mut self calls
            // below.
            let Some(resolved) =
                self.patterns[self.current].resolve_step(track, usize::from(index))
            else {
                debug_assert!(false, "examine_step indices are validated by tick()");
                return;
            };
            let micro = i16::from(step.micro());
            // Un-pulled: micro in 0..=23. Pulled: 24 + micro in 1..=23.
            // Swing adds 0..=14 pulses to odd-indexed steps.
            let swing = if index % 2 == 1 {
                self.patterns[self.current].swing_delay()
            } else {
                0
            };
            let offset = if pulled { 24 + micro } else { micro } + swing;
            debug_assert!((0..24 + 14).contains(&offset));
            let event = Event {
                track: track as u8,
                velocity: resolved.velocity,
                offset: u8::try_from(offset % 24).unwrap_or(0),
                lanes: resolved.lanes,
                locked: resolved.locked,
                velocity_locked: resolved.velocity_locked,
            };

            // This fired trig replaces any active train: hits scheduled at
            // or after the trig's pulse are truncated; earlier hits in this
            // window still play.
            self.flush_train_hits_before(track, offset, out);
            self.track_state[track].train = None;

            if offset >= 24 {
                // Displaced past this window: spills (at most one window
                // deep) into the next one via the pending buffer.
                self.track_state[track].pending.push(event);
            } else {
                out.push(event);
            }

            if let Some(retrig) = step.retrig {
                let interval = retrig.rate.interval();
                let total = match retrig.length {
                    RetrigLength::Infinite => None,
                    RetrigLength::Pulses(p) => Some(p.get() / interval + 1),
                };
                self.track_state[track].train = Some(ActiveTrain {
                    interval,
                    emitted: 1, // the trig's own event is hit 0
                    total,
                    next_offset: offset + i16::try_from(interval).unwrap_or(i16::MAX),
                    velocity: resolved.velocity,
                    vel_ramp: retrig.vel_ramp(),
                    lanes: resolved.lanes,
                    locked: resolved.locked,
                    velocity_locked: resolved.velocity_locked,
                });
            }
        }
    }

    /// Emits the active train's hits that fall inside the current window,
    /// then re-bases the train to the next window (or retires it).
    fn emit_train_hits(&mut self, track: usize, out: &mut TickOutput) {
        let Some(mut train) = self.track_state[track].train else {
            return;
        };
        while train.next_offset < 24 && !train.done() {
            out.push(Event {
                track: track as u8,
                velocity: train.hit_velocity(train.emitted),
                offset: u8::try_from(train.next_offset).unwrap_or(0),
                lanes: train.lanes,
                locked: train.locked,
                velocity_locked: train.velocity_locked,
            });
            train.emitted += 1;
            train.next_offset += i16::try_from(train.interval).unwrap_or(i16::MAX);
        }
        train.next_offset -= 24;
        self.track_state[track].train = (!train.done()).then_some(train);
    }

    /// Emits the active train's remaining hits strictly before pulse
    /// `limit` of the current window — the truncation rule when a new trig
    /// replaces the train mid-window.
    fn flush_train_hits_before(&mut self, track: usize, limit: i16, out: &mut TickOutput) {
        let Some(mut train) = self.track_state[track].train else {
            return;
        };
        while train.next_offset < limit.min(24) && !train.done() {
            out.push(Event {
                track: track as u8,
                velocity: train.hit_velocity(train.emitted),
                offset: u8::try_from(train.next_offset).unwrap_or(0),
                lanes: train.lanes,
                locked: train.locked,
                velocity_locked: train.velocity_locked,
            });
            train.emitted += 1;
            train.next_offset += i16::try_from(train.interval).unwrap_or(i16::MAX);
        }
        self.track_state[track].train = Some(train); // caller replaces it
    }

    /// Evaluates a trig condition for `track`, applying the state-write
    /// rules: `Always`/`Pre`/`Nei` are transparent; everything else records
    /// its result for `Pre`/`Nei` to read. A random draw is consumed only by
    /// `Percent`. `loop_count` is passed in (rather than read) because a
    /// wrapped negative-micro examination evaluates against the *next* loop.
    fn eval_condition(&mut self, track: usize, condition: Condition, loop_count: u32) -> bool {
        let (passed, writes_state) = match condition {
            Condition::Always => (true, false),
            Condition::Percent(p) => (self.rng.next_f32() < p.get(), true),
            Condition::Fill => (self.fill, true),
            Condition::NotFill => (!self.fill, true),
            Condition::Pre => (self.track_state[track].last_cond, false),
            Condition::NotPre => (!self.track_state[track].last_cond, false),
            Condition::Nei => (track > 0 && self.track_state[track - 1].last_cond, false),
            Condition::NotNei => (!(track > 0 && self.track_state[track - 1].last_cond), false),
            Condition::First => (loop_count == 0, true),
            Condition::NotFirst => (loop_count > 0, true),
            Condition::Ratio { a, b } => {
                // Defensive clamps: the fields are public, and no input may
                // panic (see `Condition::Ratio` docs).
                let b = b.clamp(1, 8);
                let a = a.clamp(1, b);
                (loop_count % u32::from(b) == u32::from(a) - 1, true)
            }
        };
        if writes_state {
            self.track_state[track].last_cond = passed;
        }
        passed
    }
}
