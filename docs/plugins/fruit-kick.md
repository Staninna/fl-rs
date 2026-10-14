# Fruit Kick: state, parameters, voice model

Recovered from `Fruit Kick_x64.dll` (FL Studio 2026, Delphi, image base
0x400000, no engine DLL). Ghidra project `ghidra/proj`, decompiles under
`ghidra/out/kick/vmt/` (the SDK slots with callees). One Ghidra run after the
import; the articulation unit it shares with 3x Osc was read from the 3x Osc
decompiles (`ghidra/out/osc3/voice`, `artic`), and the host services from
`ghidra/out/fruityhost/`. Corpus: 3 uses in 3 of the 165 `.flp` (2 blocks of
32 bytes, 1 of 400), 11 shipped presets of 32 bytes in 9 `.fst`.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`, `asm`.

## How the plugin is wired

`CreatePlugInstance` @ `0x0072d110` -> constructor `FUN_0072d860`; class
reference `*0x0072cf30` -> VMT `0x0072cff8` (`TTestPlug`, instance size
0x188, parent `TDelphiFruityPlug`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x0072d160 |
| 2 | Idle_Public | 0x0071b7a0 |
| 3 | SaveRestoreState | 0x0072d690 |
| 4 | GetName | 0x0072e1d0 |
| 5 | ProcessEvent | 0x0072dc60 |
| 6 | ProcessParam | 0x0072dce0 |
| 8 | Gen_Render | 0x0072e750 |
| 9 | TriggerVoice | 0x0072e2f0 |
| 10 | Voice_Release | 0x0072e4b0 |
| 11 | Voice_Kill | 0x0072e4d0 |
| 12 | Voice_ProcessEvent | 0x0072e510 |
| 13 | Voice_Render | 0x0072e520 |
| 14 | NewTick | 0x0072e840 |

`TFruityPlugInfo` @ `0x007849e8`: SDK 1, "Fruit Kick" (both names), Flags
`0x11600031` (Generator, GetNoteInput, WantNewTick, NewVoiceParams, bits 22,
24, 28), NumParams 99 (6 own + 93 of the instrument tab). Like 3x Osc it is
not a sampler hybrid: `Gen_Render` renders and articulates its own voices.

| offset | meaning |
|---|---|
| 0xbc | host; `+0x18` WaveTables[0] is the sine table the kick reads, `+0x70` TempBuffers[1] its mono scratch |
| 0xe4 | f32 base increment = `2247346493527.166 / SR` (`0x0072d530`) |
| 0x120 | i32[6] parameters |
| 0x134 | = p5 (distortion), tested for > 0 |
| 0x138 | i32 minimum increment, from p1 |
| 0x13c | f32 amplitude decay factor per sample |
| 0x140 | f32 frequency decay factor per sample |
| 0x144 / 0x148 | f32 dry = 1 - d, wet = d * 3.3 with d = p5 / 128 |
| 0x14c | voice list |
| 0x154 | f32 `44100 / SR` (`0x0072d528`) |
| 0x158 | sine table pointer (WaveTables[0]) |
| 0x160 | articulation voice manager (holds `Voice_Render` at `+0x30`) |
| 0x168 | articulation settings (same 0x338-byte object as 3x Osc) |
| 0x178 | an articulation block was loaded |

## 1. State layout (event 213: 32 bytes; 400 with the instrument tab)

`SaveRestoreState` (`FUN_0072d690`):

```
save:    Write(i32 1); Write(buf, 0x18c)         // 24 bytes params, 4 unused, 364 articulation, 4 unused
restore: Read(i32 v); v >= 2: refuse
         v == 0: zero the 6 params; Read(this+0x120, 0x1c)        // 6 params plus the stale minimum increment at 0x138
         v == 1: Read(buf, 0x18c, &got); params from the first 24 bytes; if got - 24 > 363: load the articulation block (FUN_00721c70)
         GetValue/UpdateValue loop over all 99 params (recomputes 0x138, 0x13c, 0x140)
