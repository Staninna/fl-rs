# Drumpad: state, parameters, voice model

Recovered from `Plugins/Fruity/Generators/Drumpad/engine_x64.dll` (FL Studio
2026, 449 KB, MSVC C++, image base 0x180000000). RTTI names the classes
`DrumpadSynth`, `DMPadGenerator`, `CSynthesizer`, `CVoiceGenerator` and
`CFLWrapper`. `Drumpad_x64.dll` beside it is the generic Delphi wrapper; state,
voices and sound all live in the engine. It is the framework BassDrum's
engine uses (see `docs/plugins/bassdrum.md`): the same `SSLF` state wrapper
and the same velocity curve. The Delphi classes behind `fl_fx.rs` and
`menvelope.rs` play no part.

The whole `.text` was decompiled with headless Ghidra
(`tools/ghidra/DecompileRange.java`), and the asm of every function cited
below was checked against it with a disassembler that resolves rip-relative
constants. The decompile, the disassembler script and the Ghidra project
stayed in that session's scratch directory and are not in the repository.

Corpus: 961 presets and 11 states in projects, the render test
`System/Render tests/Drumpad.flp` among them. Every one is `DMP3`, and the
Rust corpus test loads and sounds each of them with fidelity exact.

## How the plugin is wired

The wrapper's vtable is at `0x18005a1a0`:

| function | address | what it does |
|---|---|---|
| SaveRestoreState | 0x180002f10 | `SSLF`, a u32 length, the engine's chunk |
| ProcessParam | 0x180003420 | set: `int / 256`, clamped to 0..1; get: `int(256 v)` |
| slot fill | 0x180003dd0 | the per-key gains and pitch the voices read (section 3) |
| key cleanup | 0x180004840 | lets a key go once nothing sounds or holds it |
| TriggerVoice | 0x1800048f0 | key and velocity byte, below; returns the key as the voice handle |
| Voice_Release | 0x180004a10 | the synth's note off, which does nothing |

TriggerVoice takes the key as `int(InitLevels.Pitch * 0.01)`, clamped to
0..127, and the velocity byte as `int(127 ln(1 + 10 Vol) * 0.328458726)`,
clamped. 0.328458726 is `1 / ln 21`, so the byte is `127 log21(10 Vol + 1)`.

The engine's config block is at `0x180075de0`: 38 parameters at `+0x11c`,
polyphony 99 at `+0x124`.

The synth's vtable is at `0x18005c300`. A note on goes `0x1800077a0` ->
`0x18001e570` -> the poly allocator `0x18001d7b0`. Note off is a no-op. The
render is `0x180022b30`.

Voice (`DMPadGenerator`) functions:

| function | address |
|---|---|
| constructor, noise seed | 0x18001f1a0 |
| parameters to patch | 0x18001f4d0 |
| trigger, lane gains | 0x18001fca0 |
| reset | 0x1800201c0 |
| render at 38 kHz | 0x180020460 |
| resampler and output | 0x180020f50 |
| velocity blend | 0x1800229f0 |

Filters: the LOW highpass is `0x180018bf0`, the RBJ peak and high shelf
`0x180018d50`.

State and parameter tables: chunk save `0x180022d00`, chunk load
`0x180022de0`, parameter names `0x180023450`, defaults `0x180023290`,
display `0x180023330`.

CRT functions, named by the calls and constants around them:

| address | function |
|---|---|
| 0x180031d40 | powf |
| 0x1800306a0 | pow |
| 0x1800434f0 | sincosf |
| 0x1800456f0 | sinf |
| 0x180044df0 | cosf |
| 0x180045de8 | sinh, double (guessed) |
| 0x1800452f0 | logf (guessed; the `1 / ln 21` beside it fits, not disassembled to the end) |

## 1. State layout (event 213)

```
"SSLF"  u32 len  chunk[len]
chunk:  u32 magic  [char name[128]]  f32 params[...]
```

The magic reads `DMP3` as a big-endian word and sits on disk as `3PMD`. The
loader (`0x180022de0`) knows `DMP1`, `DMP2`, `DMP3` and the demo's `DPD1`,
`DPD2`, `DPD3`. In order, it:

