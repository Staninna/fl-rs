# SimSynth: state, parameters, voice and chorus

Recovered from `Plugins/Fruity/Generators/SimSynth/SimSynth_Eng_x64.dll`
(FL Studio 2026, MSVC C++, image base 0x180000000, export
`CreatePlugInstance` at 0x18000be80), where the state, the voices and the
sound live: an `SSPlug` (`TCPPFruityPlug`) holding a `Preset` core, twelve
voice wrappers and a chorus. `SimSynth_x64.dll` beside it is the Delphi
panel (`TSynthEditorForm`, whose wheel tags are the parameter numbers).
SimSynth is David Billen's SimSynth Live. Read with Ghidra (one headless
run, `DecompileRange.java` over 0x180001000..0x18000d000) and capstone
listings, and checked by running the DLL's own code under unicorn through
its `TFruityPlug` interface with a stub host (`.scratch/notes/simsynth-emu/`).

Corpus: 77 states, all 564 bytes: 73 shipped presets
(`Plugin presets/Generators/SimSynth`), two in the plugin database, one in
`Patches/Packs` and the render test `System/Render tests/SimSynth.flp`. No
shipped project uses SimSynth.

Port: `crates/plugins/src/simsynth.rs`. Against the DLL under the emulator
it plays bit for bit (`emu`): every corpus state at key 48 for 6000 frames
let go at 3000, every third at key 72 for 9000 let go at 2000, and the render
test at key 60, pinned in `plays_the_render_test_as_the_engine_dll_does`.

Confidence tags: `asm` (read from the listing or the decompile), `emu`
(matched against the DLL under the emulator), `corpus`.

## How the plugin is wired

`PlugInfo` (0x1800834a0, packed): SDK 1, Flags `0x11` (Generator,
GetNoteInput: no `FPF_NewVoiceParams`, so FL hands SimSynth the old voice
levels, with an integer pan and an integer pitch), 54 parameters. The
vtable at 0x18006b998, in SDK order:

| slot | method | address |
|---|---|---|
| 1 | Dispatcher | 0x18000a4a0: 1 ProcessMode, 3 block size, 4 sample rate |
| 3 | SaveRestoreState | 0x18000a610 |
| 6 | ProcessParam | 0x18000ac40 |
| 8 | Gen_Render | 0x18000ae50 |
| 9 | TriggerVoice | 0x18000b700 |
| 10 | Voice_Release | 0x18000bc20 |
| 11 | Voice_Kill | 0x18000bc50 |

The host is the SDK's C++ `TFruityPlugHost`: slot 3 (`+0x18`)
`ComputeLRVol(&L, &R, int pan, float vol)`, slot 5 Voice_Kill, slots 7 and
8 LockMix and UnlockMix.

Plugin fields: `+0x240` host, `+0x258` the `Preset` core (`+0x28` of it the
patch, `+0x10` the list of voices, `+0x48` the transpose, 2400 cents),
`+0xa90` chorus on, `+0xa94` the voice volume (f32 of `d[39]`), `+0xaf0` the
chorus, `+0x40b30 + 0x30 i` the twelve voice wrappers (`+0x28` of each the
voice), `+0x40d70` the twelve playing notes, `+0x40dd8` the sample clock,
`+0x40de0` f64[45] and `+0x40f48` i32[19] the knobs.

`CreatePlugInstance` builds the core and twelve voices, detunes the voices
(`FUN_180002d60`, below), loads the default patch (`FUN_180009f20`, from
0x18006b790 and 0x18006b900) and builds every voice three times. The sample
rate (dispatcher 4) sets `0x1800831d0` (rate), `0x1800831d8` (44100 / rate),
`0x180083000` (rate / 1000), sets the chorus up again and calls ProcessMode,
which sets the patch's quality byte `0xc000` to 1 and builds every voice.
So in FL the quality byte is always 1 and `0xc001` always 0.

