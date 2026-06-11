## Decision (2026-06-11)

Adopted **64 lanes** *and* the Elektron-faithful **pattern-level lock pool**
(`MAX_LOCK_LANES = 80` distinct (track, destination) entries per pattern) —
reversing the "rejected alternative" in the analysis below. With banks held
in RAM and snapshot-per-edit undo planned for performance mode, the ~8×
pattern-size reduction (and ~24× vs. dense storage at 64 lanes) was judged
worth the API-can-fail cost (`LockError::PoolFull`), which is also exactly
the hardware behavior. Events stay dense — a full resolved snapshot per
fired trig, sized by the worst case (`tick()` is allocation-free), with an
`Event::locked` bitmask + `velocity_locked` flag carrying lock provenance
for delta-style hosts. See spec.md, Slice 12.

The analysis below is kept as written, for the reasoning that led here.

---

## What Digitone II / Digitakt II actually let you p-lock

On Elektron boxes, essentially **every encoder on every parameter page is p-lockable**, and the page layout is the right mental model for counting destinations per track:

| Page    | Digitone II (FM Tone machine)                                                                                                  | Digitakt II                                             |
| ------- | ------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------- |
| SYN/SRC | ~32 params across 4 pages (algo, 4 operator ratios + offsets, levels, harmonics, detune, feedback, mix, operator envelopes...) | ~8–10 (tune, play mode, slice/start/length/loop, level) |
| FLTR    | ~10–14 (multimode filter + envelope, plus the base-width second filter)                                                        | ~10–14 (same two-filter layout)                         |
| AMP     | ~8 (attack, hold, decay, sustain, release, pan, volume)                                                                        | ~8                                                      |
| FX      | ~6–8 (bit reduction, SRR, overdrive, delay/reverb/chorus sends)                                                                | ~6–8                                                    |
| LFO     | 3 LFOs × ~8 params ≈ 24                                                                                                        | 3 LFOs × ~8 ≈ 24                                        |

So a Digitone II track has on the order of **90–100 distinct lockable destinations**. The interesting wrinkle: Elektron doesn't store a dense slot per parameter per step either — patterns have a cap on *distinct locked parameters per pattern* (80 on Digitakt II; the same order on the others). The hardware model is really "any of ~100 destinations, up to ~80 of them locked anywhere in a pattern," i.e. a sparse pool, not an array.

## Budgeting your list

- ADSR envelope: 4 — but note the hardware has *two-plus* envelopes per voice (amp + filter, plus operator envelopes on FM). Realistically 8–12 if you want filter env too.
- FM synth: 2–8 core (ratios, levels, feedback, algo, harm, detune, mix) — DN2 spends ~32 here, but 8 covers the playable core.
- Other synth params: filter freq/reso/type/env depth, pan, drive ≈ 8.
- FX: BR, SRR, overdrive, three sends ≈ 6–8.
- Sample params: tune, start, length, loop, slice, mode, level ≈ 7–8.

That's **~36–44 lanes** before LFOs. If you want LFO settings p-lockable (very much an Elektron move — locking LFO depth or speed per step is a classic trick), add ~16–24. Call it **~50–65 for full Digitone-2-style coverage**.

## So: how much headroom?

18 is clearly too small for this ambition — it covers one synth voice's playable core and nothing else. My recommendation: **64**.

- It matches the hardware's own order of magnitude (their per-pattern cap is 80 distinct destinations).
- It fits a `u64` bitmask exactly, which matters because at 64 lanes the current `[Option<UnitValue>; N]` representation gets heavy: ~520 B/step → **~1.1 MB per pattern** with full lock storage. Fine on desktop, but this is the point where the bitmask-plus-values representation the spec already footnotes (`u64` mask + `[f32; 64]` = 264 B/step, ~540 KB/pattern) stops being a someday-optimization and becomes the natural encoding.
- Engine-side, nothing changes semantically — lanes stay generic and uninterpreted, velocity stays distinguished.

The alternative worth naming and rejecting: going fully Elektron-faithful with a sparse per-pattern lock pool (any `u8` lane index, capped at ~80 distinct locks per pattern). That's what you'd do on embedded hardware, but it complicates the API (lock can *fail* when the pool is full) for memory you don't need to save on desktop. A const-generic `N` is also possible but infects every type signature and makes serde messier for little gain.

If 64 feels like more surface than you want to expose in a UI, the practical floor for your stated list is 48 — but since the cost difference is small and 64 buys the clean bitmask, I'd go 64. Want me to make the change (constant, spec text, memory notes, and regenerate the golden snapshot)?