1. sets SOF Level (17) to 0,
2. reads a 128-byte preset name when the chunk is longer than 132 bytes
   (`len - 4 > 0x80`),
3. sets Membrane Shape (12) to 1 and SOF Level to 0 again,
4. reads up to 38 f32 parameters, as many as the chunk holds.

A chunk that ends early leaves the parameters it does not reach as they
were. Version 1 and 2 chunks get their parameters remapped onto the 38; how
is not mapped.

The save (`0x180022d00`) writes 0x11c bytes: magic, name, 38 floats. That is
the 284-byte chunk every project holds. Presets and the render test hold a
288-byte chunk, a 39th float the loader never reads, and 4 more bytes past
the chunk.

## 2. Parameters

38 floats, 0 to 1 except the four velocity slots, which hold a parameter
number. Defaults are from `0x180023290`. The names are the Rust's; the
display strings at `0x180023330` are not mapped. "Blended" means the value
first goes through the velocity blend below; "raw" means the voice reads the
stored value.

| idx | parameter | default | what a note makes of `v` |
|---|---|---|---|
| 0 | Noise repeat | 0 | raw; not 0: every note restarts the noise at its seed |
| 1 | Retrigger | 0 | raw; `int(3 v)`, 0 to 3, more starts of the MID envelope |
| 2 | Mallet Amplitude | 0.5 | the mallet's start level `10^(-4 (1 - v))`, 1e-4 to 1 |
| 3 | Mallet Decay | 0.5 | `(500 (1 - v)^4 + 1) 1e-4`, what the mallet loses a sample |
| 4 | Mallet Noise | 0 | noise share `v`, tone share `1 - v` |
| 5 | Mallet Noise RP | 0 | the noise seed `v (2^32 - 1)` |
| 6 | Membrane Decay | 0.9 | lane feedback `-v^0.3` |
| 7 | Membrane Cutoff | 0.35 | lane lowpass coefficient `v^2` |
| 8 | Membrane Tension | 0.3 | gain phase step `v^4 / 2` a lane |
| 9 | Membrane Phase | 0.5 | gain phase at lane 511 |
| 10 | Membrane Material | 0.5 | gain power `20 m^4 + 1`, `m = 2 v - 1` |
| 11 | Membrane Size | 1 | first lane heard `512 (1 - v)`, at most 511 |
| 12 | Membrane Shape | 1 | lanes heard past it `128 v`, 1 to 128 |
| 13 | LOW Freq | 0 | highpass at `2^(11 v + 2)` Hz; off at exactly 0 |
| 14 | LOW Res | 0 | `q = 8 v + 1` |
| 15 | SOF Freq | 0.5 | ladder cutoff, see section 4 |
| 16 | SOF Decay | 0.5 | `((v / 2)^2 250 v^2 + 1) 1e-5`, the ladder's pitch fall a sample |
| 17 | SOF Level | 0 | `v^8 / 2` of the ladder added; off at exactly 0 |
| 18 | MID Freq | 0.5 | peak at `2^(9 v + 5)` Hz, 32 Hz to 16 kHz |
| 19 | MID Level | 0.5 | `32 (v - 0.5)` dB |
| 20 | MID Res | 0 | bandwidth `4 (1 - v) + 0.5` octaves |
| 21 | MID Freq Mod | 0.5 | `32 (v - 0.5)`, how far the envelope moves the frequency |
| 22 | MID Level Mod | 0.5 | `48 (v - 0.5)` dB, how far the envelope moves the gain |
| 23 | MID Attack | 0.5 | `(1000 (1 - v)^4 + 1) 1e-5`, what the MID envelope loses a sample |
| 24 | HIGH Freq | 0.5 | shelf at `2^(4.1 v + 8)` Hz, 256 Hz to 4.4 kHz |
| 25 | HIGH Level | 0.5 | `36 (v - 0.5)` dB; off within 0.01 dB of 0 |
| 26 | HIGH Res | 0 | bandwidth `2 (1 - v) + 0.5` octaves |
| 27 | Lo-Fi | 0 | `int(65536 (1 - v)^5 + 4)` steps from `v = 0.01`; off below |
| 28 | Hold | 1 | `3 v^3 + 0.001` seconds |
| 29 | Pitch | 0.5 | playback rate `2^(2 v - 1)`, half to double |
| 30..33 | velocity slots | 0 | slot value `n` picks parameter `n + 1`; 0 picks none |
| 34..37 | slot targets | 0 | where velocity moves the picked parameter |