```

Every shipped preset and two of the three corpus blocks are version 0; FL
2026 writes version 1 (400 bytes). The 7th int of a version-0 block is the
minimum increment the saving FL computed (section 2): the loader reads it into
`0x138` and the parameter loop overwrites it.

| byte | type | idx | parameter (DFM object, hint) | range | default | presets | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version 0 or 1 | | 1 | 0 x11 | loader, corpus, preset |
| 4..7 | i32 | 0 | Max freq (`MaxFreqWheel`, no Tag), cents | -2400..3600 | 0 | 0, -2400, 512, -304, 624, -1216, 3600 | loader, dfm, preset |
| 8..11 | i32 | 1 | Min freq (`FreqMinWheel`), cents | -1200..1200 | -600 | -600, -432, -1200, -832, -8 | loader, dfm, preset |
| 12..15 | i32 | 2 | Freq decay (`FreqSpeedWheel`) | 0..256 | 64 | 64, 176, 43, 96, 143, 42, 0, 158 | loader, dfm, preset |
| 16..19 | i32 | 3 | Amp decay (`DecayWheel`) | 0..256 | 128 | 128, 133, 91, 127, 86, 0, 256 | loader, dfm, preset |
| 20..23 | i32 | 4 | Click (`ClickWheel`) | 0..64 | 32 | 32, 30, 64, 25, 0 | loader, dfm, preset |
| 24..27 | i32 | 5 | Distortion (`DistWheel`) | 0..128 | 0 | 0, 128, 20 | loader, dfm, preset |
| 28..31 | i32 | | v0: stored minimum increment at 44.1 kHz; v1: unused (0) | | | 0x44bae3 (Default) | loader, preset |
| 32..395 | | | v1 only: 5 x 68-byte envelope/LFO blocks (pan, volume, mod X, mod Y, pitch), 8 unused bytes, cutoff, resonance, 4 unused, filter type; layout in `plugin-3xosc.md` section 1 | | | | loader, corpus |
| 396..399 | | | v1 only: unused | | | 0 | loader |

Check on the stored increment: Default has p1 = -600, so the floor is
`-600 - 3600 = -4200` cents and `round(2247346493527.166 / 44100 * 2^(-3.5)) = 4504291 = 0x44bae3`,
the value in `Default.fst`. The constant and the 3600-cent offset are
therefore `preset`-confirmed.

Preset check (`.fst` name "Fruit Kick"; frequencies with 0 cents = 523.25 Hz,
see section 4 for the host's note mapping):

| preset | max | min | freq decay | amp decay | click | dist | reading |
|---|---|---|---|---|---|---|---|
| Default | 0 | -600 | 64 | 128 | 32 | 0 | start 1047 Hz at pitch 0, floor 46 Hz; pitch falls with factor 0.9997 - 0.00049 per sample (section 2) |
| Classic | -2400 | -432 | 176 | 128 | 30 | 0 | start 262 Hz, floor 51 Hz, slower sweep |
| Low | -2400 | -832 | 143 | 127 | 0 | 0 | floor 41 Hz, no click: the sine starts at phase 0 |
| High | -304 | -1200 | 96 | 91 | 25 | 128 | floor 33 Hz, full distortion, faster amplitude decay |
| Punchy | 624 | -8 | 42 | 86 | 64 | 20 | floor 66 Hz, maximum click: the sine starts a quarter cycle in, at its peak |
| Pure click | -1216 | -1200 | 0 | 0 | 64 | 0 | decay factors 0.99 and 0.995 per sample: gone in a few ms |
| SciFi | 3600 | -1200 | 158 | 256 | 0 | 0 | start clamped at 4800 cents (8.4 kHz), amplitude factor 0.99998: over a second of sweep |

## 2. Parameter semantics (`ProcessParam` @ 0x0072dce0)

Indexes 6..98 go to the instrument tab handler (`this+0x170`). For 0..5 the
int is stored and, with `r = 44100 / SR` (`this+0x154`) and `pow(a, b)` from
`FUN_005ea930`:

```
idx 0, 1  minInc  = round(base * 2^((p1 - 3600) / 1200))                 // FUN_0072dcb0 -> 0x138; the floor is 3 octaves under the knob
idx 2     freqDec = (0.9997  - (1001^(1 - p2/256) - 1) * 9.7e-6) ^ r      // 0x140: p2 = 0 -> 0.99, p2 = 256 -> 0.9997
idx 3     ampDec  = (0.99998 - (1001^(1 - p3/256) - 1) * 4.98e-6) ^ r     // 0x13c: p3 = 0 -> 0.995, 128 -> 0.99985, 256 -> 0.99998
idx 5     Host lock (slot 32); dry = 1 - p5/128; wet = (p5/128) * 3.3; Host unlock (slot 33)
```

Constants: `0x0072e078` 1001.0, `0x0072e080` 1.0, `0x0072e088` 0.99998,
`0x0072e098` 4.98e-6, `0x0072e0a0` 0.9997, `0x0072e0a4` 9.7e-6, `0x0072e0a8`
3.3, `*0x00784df8` 1/256, `*0x00785b00` 1/128. Display (`FUN_0072e140`):
every value as `p / Max` of its wheel.

Dispatcher ID 4 (sample rate) sets `0x154` and `0xe4` and reruns the
parameter loop, so the decay factors follow the rate.

## 3. Voice model

`TriggerVoice(params, tag)` (`FUN_0072e2f0`; `FUN_0072e400` first releases the
oldest voice at the polyphony limit), 0x24-byte voice record:

```
rec+0x00 tag;  rec+0x08 params
rec+0x10 phase (u32)     = click << 24                                   // 0..64 -> 0..1/4 cycle: the sine starts up to its peak
cents  = clamp(p0 + params.FinalLevels.Pitch * 0.5 + 1200, p1 - 3600, 4800)   // *0x00785c50 = 0.5, 0x0072e3f4 = 1200, 0x0072e3f8 = 4800
rec+0x14 increment (f32) = round(base * 2^(cents / 1200))               // FUN_0071b300
rec+0x18 amplitude (f32) = 1.5
rec+0x1c articulation voice (FUN_00720d40)
```

The key tracks the start pitch at half rate (50 cents per semitone) and the
floor not at all: a kick an octave up starts 6 semitones higher and lands on
the same note.

`Voice_Render(rec, dest, &len)` (`FUN_0072e520`), `tmp = Host.TempBuffers[1]`:

```
for n in 0..len-1:
    tmp[n] = sine[phase >> 18] * amp            // 16384-point host sine table
    phase += round(inc)
    amp   *= ampDec
    inc    = inc > minInc ? inc * freqDec : minInc