## 1. State (event 213: 564 bytes)

| byte | type | field |
|---|---|---|
| 0..359 | f64[45] | `d[i]`, the knobs, 0..1 |
| 360..435 | | 19 unused i32 |
| 436..511 | i32[19] | `n[i]`, the buttons, 0 or 1 |
| 512..559 | | 12 unused i32 |
| 560..563 | i32 | version, 14 now |

Load (`asm`, `0x18000a610`): version 0 multiplies `d[39]` by 0.75; below 5
each coarse tune `d[1], d[8], d[15]` is `s = int(24 d - 12)`, plus 12 when its
shape `d[i+5]` is 0.3, then `(s + 24) / 48`; below 13 each coarse is
`(int(48 d - 24) + 24) / 48`. Then the transpose (2400), the chorus flag
`n[18]`, the volume `f32(d[39])`, every i32 (`FUN_180002340`), every f64
(`FUN_180001820`) and every voice (`FUN_180008180`).

## 2. Parameters

`ProcessParam` (`0x18000ac40`) reads its table at 0x1800831e0 (kind, index,
maximum). A knob stores `value / max` into `d[index]` and reads
`floor(max d)`; a button stores `value != 0` into `n[index]`; a shape stores
`[0.2, 0.1, 0.0, 0.4, 0.3][value]` into `d[index]` (0x180083540). The panel
numbers (`asm`, DFM tags):

| param | name | value | param | name | value |
|---|---|---|---|---|---|
| 0, 3, 6 | Osc1..3 Level | `d[3], d[10], d[17]` | 36 | Chorus | `n[18]` |
| 1, 4, 7 | Osc1..3 LFO | `d[4], d[11], d[18]` | 37 | Retrigger | `n[13]` |
| 2, 5, 8 | Osc1..3 Env | `d[5], d[12], d[19]` | 38 | SVF Track Amp | `n[16]` |
| 9 | LFO Delay | `d[21]` | 39 | LFO On | `n[12]` |
| 10 | LFO Rate | `d[22]` | 40 | SVF On | `n[14]` |
| 11 | SVF Cut | `d[26]` | 41, 42, 43 | Osc1..3 On | `n[0], n[4], n[8]` |
| 12 | SVF Emphasis | `d[27]` | 44 | Ring Osc1 x Osc2 | `n[17]` |
| 13 | SVF Env | `d[28]` | 45, 48, 51 | Osc1..3 +1 Octave | `n[1], n[5], n[9]` |
| 14 | SVF LFO | `d[29]` | 46, 49, 52 | Osc1..3 +2 Octaves | `n[2], n[6], n[10]` |
| 15 | SVF High | `d[32]` | 47, 50, 53 | Osc1..3 Warm | `n[3], n[7], n[11]` |
| 16 | SVF Band | `d[33]` | 54, 55, 56 | Osc1..3 shape | `d[6], d[13], d[20]` |
| 17..20 | Env A, D, S, R | `d[35..38]` | 57 | LFO shape | `d[24]` |
| 21 | VCA Velo | `d[39]`, the voice volume | | | |
| 22..25 | SVF Env A, D, S, R | `d[41..44]` | | | |
| 26, 29, 32 | Osc1..3 Pulse Width | `d[0], d[7], d[14]` | | | |
| 27, 30, 33 | Osc1..3 Coarse | `d[1], d[8], d[15]` (max 20160) | | | |
| 28, 31, 34 | Osc1..3 Fine | `d[2], d[9], d[16]` | | | |
| 35 | SVF Kb | `d[30]` | | | |

`d[23], d[25], d[31], d[34], d[40]` and `n[15]` have no control; `d[31]`
(`0xc160`), `d[34]` (`0xc144`), `d[40]` (`0xc194` = `d^2`) and `n[15]`
(`0xc15c`) are stored and never read by the sound.