The dB figures are the knob's. The biquads take them as `10^(dB / 20)` where
the RBJ cookbook has `/ 40`, so a filter's gain comes out at twice the dB the
table gives: up to 32 dB for MID, 36 dB for HIGH.

Velocity blend (`0x1800229f0`). Slot `k` (0..3) picks parameter
`int(params[30 + k]) + 1` and sets its target to `params[34 + k]`. The note's
value is `(1 - x) base + x target`, with `x` the note's velocity term from
section 3. A parameter no slot picks keeps its base. When two slots pick the
same one, the later slot wins in the Rust.

ProcessParam (`0x180003420`) sets a parameter to `int / 256`, clamped to 0..1,
and reports it as `int(256 v)`. Automation can therefore move a slot only to 0
(none) or 1 (Mallet Amplitude); other picks come from a saved state.

The voice reads the parameters once, when its note starts. Moving a knob
does not change a hit that is sounding.

## 3. The note model

The key starts a voice and nothing else. Drumpad has no pitch of its own
per key: two keys on fresh plugins give the same sound.

Velocity. FL's velocity byte (section "How the plugin is wired") goes to the
poly allocator (`0x18001d7b0`), which stores `x = powf(vel / 127, 0.5)` at
voice `+0x20`. The exponent 0.5 lives at synth `+0x38d8`. A velocity byte of
0 sounds nothing in the Rust.

Per-key gains and pitch. The voices do not take their gains or their pitch
from their own note. For each key with a voice sounding or a note held, the
wrapper keeps the left and right gain of the last note on it and its pitch
offset in semitones: synth `+0x20` and `+0x220` hold the 128 left and right
gains, `+0x420` the semitones. The slot fill (`0x180003dd0`) gives every key
between those the values of the nearest such key below it, and the keys
under the lowest one the lowest one's. The render (`0x180022b30`) then has
voice `i` of the pool read the slot of key `i`, not the slot of the key it
plays. With the keys a song uses above the voice numbers, every voice
follows the last note on the lowest key that sounds. The bend is
`powf(2, clamp(semis, -12, 12) / 12)`. The key cleanup (`0x180004840`) drops a
key once no voice sounds it and no note holds it.

What fills the slots is partly guessed. The semitones are taken as the
note's fine pitch over 100, from the reading of `FinalLevels.Pitch` in
`docs/plugins/host-voice.md`; the binary was not checked for it. The Rust
takes the gains as the host's circular pan law of the note's pan and
velocity (`sampler::circular`); the note on the binary does not record that
formula.

Note off. The synth's note off does nothing. A hit plays until the voice's
silence rule ends it (section 4).

Voice allocation. FL has 99 voices. How the poly allocator picks one when
all are busy is not mapped. FL keeps each voice's 128 lines apart, so a
reused voice runs on with what its lines still hold.

The noise seed. The voice constructor (`0x18001f1a0`) seeds each voice's
noise from one global generator, `s = s * 0x1c3c718d - 0xaf2c25b`, run 1000
steps for every voice made. Because the generator is global, it runs on
from one plugin to the next in the order FL makes their voices.

## 4. The voice

A voice makes its samples at a fixed 38 kHz (`0x180020460`) and the
resampler (`0x180020f50`) reads them out at the host's rate. Per 38 kHz
sample, in order: mallet, membrane, SOF, LOW, HIGH, MID, clip to ±1, hold,
lo-fi, silence check. One sample is `2.6315789e-5` s (`0x18005c4e8`).

### Mallet

