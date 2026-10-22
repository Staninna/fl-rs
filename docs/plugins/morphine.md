# Morphine: state, parameters, signal chain

Recovered from `Plugins/Fruity/Generators/Morphine/Morphine_x64.dll` (FL
Studio 2026, MSVC, image base 0x180000000). No Delphi shell: the plugin is
Image-Line's `CFLWrapper` around a C++ engine, all inside the one DLL.
Corpus: 4 shipped projects (Africa's Calling, NIGHT TERROR, SHINE, I'll Be
Your Madonna) with 7 Morphine channels, 3 `.fst` presets and 300 of
Morphine's own `.mrp` presets under `Patches/Plugin presets/Generators/Morphine`.
The noise layer reads `Plugins/Fruity/Generators/Morphine/Noise Samples/*.wav`.

Port: `crates/plugins/src/morphine.rs` (engine and voices),
`morphine_state.rs` (state), `morphine_noise.rs` (noise layer),
`morphine_fx.rs` (effects).

## What it is

Four generators each play an analysed sound: a run of frames, each the
amplitude, pan and detune of 128 harmonics, read along at a rate and
looped. A point walking the morph line over the square between the four
weighs their spectra into one, which a bank of 128 sine oscillators plays at
the note's pitch under a master ADSR. Four envelopes and a twelve-route
matrix modulate the rest. A noise sample plays under each voice through a
two-pole filter. The voices' sum goes through overdrive, chorus, delay,
reverb and an eight-band equaliser, then the master level.

## State

Event 213 holds `"SSLF"`, a u32 length and the engine's chunk tree. A
`.mrp` preset is the same chunk without the `"SSLF"` header. Every chunk is
a four-letter tag stored backwards (`"0roM"` for `Mor0`); a child chunk has
its length before it, and the fields an object adds after its children are a
block that starts with its own length (counting itself). A block longer than
the engine knows is cut to what it knows (`FUN_180130440`).

```text
"SSLF" u32 len | "0roM" | "0RPM"                                 FUN_18010f060
  4 x (u32 len, "0CVM" generator)                                FUN_1801135b0
  u32 len, "0LMM" morph line                                     FUN_180102450
  u32 len, "0DMM" envelopes, matrix                              FUN_1800fc8d0
  block 0x5c4: the program's parameters, noise file and name     FUN_18010eb40
"0CVM": u32 len, "0HZM" zones, then block 0x1c                   FUN_1801135b0
"0HZM": u32 n, n x (i32 keys, f32 level, u32 len, "0PSM")        FUN_18012afa0
"0PSM": u32 n, n x (i32 duration, f32 gain, u32 len, "0PBM"),
        then block 0x424                                         FUN_1800ec5d0
"0PBM": three arrays of 128 f32 (amplitude, pan, frequency), each
        sent eight at a time behind a byte whose bit i says value i
        is there; a missing one is 0                             FUN_1800f0390
"0LMM": u32 n, u32 stride, n x (f32 x, f32 y, i32 duration), block 0x54
"0DMM": u32 n (4), n x (u32 len, "0GEM"), block 0x94: twelve sources,
        twelve destinations, twelve amounts                      FUN_1800fc8d0
"0GEM": u32 n, u32 stride, n x (f32 value, i32 duration), block 0x2c
                                                                 FUN_1800f46e0
```

The program block copies engine fields 0x608..0x6b0 in place and the five
after them from its end (`FUN_18010eb40`); `program_offset` in
`morphine_state.rs` lists where each parameter sits. The engine's field per
parameter is `FUN_18010dc50`. Defaults for a fresh engine come from
`FUN_1800ebab0` and `FUN_180113750` (generators) and `FUN_1800c4090`.

## Parameters

The setter is `FUN_180111ec0(engine, index, value)`; values are 0..1 except
the integers the engine keeps as floats. The engine has 0x91 slots, FL
shows 141.