The patch (`FUN_180001820`, `FUN_180002340`; `d` clamped to 0..1, `int`
truncates), per oscillator `i` at `0xc008 + 0x34 i`:

| field | from |
|---|---|
| pulse width `+4` | `2 + floor(96 d)` (`2 - int(-96 d)`) |
| coarse `+8` | `(floor(48 d) - 24) * 100` cents |
| fine `+0xc` | `int(200 d - 100)` cents |
| level `+0x14` | `int(100 d)` |
| LFO, Env `+0`, `+0x18` | `int(200 d - 100)` |
| off `+0x1c` | `n == 0` |
| +1, +2 octave, warm `+0x20..0x28` | `n` |
| shape `+0x2c` | 0 → 2 triangle, 0.1 → 1 saw, 0.2 → 0 pulse, 0.3 → 4 sine to sine^4, 0.4 → 3 noise; any other value leaves it |

and `0xc038` ring = `n[17]`, `0xc0ac` LFO rate `int(999 d)`, `0xc0b4` LFO
delay `int(100 d)`, `0xc0c0` LFO off, `0xc0cc` retrigger, `0xc0d0` LFO shape
(0 → 2, 0.1 → 1, 0.2 → 0, 0.4 → 3; 0.3 leaves it), the envelopes' times in ms
`int(8000 d^3)` (attack) and `int(8000 d^2)` (decay, release) and sustain
`int(255 d)`, `0xc12c` cut, `0xc138` emphasis, `0xc13c` high, `0xc140` band
`int(255 d)`, `0xc130` / `0xc134` the filter's env and LFO amounts
`int(255 (d - 0.5))`, `0xc164` key tracking `f32(d)`, `0xc178` 0x2253 when
the SVF is off, `0xc128` the filter's own envelope (`n[16] == 0`).

## 3. Tables

Static, then again whenever the voices are built at a new rate
(`FUN_180008830`, `FUN_180008f40`):

- pitch steps `0x1800fc4f0[i] = (u32)(i64)(f32(2^32 / R) f32(2^(i/1200) 4.088681697845459))`, `i < 30000`, `R` the rate the voice build hands it: twice the host's (quality byte 1);
- the note map `0x1800dc4b0[k] = k - 32` for 74 ≤ k ≤ 30027, 29996 above, so a pitch in cents (from 4.0887 Hz) indexes the steps;
- noise `0x18009c4a0[i] = rand() / 32767`, 65536 of them. Every voice build ends in `srand(666)` and the table is made again on a new rate, so FL's table is the run after `srand(666)` (`emu`);
- saw `sin(x pi/2)` for `x` from 1 down by 1/2048 in f32, triangle 0 → 1 → -1 → 0 in f32 steps of 1/1024, sine `sin(y)` with `y` up by `f32(2 pi / 4096)`, and its fourth power.