The mallet level starts at Mallet Amplitude's `10^(-4 (1 - v))` and moves
towards 1e-7 by the Mallet Decay rate each sample. The noise is a 32-bit LCG,
`r * 0x0bb38435 + 0x3619636b`, read as `r / 2^32`. The mallet signal is
`((noise - 0.5) noise_share + tone_share) level`, so with Mallet Noise at 0 it
is a pure decaying DC pulse. With Noise repeat on, every note restarts the
generator at the Mallet Noise RP seed; otherwise it runs on from the last
note.

### Membrane: lines and lanes

The membrane is 512 lanes, lane `k` a delay of `512 - k` samples. The lanes
live four to a line: 128 lines, line `i` holding lanes `4i` to `4i + 3` in
`512 - 4i` slots of four floats. Lane `j` of a line is `j` slots shorter than
lane 0. The binary gets that by shifting lanes 1 to 3 along by `j` slots
whenever the line's pointer wraps, and copying their first slots to the
tail just before. Simulating that pointer and shift scheme confirmed the
`512 - k` delays.

Each sample, for every line the note runs and each of its four lanes:

```
v   = lane[slot]
sum += v * gain[lane]
lp  += (v - lp) * cutoff          // one-pole lowpass, cutoff = Membrane Cutoff^2
lane[slot] = lp * (-decay^0.3) + mallet
```

The output is the sum over all lanes. The mallet goes into every lane the
note runs, heard or not.

Which lanes a note runs: from line `size / 4` (at most 127) to the line that
holds lane `size + shape`, rounded up, no further than line 128. That is at
most 33 lines. Lanes from `size` to `size + shape` (511 past the end) are
heard; the others in those lines get gain 0.

Lane gains (`0x18001fca0`, at trigger). A phase starts at Membrane Phase on
lane 511 and steps down the lanes by `tension^4 / 2`, wrapping at 1. A heard
lane's gain is a triangle of the phase, -1 to 1, raised to the power
`20 m^4 + 1` with `m = 2 material - 1`, or to its inverse when `m < 0`, with
the sign kept.

### LOW

A two-pole highpass (`0x180018bf0`, sincosf), on while LOW Freq is not 0.
With `w = 2 pi f / 38000`:

```
c0 = (cos w + 1) / (sin w / q + 2)
c1 = -2 q cos w / (sin w + q)
c2 = (q - sin w) / (sin w + q)
y  = (x - 2 x1 + x2) c0 - c1 y1 - c2 y2
```

The numerator and the poles take `q` in different ways, so the alpha mixes.
Every coefficient under 1e-9 either way becomes 0 (`0x18005c4e4`). It is
tuned at trigger.

### MID and HIGH

Both are RBJ biquads from `0x180018d50`, with `A = 10^(dB / 20)` (the dB
doubled, section 2), `w = 6.283185 f / 38000` clamped to 0..3, and a
bandwidth in octaves through `sinh(ln 2 / 2 * bw * w / sin w)`. Small
coefficients flush to 0 as in LOW.

HIGH is type 2, the high shelf, tuned once at trigger. It runs while its
gain is more than 0.01 dB from 0.

MID is type 1, the peak. It runs while MID Level or MID Level Mod is off
0.5; MID Freq Mod alone does not turn it on. An envelope starts at 1 and
loses the MID Attack rate each sample. With `rise = 1 - env`, the gain is
`level + rise * level_mod` dB and the frequency is `f (1 + rise * freq_mod)`
for a positive mod, `f / (1 - rise * freq_mod)` for a negative one, clamped to
32..12000 Hz. The filter is retuned every 6 samples. Retrigger sets the
envelope back to 1 when it falls under 0.01, up to 3 times.

### SOF

A self-oscillating 4-pole ladder, on while SOF Level is not 0. No shipped
preset turns it on. Its cutoff is
`c = clamp(2^(9.3 (freq / 2 + 0.1) + 5) * 0.00015789474, 0, 1)` and
`k = c * 1000 / 3`. Each sample:

