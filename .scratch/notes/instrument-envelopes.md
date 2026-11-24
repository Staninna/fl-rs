# Instrument envelopes, tracking and echo delay: what is left

Built on branch `worktree-agent-afa284bf3105a43e4` (2026-11-24):
`crates/engine/src/instrument_env.rs`, `crates/engine/src/echo.rs`, the
tracking scale in `crates/engine/src/levels.rs`, hooks in `voice.rs` and
`graph.rs`. `docs/rec-channel.md` has the reading, with addresses, in its
last four sections. The render no longer counts these three parts.

Left, roughly by what it would change:

- A clip on a sampler's envelope controls (0x1000..0x17FF). FL works the
  record out again as the clip moves it (`FUN_009ce9e0` from the REC
  dispatcher) and its sounding voices read the record live. The engine
  reads the 218 once per voice, so it would need the clip's value at each
  tick handed to the voices' `Shape` (an `Arc` per voice now). No corpus
  clip drives one on a sampler; the one corpus binding (0x1408 in
  `Treben - Gimbal and Sinan - RawFL`) is on a 3x Osc channel and plays
  through the tab's parameter.
- Ticks under a moving tempo. A voice's ticks fall where `round((tick + k)
  * samples per tick)` does at the tempo its note starts at; with a clip
  on the tempo they drift from the clock's. The record's tick times use
  that same tempo, where FL recomputes every record when the tempo moves
  (`FUN_00fe9b50` calls `FUN_009ce9e0`).
- The pan LFO keeps the level and pan the voice started with; a clip on
  the channel's volume or pan does not reach such a voice.
- Echoes: an echo's tracking is its note's, where FL tracks each echo's
  own `InitLevels`; the meaning of the note's `+0x44`, which fat mode
  bumps by 2 an echo, is not read; the voice condition `voice+0xa4 == 0`
  at `0xd2975e` is not read either.
- Plugin channels: the reading is that the host never runs these records
  on a generator that renders its own voices (`chan+0x164` 2,
  `FUN_012bd640`). Which plugins set `FPF_UseSampler` (and so get
  `chan+0x164` 1) was not checked plugin by plugin.
- The sign of the key tracking's middle offset (`chan+0x1dc = (middle -
  60) * 100`, added) is as read but not borne out: every corpus channel
  has the key middle at 60.
- The Fast LP still moves over 512 samples, not a tick, when the
  envelopes set it each tick (`channel_filter.rs`).
