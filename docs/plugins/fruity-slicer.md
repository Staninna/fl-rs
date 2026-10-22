# Fruity Slicer: state, slices, key map, voice

Recovered from `Fruity Slicer_x64.dll` (FL Studio 2026, Delphi, image base
0x400000). Ghidra project `/root/ghidra/slicer-gms/proj/slicer`; decompiles
under `/root/ghidra/slicer-gms/out/slicer/` (`slots/` the SDK slots and the
constructor, `load/` what runs after a state loads and the sample voice,
`art/`, `dc/`, `env/` the articulation unit and its declicker, `dk/` the
slice declicker). Corpus: 13 states in 6 `.flp` (versions 15 and 16) and 47
in `.fst` presets, 60 in all; every one reads to its last byte.

Confidence tags: `loader`, `asm`, `corpus`, `inferred`.

## How the plugin is wired

`TTestPlug` (VMT `0x766188`, 0x3d8 bytes), built by `FUN_00767770` from
`CreatePlugInstance` (`0x007663c0`).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x00766560 (`0xe` FPD_SetTimeSig: PPQ to the envelope unit, `FUN_00755790`) |
| 3 | SaveRestoreState | 0x00766c40 |
| 6 | ProcessParam | 0x00767d80 (forwards to the articulation's parameters) |
| 8 | Gen_Render | 0x0076c8b0 |
| 9 | TriggerVoice | 0x00768230 |
| 10 | Voice_Release | 0x007684d0 -> `FUN_00756d80` -> `FUN_007579d0` |
| 11 | Voice_Kill | 0x007684f0 |
| 13 | Voice_Render | 0x00768680 -> `FUN_007685f0` |

Plugin object: `+0x120` the header floats and words, `+0x13c..+0x141` the
switches, `+0x144` the root key, `+0x148` the path, `+0x158` the loaded
`TSample` (`+0x70` its rate as f64, `+0x80` its data, `+0x88` its length),
`+0x190` the slices (0x34 bytes each), `+0x198` the key map (125 i32),
`+0x3b0` the articulation's voice manager, `+0x3b8` its settings.

## 1. State layout (event 213)

`FUN_00766c40`, load side (save side writes version 16 and the same fields):

```
i32 version                 FL 2026 writes 16; read up to 16
f32 beats       +0x120      below 12 an i32; below 7 times 4
f32 tempo       +0x124      the loop's own tempo; below 8 an i32
i32 pitch       +0x128      version 9 on: the stretcher's pitch knob, cents
i32 time        +0x12c      version 9 on: the time knob, 2^(time / 10000) the stretch
i32 method      +0x130      version 9 on: stretching method; <2 BeatSlicer gap filling, >=2 Elastique
i32 attack      +0x134      ms, every slice's fade in
i32 decay       +0x138      ms, every slice's fade out
string path     +0x148      u8 length (0xff: then u32), UTF-8; trailing control chars go (FUN_0074b2d0)
i32 n
n x slice (0x34 bytes in memory):
    string name   +0x00
    i32 start     +0x08     frames, counted at the rate below
    version 13 on:
    i32 key       +0x0c     -1: after the root
    f32           +0x10     -1.0 (FUN_0076c1d0 default), not read for playing
    u8 reversed   +0x14
    versions 6 to 12: u8 reversed only
u8 animate      +0x141
i32 root        +0x144      version 2 on; overwritten on load (section 3)
u8 play to end  +0x13e      version 3 on
i32 rate                    version 4 on: the sample's rate when saved
u8 auto-dump    +0x13f      version 5 on
u8 fit                      versions 5 to 8
u8 method                   versions 6 to 9 (old method); fit and version < 9: time = 10000
u8 declick      +0x13c      version 10 on
u8 auto-fit     +0x140      version 11 on
i32 view                    version 14 on: the GUI's selected slice
version < 15 and method > 1: method += 2
0x16c bytes articulation    version 16: five envelopes of 0x44 bytes (FL's channel ENV/LFO layout) and six words
```

Corpus: method 4 everywhere, pitch 0, time 0 but in five states (766, 167
and 83: the auto-fit to the project's tempo, so Elastique runs), attack 0,
decay 4, declick on but in one, play to end off.

The articulation block (`FUN_00757c60` copies it into five 0xa0-byte
records, `FUN_00755d80` derives the rates): per envelope `+0` flags (bit 0
tempo-free time, bit 2 start at full), `+4` on, `+8` delay, `+0xc` attack,
`+0x10` hold, `+0x14` decay, `+0x18` sustain (of 128), `+0x1c` release,
`+0x20` amount, LFO words, `+0x38..+0x40` attack, decay and release
tension. Envelope 1 is volume. Every corpus state has the same block: only
the volume envelope on, attack/hold/decay 100 (none), sustain 128, release
6500, release tension -101.

## 2. The sample after loading

`FUN_007686d0` with mode 0 (a state), then:

1. Starts times `TSample.rate / saved rate`, rounded (`FUN_007686d0`).
2. `FUN_00769900`: slices from the first that starts at or past the end go;
   an empty list with a sample gets one slice at 0. Lengths `+0x20` and
   `+0x28`: the next start minus this one (at least 1), the last to the end.
   Root: 60, minus 12 while `root - 1 + n >= 125` and `root >= 12`.
   `FUN_0076c200`: key map, every entry -1, then slice `i` on its key
   (`FUN_0076b4f0`: `+0xc`, or root + `i` when below 0, clamped 0..124);
   a later slice on the same key wins.
3. `FUN_00769cd0`: reversed slices are reversed in place (`FUN_00607a40`).
4. `FUN_0076a790`, with declick: `FUN_00608530(start, end, 1, 0.7)` ->
   `FUN_0060d020`: over `round(rate * 0.04 * 0.7)` frames (28 ms) at the end
   of the slice, `FUN_0060cdd0`:

   ```
   pass over the zeros at the end
   n = min(28 ms, what is left); m = min(n, round(rate * 0.001))
   for i = 1..n (forward to the end):
       k = min(1, 2 / rate * exp(i * ln(22050) / (ln 2 * n) * ln 2))   // 0x7db440 = ln 2 (asm, 0x006117e1)
       dc += k * (x - dc)
       w = 1, or over the last m: (cos((i - (n - m)) / (m + 1) * pi) + 1) / 2
       x = (x - dc) * w
   ```
5. `FUN_0076aa20`, when time or pitch is set: ratio `2^(time / 10000)`
   (`FUN_00766300`); the sample is `round(len * ratio)` long, starts are
   `round(start * ratio)`, lengths recomputed. Method < 2 with a time:
   `FUN_0075b5d0` fills the gaps (BeatSlicer); otherwise Elastique
   (`FUN_007520c0` opens it, `FUN_00752210` stretches each slice from its old
   length to its new one, pitch `2^(pitch / 1200)`), then step 4 again.
6. `FUN_0076a830`, when attack or decay is set: per slice
   `FUN_00607ed0` fades. In: `round(attack * rate / 1000)` frames (at most
   the slice), gain 0 to 1, kind 0x102; out: the last
   `round(decay * rate / 1000)` frames, 1 to 0, kind 0x103 with bend 10. With
   `x = (j + 1) / (len + 1)`: kind 2 `x`, kind 3 `(11^x - 1) / 10` (falling:
   mirrored), kind 4 `(cos((1 - x) pi) + 1) / 2`; flag 0x100 turns a fade of
   50 ms or less into kind 4. Gain = from + shape * (to - from).

## 3. A note

`FUN_00768230`: `key = round(InitLevels.Pitch * 0.01)` (0..124, else no
voice), slice = key map. A `TSampleVoice` (`FUN_005fdb80`, set up by
`FUN_005fe7f0`) reads from the slice's start for `max(len - 1, 1) + 1`
frames, `len` the slice's length or, with play to end, the sample's end
less the start. Voice pitch offset `(60 - key) * 100`, so
`FUN_007685f0` -> `FUN_005fe560`: speed = `2^((FinalLevels.Pitch + offset) /
1200) * TSample.rate * (1 / output rate)`: the file's own pitch moved by the
note's fine pitch. `FUN_005feed0` reads with FL's interpolator
(`docs/plugins/fl-interpolator.md`; constructor `FUN_005fdb80` and region
setter `FUN_005fe7f0` are Slicex's and Fruity Convolver's class). The voice
gives it the slice's data from its start and frames `0 ..= max(len - 1,
1)`, so the kernel's taps past the slice's ends hold the slice's own end
frames; it sets `+0x77` and no quality, so the unit's, which the host sets
through `FPD_ProcessMode`, applies (`FUN_005fde70`), as in FPC.

The articulation unit (`InstrVoice`, `FUN_00757080` a voice): each
`Gen_Render` it ticks the envelopes when the host's tick flag is set
(`FUN_00756cd0`, `FUN_00757260` -> `FUN_00757840` -> `FUN_00755830`), then
`FUN_007572a0` multiplies the voice's stereo block by
`ComputeLRVol(FinalLevels.Pan + pan env, FinalLevels.Vol * volume env)`
(`FUN_0060f9d0`, the circular law, 0.7071 a side in the centre), ramped
linearly from the last block's gains, and runs the declicker
(`FUN_00754730`).

Volume envelope (`FUN_00756140` start, `FUN_00755830` per tick): with flag
bit 2 and attack 100 it starts at 1, hold 0, decay to sustain 1. Times:
`FUN_00755cf0`: 0 at or below 100, else
`round((1001^(v / 65536) - 1) * unit)` ticks, `unit = (PPQ/4) / 48 * 24576 /
1000`, times `tempo / 120` unless flag bit 0 (`FUN_00755d80`); a tick is one
of the host's PPQ ticks, so a time is `(1001^(v/65536) - 1) * 64 ms`.
Release (state 6, `FUN_00756060` starts it at `x` with `curve(x)` = the
level): each tick `x -= 1 / (ticks + 1)`, level = `curve(x)`; tension `t`
(`FUN_00755c30`): `T = sign(t) (6347^(|t|/128) - 1) * 0.1`; t > 0:
`1 - (exp((1 - x) ln(|T| + 1)) - 1) / |T|`, t < 0:
`(exp(x ln(|T| + 1)) - 1) / |T|`. At a level of 5.96e-8 or less the state is
9 and the declicker's release starts.

Declicker (`FUN_00756a20` -> `FUN_007545c0`): attack mode 0, release mode 3
of 10 ms (`FUN_00753ee0`): frame `i` of `n = round(0.01 rate)` at
`(1 + cos(i pi / (n + 1))) / 2`, then silence; the voice ends when it is
through (`FUN_00757ab0`) or the sample is (`FUN_007685f0`). A note off with
the volume envelope off goes straight to it (`FUN_007579d0` ->
`FUN_00757a50`).

## What the Rust port does

`crates/plugins/src/fruity_slicer.rs`: all of sections 1 and 2 but
Elastique and the gap filler (a stretched slice is the old one cut or padded
with silence to its new length), and section 3 with the release and the
declick per frame rather than per tick and block ramp, and FL's interpolator
at its default render quality, 16 taps a side (inferred, see
`docs/plugins/fpc.md`). The other envelopes, the LFOs and the volume
envelope's attack, hold and decay are read and not run; fidelity says so
when a state has them on.