if p5 > 0: Host.slot24(0, 10, tmp, len, dry, wet, 0.1)      // waveshaper table 10 of the host's set 0 (FUN_0132adc0), mix dry + wet * shaped
dest[n] = (tmp[n], tmp[n])                      // FUN_005f89d0, mono to stereo
return amp < 2e-4                               // 1 = voice finished (0x00784a88)
```

The host waveshaper (`FUN_0132adc0`) indexes an 8192-entry table at
`*0x015e4040 + 10 * 0x4004 - 0x4002` through a SIMD routine with the three
scalars; the table is generated at start-up and was not read (section 4).

Around it the articulation unit does what `plugin-3xosc.md` sections 3.2 and
3.3 describe: envelopes and LFOs stepped per tick, pitch modulation written
into `params.FinalLevels.Pitch` (which this plugin only reads at trigger, so
the pitch envelope has no effect on a running kick), the shared filter, pan
law and ramped mix into the manager's buffer, then `Gen_Render`
(`FUN_0072e750`) adds that buffer to the host's and kills finished voices
(host slot 5). Finished means the volume envelope is done *or* the amplitude
fell under 2e-4, so a kick with the default (disabled) volume envelope ends
itself.

`Voice_Release` -> `FUN_00720d90` (release the envelopes); `Voice_Kill` ->
`FUN_00720e80`, remove from the list, free. `Voice_ProcessEvent` returns 0.
`NewTick` -> `FUN_00720f80` (advance the global LFO phases).

## 4. What stays unknown

- The host's table-10 waveshaper curve and the meaning of the third scalar
  (0.1); the table lives in `.bss`.
- Which note gives `FinalLevels.Pitch = 0` (the plugin side is exact: 0 cents
  is 523.25 Hz).
- The pan law and filter kernels of the articulation unit (CPU-dispatched).
- Dispatcher IDs other than 4 and the ones listed for 3x Osc were not read;
  Fruit Kick's `FUN_0072d160` follows the same shape.

## Pitch reference

`FinalLevels.Pitch = 0` is MIDI 60 (FL C5); see `docs/plugins/host-voice.md` for the host side of the voice path.

## Corrections from the implementation (crates/plugins/src/fruit_kick.rs)

- Increments truncate from an f32 base; five presets disagree with `round`.
- Default amp decay is 0.99983, not 0.99985.
- The v1 articulation block is parsed but not played yet (one corpus state); reported as Approximate.