| index | meaning | setter |
|---|---|---|
| 0 | master level, `10^(2.4 v - 1.2) / 2` | case 0, `FUN_1800e9160` |
| 1 | glide time, `v^3` | case 1, `FUN_1800e90a0` |
| 2 | velocity curve, `vel^(5 v)` | case 2 (`5 v`, at least 0) |
| 3 | `int(12 v)` at `this+0x38dc` | case 3; not read by the port |
| 4, 5 | transpose `int(48 (v - 0.5))` semitones, fine `int(200 (v - 0.5))` cents | cases 4, 5 |
| 6 | overdrive, `16 v^3` | `FUN_1800ff7e0` |
| 7..10 | chorus depth, wet, speed, on | `FUN_1800f2960`, `FUN_1800f29c0`, `FUN_1800f28e0`, `FUN_1800f2c90` |
| 11..16 | delay time, stereo offset, depth, feedback, level, on | `FUN_1800f2d80`, `FUN_1800f2d90`, `FUN_1800f2df0`, `FUN_1800f2da0`, `FUN_1800f2e50`, `FUN_1800f2d70` |
| 17..20 | reverb size (comb feedback), damping, mix, on | `FUN_180124d70`, `FUN_180124d00`, `FUN_180124cd0`, `FUN_1801250d0` |
| 21..28 | equaliser gains, 62.5 Hz to 8 kHz an octave apart | `FUN_1800f7db0` |
| 29 | equaliser on | `FUN_1800f7f60` |
| 30..33 | master ADSR attack, decay, sustain, release | `FUN_1800da760`, `FUN_1800da830` |
| 34..42 | noise mix, pan, mute, mono, velocity sense, ratio, cutoff, resonance, key tracking | `FUN_180100300` |
| 0x2b + 10 g | generator g's rate, tracking, loop and PWM settings | `FUN_1800ebab0` |
| 0x7c | polyphony - 1 (0 is mono) | |
| 0x7d | (engine field 0x610) | |
| 0x7e, 0x7f | noise loop mode, filter mode | |

## Engine and voices

| part | address | notes |
|---|---|---|
| control block | `DAT_180177030` | `int(0.0029 * rate)` samples; every block moves the envelopes and ADSR, every third (`this+0xfc`, `this+0x100`) builds the spectrum anew |
| voices | `this+0x38e8` | 32 at most |
| note on | `FUN_1800fffa0` | envelopes restart with gain from velocity (`FUN_1800fbed0`, `FUN_1800f7880`); each generator picks its zone by the transposed key |
| note off | `FUN_180100620` | |
| glide | `FUN_1800e9d90` | from the key last played (`this+0x3990`); nearest held key `FUN_1800e8b70` |
| pitch | `FUN_1800e9e90`, `FUN_1800e9e50` | |
| rates | `FUN_18012cd10`, `FUN_1800fb9d0`, `FUN_180105df0` | `2^(8 (rate + (vel - 0.5)(2 vt - 1)) - 4 + (key - 48)/12 (2 kt - 1))` |
| time units | `DAT_1801772a0`, `DAT_180177128` | hundredths of a second, or `tempo * 0.41667` a second when synced |
| loops | `FUN_1800eb970`, `FUN_180101e20` | modes forward, backward, back and forth; loop end `FUN_1800ebc50` |
| generator spectrum | `FUN_18012c850`, `FUN_1800eef20` | frames interpolated along the zone; partials' LFO `FUN_1800f8460` |
| envelope value | `FUN_1800f3e20` | straight lines, last point held |
| modulation | `FUN_1800fb820`, `FUN_1800fb9d0`, `FUN_1800fbbc0`, `FUN_1800fbff0` | unset destination -1000; parameter plus route kept in 0..1 |
| spectrum and ramps | `FUN_1801001c0`, `FUN_1801054b0` | oscillators aimed at the new spectrum over three blocks; none above `0.4535 rate` (`DAT_180177130`) |
| oscillator bank | `CResynth`, `FUN_1801220c0`, kernels `FUN_180115860` | 128 magic circles `c -= k s; s += k c`, `k` and both gains ramped; silent below 1e-6 (`DAT_180176fe0`) |
| level curve | `FUN_180122ae0` | Catmull-Rom over the ADSR's last four block values, so the amplitude trails the envelope by two blocks |
| ADSR | `FUN_1800da760`, `FUN_1800da830`, `FUN_1800daa00` | decay and release bend `1 - 10^(-2x)(1 - x)` |
| noise layer | `FUN_180100300` (block), `FUN_180100880` (play), `FUN_180113b70`/`FUN_180113df0` (filter), `FUN_1800de050` (file) | cutoff over 9 octaves from 35 Hz, `q = 30 r^2 + 1` |

## Effects

`FUN_1800ff630` runs the effects on the voices' sum in place, in this order.
The object (`param_1[0xc16]` of the engine) is made by `FUN_1800ff520` and
sized for the rate by `FUN_1800ff570`; `FUN_1800ff5d0` resets it.