An oscillator's table (`FUN_180007ae0`, 4096 f32 in the patch at `osc *
0x4000`): pulse is `x = 0.65 x + 0.2275` for the first `pw% of 4096`
samples (at least one), `0.65 x - 0.2275` to 0xff8 and `0.65 x` to the end;
saw and triangle are copied; the sine shape is `sine (1 - k) + sine^4 k`
with `k = (pw + 2) / 96`. +1 octave adds the table read at `2n`, +2 at `4n`,
both `t[2n] + t[4n] + t[n]`. Then every `t[n] = (t[n] + t[n+1]) / 2`.

## 4. The voice

The voice build (`FUN_180008180`) and note-on (`FUN_1800061a0`):

- envelopes (`FUN_1800045f0`): counts `int(ms * rate / 1000)`; attack climbs
  linearly to 1, decay goes to the sustain level `S = (int(255 d) << 8) /
  65280` (`<< 8` kept to 16 bits, 0x100 read as 1), sustain lasts 0x4fffffff
  samples, release (`FUN_180007980`) falls linearly from where it is to 0 in
  `max(release, 2)` ms (the filter's `max(release, 1)`). The amp envelope is
  squared past the attack;
- oscillators (`FUN_180006660`): a wavetable oscillator at quality 1
  (`p_loopquality`, tick `FUN_180003940`): two 12.20 phase steps a sample,
  `y = (t[p0] + y + t[p1]) / 3` (`k = 0x3eaaaa9f`); Warm adds a second phase
  20 cents up. Noise (`p_noise`, `FUN_180003d60`) is the noise table through
  a band pass `low += k band; band += k (n - low - band)`, `k = 0.3025`, times
  16. Every voice build starts each wavetable phase at
  `int(noise * 2^31)`; a note-on does not move it. Level `(level / 100)^2`;
- pitch: `+0x250 = max(note cents + 4800, 100) + 2400`, every block from
  `FinalLevels.Pitch` (read as the old format's integer, `0x18000b046`); an
  oscillator's pitch is that plus coarse plus fine, and every 32 samples
  (`FUN_1800039f0`) it adds `int(lfo * 12 LFO + env * 12 Env)` cents and,
  if it moved, looks up `step = steps[map[pitch + detune + 32]] + detune`.
  The detune is the voice's (`FUN_180002d60`: 0, -8, 8, -6, 6, -5, 5, -4,
  4, -2, 2, -1 for voices 0..11) plus the oscillator's number;
- the LFO (`p_lfo`, `FUN_180004a30`): a half period of
  `max(64, (1 - rate/999)^3 * R * 4)` samples; square 0/1, saw 1 → 0 each
  half, triangle (two outputs: 0..1 for the filter, a bipolar lobe for the
  pitch), sample and hold from the noise. A delay fades it in over
  `delay * 3528` samples on a retriggered note. Free running it starts at
  the plugin's sample clock;
- the filter (`p_filt`, `FUN_180004020`, `FUN_180004130`): two state
  variable stages in f64, the first with `f2 = r fc^2`, the second with
  `f1 = r fc` (`r = 44100 / rate`, each at most 1), damping
  `1.1 - (1 - (1 - e/255)^2)`, mixed `LP cos(a) cos(h) + BP sin(a) + HP cos(a)
  sin(h)` with `a = band pi/510`, `h = high pi/510`. Cutoff every 32 samples:
  `cut/255 + (pitch + 4800)/12700 * kb + FCut + env * envAmt/128 + lfo *
  lfoAmt/128`, clamped to 1e-4..1. Mod Y moves the emphasis by
  `int(255 FRes)`;
- the sample (`FUN_1800050c0`): `(osc1 + osc2 + osc3)` (ring: `osc1 * 2
  osc2`) through the filter, times a level that follows the squared amp
  envelope 3% a sample.

## 5. Render

`Gen_Render` renders each playing voice into a mono buffer and adds it with
`ComputeLRVol(pan, FinalLevels.Vol * volume)` (gains ramping over 128
samples when they move). With the chorus off both sides are the mix. With it
on (`FUN_180009ca0`): the left side is only the mix delayed by `65536 -
20 rate/1000` samples less than a 65536 line, read with linear
interpolation at a speed that swings between 0.9928 and 1.0072 by `2 * 0.0144
/ (338 rate/1000)` a sample (338 ms: 333 plus a counter global to the DLL
that the chorus set-up moves on by 5); the right side is the dry mix. A
voice whose amp envelope ends is handed back to the host.

## 6. Left open

- FL's oscillator phases at a note: they come from a noise read position
  global to the DLL that every voice build and noise oscillator moves on, so
  they depend on how many SimSynth instances FL made and in what order. The
  port starts as one instance after its create and one sample rate.
- Moving a parameter while notes sound: the DLL rebuilds the sounding voices
  piece by piece (`FUN_180001000`); the port applies the patch to the next
  notes only.
- The runtime's `pow` and `sin` are Rust's (the pitch steps, the tables and
  the filter's mix), equal on every value the tests reach.
