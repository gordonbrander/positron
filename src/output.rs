//! Tick output: the events fired during one step window.

use crate::NUM_PARAM_LANES;

/// Maximum number of events a single tick can emit.
///
/// Capacity proof, per track per window: at most 1 spilled event arrives
/// from the previous window (only one of any two adjacent steps is
/// odd-indexed, and only swing on odd steps can push an offset past the
/// window). Within the window, up to 2 step events plus train hits at the
/// minimum interval of 4 pulses; even with a mid-window train replacement
/// the trigs and hits interleave to at most 8 events (e.g. 6 old-train hits
/// before a pulse-23 trig collision). 9 × 16 tracks = 144 ≤ 160.
pub const MAX_EVENTS_PER_TICK: usize = 160;

/// A fired trig, stamped with its pulse offset inside the tick's step window.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Event {
    /// Which track fired (`0..16`).
    pub track: u8,
    /// Velocity in `0.0..=1.0`.
    pub velocity: f32,
    /// Pulses after the window start (`0..24`). The host converts to time:
    /// one pulse = `step_duration / 24`.
    pub offset: u8,
    /// Resolved parameter-lane values, each in `0.0..=1.0`: per lane, the
    /// step's lock if set, else the track default. The engine attaches no
    /// meaning to these; mapping and scaling are host concerns.
    pub lanes: [f32; NUM_PARAM_LANES],
}

impl Event {
    pub(crate) const EMPTY: Self = Self {
        track: 0,
        velocity: 0.0,
        offset: 0,
        lanes: [0.0; NUM_PARAM_LANES],
    };
}

/// Fixed-capacity list of the events scheduled in one step window.
///
/// Returned by value from `Sequencer::tick()`; never heap-allocates.
#[derive(Clone, Debug)]
pub struct TickOutput {
    events: [Event; MAX_EVENTS_PER_TICK],
    len: usize,
}

impl TickOutput {
    pub(crate) fn new() -> Self {
        Self {
            events: [Event::EMPTY; MAX_EVENTS_PER_TICK],
            len: 0,
        }
    }

    /// Appends an event. By construction a tick can never exceed capacity
    /// (see [`MAX_EVENTS_PER_TICK`]); this is debug-asserted, and in release
    /// builds an overflowing event would be dropped rather than panic.
    pub(crate) fn push(&mut self, event: Event) {
        debug_assert!(self.len < MAX_EVENTS_PER_TICK, "TickOutput overflow");
        if self.len < MAX_EVENTS_PER_TICK {
            self.events[self.len] = event;
            self.len += 1;
        }
    }

    /// Stable insertion sort by `(offset, track)`: same-key events keep
    /// their push order (pending, step W, step W+1, train hits). n ≤ 128,
    /// in place, allocation-free.
    pub(crate) fn sort(&mut self) {
        for i in 1..self.len {
            let item = self.events[i];
            let mut j = i;
            while j > 0
                && (self.events[j - 1].offset, self.events[j - 1].track) > (item.offset, item.track)
            {
                self.events[j] = self.events[j - 1];
                j -= 1;
            }
            self.events[j] = item;
        }
    }

    /// The fired events, in `(offset, track)` order.
    pub fn as_slice(&self) -> &[Event] {
        &self.events[..self.len]
    }

    /// Iterates over the fired events.
    pub fn iter(&self) -> std::slice::Iter<'_, Event> {
        self.as_slice().iter()
    }

    /// Number of events fired this tick.
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` if nothing fired this tick.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl PartialEq for TickOutput {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<'a> IntoIterator for &'a TickOutput {
    type Item = &'a Event;
    type IntoIter = std::slice::Iter<'a, Event>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, TickOutput};

    fn ev(track: u8, offset: u8, velocity: f32) -> Event {
        Event {
            track,
            velocity,
            offset,
            ..Event::EMPTY
        }
    }

    #[test]
    fn sort_is_stable_by_offset_then_track() {
        let mut out = TickOutput::new();
        out.push(ev(3, 5, 0.1));
        out.push(ev(1, 0, 0.2));
        out.push(ev(1, 5, 0.3)); // same (offset, track) as the next push
        out.push(ev(1, 5, 0.4)); // must stay after 0.3 (stability)
        out.push(ev(0, 23, 0.5));
        out.sort();

        let got: Vec<(u8, u8, f32)> = out
            .iter()
            .map(|e| (e.offset, e.track, e.velocity))
            .collect();
        assert_eq!(
            got,
            vec![
                (0, 1, 0.2),
                (5, 1, 0.3),
                (5, 1, 0.4),
                (5, 3, 0.1),
                (23, 0, 0.5),
            ]
        );
    }
}