| part | object | process | notes |
|---|---|---|---|
| overdrive | `+0x570` target, `+0x574` now | `FUN_1800ff800` | drive smoothed 0.01 a sample, off at 0.01 or less; `x >= 0: 1 - 10^(-d x)(1 - x)`, else `10^(d x)(x + 1) - 1`, times `0.7 (1 - d/16)^8 + 0.3` |
| chorus | `+0x0`: three `CTapeDelay` at `+0x18`, `+0x20`, `+0x28` | `FUN_1800f2bc0` | tapes `1.1 n`, `0.854 n`, `n` samples, `n = rate * 0.02f` (`FUN_1800f2a10`), write heads starting at 0, 0.467, 0.778; LFO speeds `5 v + 0.1` Hz times 0.896, 1, 0.689; fed `L + R`; left gets tapes 1 + 2, right 2 + 3; wet `-(v^2)`; the switch only gates it |
| delay | `+0x38`: two `CTapeDelay` at `+0x8`, `+0x10` | `FUN_1800f3000` | tapes of `2 rate` samples, write heads at 0 and 0.7 (`FUN_1800f2e90`); stretch `t = (0.95 v + 0.05)^2 / 2`, `t ± (offset - 0.5) t` clamped to 0.05..1 (`FUN_1800f3080`); LFOs 1 and 0.876 Hz, depth `0.02 v^2`, feedback `0.84 v`, level `v^2`; setting the switch resets both |
| reverb | `+0x70`: two channels at `+0x20`, `+0xa8` | `FUN_180125020`, `FUN_1801247d0` | 50 Hz two-pole highpass (`FUN_1801247b0`, `FUN_1800df680`, `FUN_1800df780`), eight damped combs (`FUN_180124650`) summed into four allpasses of gain 0.5 (`FUN_1801246c0`); lengths `rate / 40090.9` times the tables at `DAT_180177500..578`, `DAT_1801772b8..2f8` (`FUN_180124e50`); feedback `0.67 v^0.25 + 0.3`, damping `0.8 v^0.25 + 0.2` (at most 1), mix `0.2 v^3`; setting the switch clears it |
| equaliser | `+0x1a0`: 2 x 8 bands of 0x3c bytes | `FUN_1800f7e70`, `FUN_1800dfe00` | gains `12 d |d|` dB, `d = 2v - 1`; band 0 low shelf, 7 high shelf, the rest peaks at `250 * 2^(i - 2)` Hz (`FUN_1800f7ba0`), bandwidth 1; RBJ designs with `A = 10^(dB/20)`, `w = hz * 6.283185 / rate` clamped to 3 (`FUN_1800dfa60`); turning it off clears the bands |

`CTapeDelay` (`FUN_1800e9550`, run `FUN_1800e9810`, reset `FUN_1800e9750`)
records each pass of its tape as (value, time in the pass) pairs in one of
two buffers and plays the previous pass back at the time the write head is
at now, interpolating between the two samples around it. The write head
moves `step` a sample, bent up by a triangle LFO `(3 - 2t) t^2 depth + 1`,
clamped to `1/size .. 0.2` (`+0x68`, `DAT_1801770f8`); `step` follows
`1 / ((s (1 - 2/size) + 2/size) size)` for stretch `s` in 0..0.99
(`FUN_1800e9bb0`) at 0.005 a sample. When the write head crosses the end of
the tape, the pass just finished keeps the crossing sample at its end and
gets the last three samples of the pass before it copied ahead of its start;
it becomes the pass the read head plays, from its first sample, and the
other buffer is emptied for the new pass, which starts with the crossing
sample again. The output is the read times a gain smoothed 0.01 a sample
toward its target; the write is the input plus the read times the feedback.
Its two optional filters (`+0xcc`, `+0xcd`) are never switched on by
Morphine. Constants: setters `FUN_1800e9c00` (gain), `FUN_1800e9c10`
(feedback), `FUN_1800e9c20` (LFO rate, Hz / rate), `FUN_1800e9c40` (depth).

`FUN_1800ff630` also has an idle path keyed on a counter at `+0x584` that
would stop the chorus after half a second and the reverb and equaliser
after three; the DLL only ever writes 0 to the counter, so the path never
runs and is not ported.

## Fidelity

Read from the binary: the state, every parameter's law, the control-block
timing, voices and glide, generators, morph line, envelopes and matrix, the
oscillator bank and level curve, and the effects, which match the listings
above term for term and constant for constant. Approximated by the port's own
account and not rechecked against the binary: the noise layer and the jump an
envelope or generator makes when released inside its release loop. Left out:
unison and the per-note tuning a host can send. No null test against FL.