```
in   = x - 100000 * s4             // feedback -1e5
x   += s4 * level
cc   = c / (k + 1 - env * k)       // env falls from 1 by the SOF Decay rate
in   = in / (1 + 0.005 |in|)       // saturation
s1..s3 and then s4: one-pole stages at cc
```

As the envelope falls, the cutoff drops from `c` to `c / (k + 1)`.

### Hold

After the MID filter the signal is clipped to ±1 and multiplied by a hold
envelope. Until the note's time reaches Hold's `3 v^3 + 0.001` s, the
envelope moves a tenth of the way to 1 each sample. After that it loses
0.001 of itself each sample. Reset sets it to 1.

### Lo-fi

From Lo-Fi at 0.01 up, the sample becomes `int(steps * x) / steps`,
truncated towards 0, with `int(65536 (1 - v)^5 + 4)` steps.

### The silence rule

A sample under 1e-4 (`0x18005c4f0`) is silent. A voice ends after 25 ms of
silence once it has gone above that level, or after 0.5 s of silence if it
never has.

### 38 kHz render and the resampler

The resampler (`0x180020f50`) works in passes of at most 256 output frames.
Each pass renders as many 38 kHz samples as it needs and reads them at a
step of `38000 / rate * pitch * bend`, interpolating linearly between the two
samples either side. The last sample of a pass stays as sample 0 of the
next. The output is multiplied by a left and a right gain that move 0.001
of the way to the key slot's gains each frame. A gain of -1 means unset; the
first frame jumps straight to the target.

The reset (`0x1800201c0`) silences the voice and clears its lines, filters,
ladder and buffer, and sets both gains back to unset.

## 5. What `drumpad.rs` builds

`crates/plugins/src/drumpad.rs` is the plugin (state, parameters, voice
pool, per-key slots) and `crates/plugins/src/drumpad_voice.rs` one voice
(`Patch`, lane gains, LOW and the biquads, the membrane, the render and the
resampler). Both are registered as the `Drumpad` generator.

Exact as read: the state loader with its parameter 12 and 17 overrides and
the short-chunk rule, ProcessParam's `int / 256`, the velocity byte and its
square root, the velocity blend, every parameter curve in section 2, the
lane gains, the membrane with FL's pointer and shift scheme, LOW, the peak
and shelf with their doubled dB, the MID envelope and retrigger, the SOF
ladder, hold, lo-fi, the silence rule, the 256-frame resampler and gain
glide, the per-key slot fill and its voice `i` reads key `i` quirk, and the
noise seeds of the first plugin FL loads. Clamps run `max` then `min` as the
binary does, so a NaN becomes the lower limit.

Approximated:

- 32 voices instead of 99. A note past them takes the oldest voice and
  starts its lines from silence, where FL's would run on with what they
  hold. Fidelity reports "Drumpad voices past 32" once that happens.
- Lines are laid out per note, in room sized when a state loads for both
  velocity extremes. An automated Size or Shape that needs more gets fewer
  lines, and fidelity reports "Drumpad membrane lines past their room".
- Each plugin instance seeds its noise as the first one FL loads does. Only
  the noise of a first instance, or of any instance with Noise repeat on,
  matches FL.
- `DMP1` and `DMP2` states (and `DPD1`, `DPD2`) are refused, where FL remaps
  them.
- A sample-rate change cuts every voice, as the module docs say FL does.
- `tail()` is the Rust's own bound: hold, the hold envelope's fall, the
  25 ms and one pass, at the lowest pitch, or the half second.
- No render has been compared against FL's output.

Unknown or guessed:

- `0x180045de8` as sinh and `0x1800452f0` as ln rest on constants and usage.
  The second was not disassembled to the end.
- The slot semitones as fine pitch over 100 come from `host-voice.md`, not
  the binary. The pan law that fills the slot gains is not recorded.
- How FL's poly allocator picks a voice when all 99 are busy.
- How the loader remaps `DMP1` and `DMP2` parameters.
- What the display function at `0x180023330` prints, and the parameter
  names as stored at `0x180023450`.
- What the wrapper does to a key's held state on Voice_Release.
