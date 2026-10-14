# Luxeverb: state, parameters, signal chain

Recovered from `Plugins/Fruity/Effects/Luxeverb/LuxeVerb_x64.dll` (FL Studio
2026, Delphi shell, image base 0x400000) and `Shared/ildsp_x64.dll` (MSVC
C++, image base 0x180000000). The shell binds `Shimmer_CreateInstance` from
the library and keeps the state, the editor and the curves of its 49
controls. The library class `ShimmerReverbImpl` does all the sound: a
sixteen-line feedback delay network with a granular pitch shifter in its
feedback. Ghidra projects `ghidra/proj/luxeverb` (shell, imported for this)
and `ghidra/proj/ildsp` (shared with Spreader). Decompiles and listings are
under `ghidra/out/luxeverb/`: `shell1/`, `vmt/`, `shellasm/` for the shell,
`shim1/`, `impl/`, `eng2/`, `eng2asm/`, `mathasm/` for the library.

Corpus: 24 states in 9 `.flp` (13 of version 7, 11 of version 8), 102 in
the presets and the plugin database (all version 8, plus four copies of the
version 7 database default). No render-test project ships, so the checks
are unit tests from the maths and the corpus renders.

Confidence tags: `loader`, `dfm`, `asm`, `corpus`, `inferred`.

## Shared code with Reeverb 2 and FL's reverb

None. Fruity Reeverb 2 is a Dattorro plate whose loop runs in
`Shared/Reverb_x64.dll` (`BaseReverb`), and FL's own `TFruityReverb` lives
in FL's executable. Luxeverb's network, filters and shifter are all in
`ildsp_x64.dll` and call nothing in either. `luxeverb.rs` reuses nothing
from `reeverb_2.rs` or `fl_fx.rs`.

## How the plugin is wired

Shell: `CreatePlugInstance` @ `0x0072de40` -> constructor `FUN_0072df10`,
VMT `0x0072dd08`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x0072e280 |
| 3 | SaveRestoreState | 0x0072f610 |
| 5 | ProcessEvent | 0x0072fb50 (tempo: stores it, then `FUN_0072fc40`) |
| 6 | ProcessParam | 0x0072fe70 |
| 7 | Eff_Render | 0x00731870 |

Shell object: `0x1b8` the library object, `0x1c0` i32[49] the controls,
`0x284` the sidechain input, `0x294` the tempo. `FUN_0072b710` resolves
`Shimmer_CreateInstance` (`0x1801ab810`), which allocates the 0x191470-byte
engine (`FUN_1801843b0`) behind a 16-byte `ShimmerReverbImpl` (vtable
`0x1803e0068`):

| Impl slot | address | does |
|---|---|---|
| +0x08 | 0x1801ab9c0 -> `FUN_1801849a0` | prepare(rate): buffers, defaults, then reset |
| +0x10 | 0x1801ab9d0 -> `FUN_180186030` | set parameter (index 0..48, float) |
| +0x18 | 0x1801ab9e0 | tempo: engine `+0x190afc` and the shifters' case 9 |
| +0x20 | 0x1801aba40 | meters for the editor |
| +0x28 | 0x1801abad0 -> `FUN_180187290` | process(n, in, sidechain, out), interleaved |
| +0x50 | 0x1801abc90 -> `FUN_180188ac0` | reset |

`Eff_Render` hands the library the sidechain buffer when `0x284 > 0` and
the host has one, and otherwise its own output buffer. The rest of the
render (`FUN_00731540`) only feeds meters.

The dispatcher's sample-rate case (4) calls prepare, then pushes every
control in index order. State load (`SaveRestoreState`) pushes every control
in order, then calls `FUN_0072fc40` with the tempo, which sets the library's
tempo and pushes once more the delay of each pair the sync buttons pick: 5
or 6, 17 or 18, 45 or 46, 47 or 48. `asm`

## 1. State layout (event 213: 165 bytes in version 7, 169 in 8, 211 in 9)

`SaveRestoreState` (`FUN_0072f610`):

```
save:    i32 9; i32 sidechain (0x284); u8 predelay sync; u8 feedback delay sync;
         u8 editor 0x948; u8 editor 0xc00; u8 editor 0x950;
         i32[49] controls; u8 ff delay 1 sync; u8 ff delay 2 sync
restore: v >= 10: nothing is read
         v < 8:  i32[38], then control 38 = 1
         v == 8: i32[39]
         v >= 9: i32[49], then the two ff sync bytes
         v < 9:  controls 39..48 = 0, 0, 24, 20, 1719, 20, 1000, 192, 1000, 192 (FUN_0072f570)
```

`loader`, `corpus`. The three editor bytes are display toggles. The fill
gives control 43 the value 1719, past its 0..1000 range; the shell clamps
its place to 1, and pitch type 0 never reads it. Whether 39..48 sit in the
order of the editor's tags cannot be checked: no version 9 state is in the
corpus. `inferred`

## 2. Controls

Index = editor tag. Ranges and defaults from the DFM (`dfm`). The shell
turns a control into `x = clamp((v - min) / (max - min), 0, 1)` in doubles
and then into the library's float (`FUN_00730100`, `asm`). `curve(x, a, b,
k)` is `a + (b - a) * exp(ln(x) / k)` (`FUN_0072deb0`; `ln` and `exp` are
`FUN_00409b50` and `FUN_00408930`), so `x^(1/k)`.

| # | control | range | library gets | library does (`FUN_180186030`) |
|---|---|---|---|---|
| 0 | input wet gain | -600..1500 | curve(x, -60, 15, 2) dB | `0x9e4` = 10^(dB/20) |
| 1 | input high cut | 40..20000 | curve(x, 40, 20000, 1.5) Hz | S10 target |
| 2 | input low cut | 20..5000 | curve(x, 20, 5000, 0.5) Hz | S9 target |
| 3 | reverb on | 0..1 | x | on/off at 0.5, see below |
| 4 | decay | 250..20000 | curve(x, 0.25, 20, 0.4) s | S5 target |
| 5 | predelay | 0..5000 | curve(x, 0, 500, 0.5) ms | predelay = clamp(ms * 0.001 * rate, 1, 172000) |
| 6 | predelay synced | 1..768 | x * 16 | predelay = clamp(15 / tempo * (v * rate), 1, 172000) |
| 7 | brightness | 0..1000 | x | S4 target |
| 8 | character | 0..100 | v * 0.01 | rotation angle v * 90 degrees, matrix rebuilt |
| 9 | size | 1..100 | x * 99 + 1 | line lengths from the primes, size gain |
| 10 | modulation rate | 0..1000 | x | LFO step = x / rate (so at most 1 Hz) |
| 11 | modulation depth | 0..100 | v * 0.01 | depth = v * 0.1 * rate / 44100 |
| 12 | diffusion | 0..1000 | x | diffuser mix |
| 13 | freeze mode | 0..2 | v + 1 | 1 normal, 2 freeze, 3 sustain |
| 14 | high quality | 0..1 | x | Hermite and Lagrange reads instead of linear |
| 15 | feedback on | 0..1 | x | feedback path |
| 16 | feedback pitch | -120..120 | (x - 0.5) * 24 semitones | `0xa08`, shifter 0 case 0 |
| 17 | feedback delay | 10..5000 | x * 499 + 1 ms | shifter 0 case 6 |
| 18 | feedback delay synced | 1..768 | x * 16 | shifter 0 case 10 |
| 19 | feedback gain | 0..1000 | x | `0x190e60` |
| 20, 21 | feedback high, low cut | 20..5000 | curve(x, 20, 5000, 0.5) Hz | S11, S12 targets |
| 22 | feedback reverb mix | 0..1000 | x | `0x19145c` |
| 23 | envelope scale | -1000..1000 | x * 2 - 1 | `0x9ec` |
| 24 | envelope threshold | -6000..0 | v * 0.01 dB | `0x9e8` |
| 25, 26 | envelope attack, decay | 50..1000 | v | coefficient exp(-2200 / (v * rate)) |
| 27 | envelope offset | 0..100 | x - 0.5 | `0x9f0` |
| 28 | envelope mode | 0..2 | v + 1 | 1 off, 2 to wet, 3 to decay |
| 29 | envelope sidechain | 0..1 | x | detector listens to the sidechain |
| 30 | envelope low cut | 20..2000 | curve(x, 20, 2000, 0.5) Hz | detector high pass, Q 0.75 (`FUN_1801871b0`) |
| 31 | envelope smoothing | 10..1000 | x * 99 + 1 ms | S2 time |
| 32, 33 | dry, wet level | -600..1500 | curve(x, -60, 15, 2) dB | dry = clamp(10^(dB/20), 0, 5.6); wet stays in dB |
| 34 | peak frequency | 40..10000 | curve(x, 40, 10000, 0.5) Hz | S6 target |
| 35 | peak gain | -1800..1800 | v * 0.01 dB | S7 target = 10^(dB/20) |
| 36 | peak Q | 25..5000 | x * 4.75 + 0.25 | S8 target |
| 37 | stereo width | 0..200 | v * 0.01 | S3 target |
| 38 | limiter | 0..1 | v | output gain riding |
| 39 | pitch type | 0..1 | x | 0 feedback, 1 feedforward |
| 40 | feedforward unpitched | 0..1000 | x | dry share of the feedforward input |
| 41, 42 | feedforward pitch 1, 2 | -24..24 | v semitones | shifters 1, 2 case 15 |
| 43, 44 | feedforward amp 1, 2 | 0..1000 | x | shares of shifters 1, 2 |
| 45, 47 | feedforward delay 1, 2 | 10..5000, 0..1000 | x * 499 + 1 ms | shifters 1, 2 case 6 |
| 46, 48 | feedforward delay synced | 1..768 | x * 16 | shifters 1, 2 case 10 |

Check: the default 746 of the levels gives 0.04 dB, the default decay
13105 gives 7.0 s. `corpus`

Reverb on, case 3: switching off empties the lines, the predelay and the
diffusers and sets S0 and S1 back to 1; switching on applies the freeze
mode. Freeze, case 13, with the reverb on: mode 1 sets S0 and S1 to 1,
mode 2 sets both to 0 (input cut, no damping), mode 3 sets S1 to 0 and S0
to 1. Feedback off, case 15: clears the feedback filters, the DC blocker and
shifter 0's buffers. `asm`

## 3. Prepare and the smoothers

`FUN_1801849a0` at rate `sr`. Each line group (4 groups of 4 lines, objects
of 0x90 bytes at `+0x610`, sized by `FUN_180188c60`) gets a buffer of the
next power of two at or over `(int)(sr + 0.5)`. Base lengths are
`sr * {877, 1303, 1907, 2053, 2711, 2659, 3527, 3803, 4397, 4373, 4957,
5449, 5741, 6217, 6761, 6997} / 44100`, which size 9 then replaces. The
lines glide to their targets by `6.615 / sr` a sample. Matrix at 45
degrees, decay 3.5 s, high decay 0.5 s, LFO step 0.5 / sr, size gain
1.8666667, dry and wet 0.5, envelope detector high pass at 20 Hz.

Thirteen smoothers of `{current, target, time, rate, coef}` at
`0x864 + 0x14 k` move `current += (target - current) * coef`, with
`coef = 1 - 0.37^(1 / (time * rate))`. All have time 0.005 s except S0 and
S1 (0.02 s, current and target 1) and S2 (control 31). S0..S5 use `sr`,
S6..S12 use `sr / 16`: the library meant to update those every 16 samples,
but its counter at `+0x191460` is never incremented, so they and every
coefficient that hangs off them run every sample with the slower
coefficient. `asm`

S0 input to the network, S1 damping share, S2 envelope, S3 width, S4
brightness, S5 decay, S6 peak frequency, S7 peak gain, S8 peak Q, S9 input
low cut, S10 input high cut, S11 feedback high cut, S12 feedback low cut.

Other rates: dry/wet glide `1 - 0.37^(45.4545 / sr)`, limiter glide
`1 - 0.37^(100 / 44100)`, output followers rise 0.004976 / keep 0.995 and
fall `exp(-110 / 44100)` (fixed, not scaled by the rate). DC blocker
coefficient `(8.946186e-7 - sr * 2.6722869e-12) * sr + 0.9157444`.

## 4. Signal chain, per frame (`FUN_180187290`)

The function sets MXCSR to flush-to-zero, denormals-are-zero and round
toward zero for its whole run, the maths calls included.

1. **Envelope** (mode not 1). Detector input is the sidechain
   `(L + R) / 2`, or the input `(L + R) * gain / 2`. A cookbook high pass,
   then a peak follower (attack and decay from 25, 26), S2 smooths it,
   `envdB = 20 log10(S2)` (`FUN_1801c6430`). Mode 1 sets the follower and
   S2's target to 0.
2. **Input gain** (0), then for pitch type 1 with feedback on, the
   feedforward shifters 1 and 2 on the stereo input:
   `L = L * unpitched + amp1 * shift1.L + amp2 * shift2.L`.
3. **Input filters**: S9, S10 tick; one-pole high pass and low pass by the
   bilinear transform without prewarping, `w = pi f / sr`,
   high pass `[1, -1, w - 1] / (w + 1)`, low pass `[w, w, w - 1] / (w + 1)`.
4. **Predelay** (reverb on): 172000-sample lines, read before the write.
   The length glides by 1.5e-4 a sample from 1.0. Linear read, or with
   high quality four-point Lagrange (`FUN_180183f40`).
5. **Diffusion**: eight allpass stages of 4096 samples a side
   (`FUN_180184170`), lengths `sr * {4.7377, 3.5953, 12.735, 9.3075,
   7.7731, 16.1125, 13.0412, 8.9123} ms`, gains `{0.75, 0.625, 0.7, 0.5,
   0.675, 0.712, 0.6, 0.583}`; out = `diff * d + (1 - d) * in`.
6. **Feedback in** (feedback on, pitch type 0): last frame's feedback
   signal `* gain * 8 * min(decay^-0.2, 1) * comp * S1`, added to both
   sides, where `comp = 0.5 + 0.7 p/12` for pitch `p >= 0` and
   `0.5 - 2 p/12` below.
7. `mid = (L + R) / 2` for the feedback send; S0, S1 tick.
8. **Network** (reverb on), inputs `L * S0`, `R * S0`:
   - LFO: phase += step, wrapped; line `k` at phase + k/16, `t = 2 frac`,
     `a = min(t, 1)`, `b = max(t, 1) - 1`, `lfo = 4((a - a^2) - (b - b^2))`;
     target length `= (lfo * depth + 1) * base`.
   - S5 ticks into the decay `T`; envelope mode 3 makes it
     `clamp(T + (max(envdB - thr, 0) + offset * thr) * scale, 0.25, 20)`.
     S4 ticks, high decay `= T * S4`.
   - One line a frame, round robin, gets its damping shelf from its base
     length: `g = 10^(-3 len / (sr T))`, `h` the same with the high decay,
     `K = 2 sr / (2 pi 2124.7974)`, `a1 = (1 - K)/(1 + K)`,
     `b0 = (g + h K)/(1 + K)`, `b1 = (g - h K)/(1 + K)`.
   - Each line: glide the length, read at `write - len` (linear, or with
     high quality the 4-point Hermite of `FUN_180188d10`), shelf,
     `y = shelf * S1 + read * (1 - S1)`.
   - Write index steps, then each line writes `sum_j M[k][j] y_j + in_k`,
     where `in_0 = in_1 = (L + R)/2` and the rest alternate L, R. `M` is the
     2 by 2 rotation `[[c, s], [-s, c]]` Kronecker-squared twice. It is
     orthogonal at any character; at 45 degrees it is a 16-point Hadamard.
   - Wet L = `(y0 + y1 + y2 + y4 + ... + y14) * size_gain / 8`, wet R the
     same with the odd lines. With the reverb off the wet is step 6's
     signal as it stands.
9. **Limiter**: followers on each wet side; target `max(L, R)` with the
   limiter on, 0.24999 with it off; glide; `wet *= 1 / max(4 x, 1)`.
10. **Feedback out** (feedback on, pitch type 0): `(R + L) mix/2 +
    (1 - mix) mid` into shifter 0, a DC blocker, a low pass and a high
    pass each made of two cascaded bilinear one-poles (`w = 4.881407 f /
    sr`, which puts -3 dB at f), into the feedback signal for the next
    frame.
11. **Peak filter**: S6..S8 tick; `w = 2 pi f / sr`,
    `k = (2Q - sin w)/(2Q + sin w)`, `a1 = -cos w (1 + k)`, `a2 = k`,
    `b0, b2 = (1 + k)/2 +- G (1 - k)/2` (Regalia-Mitra; `FUN_1801c97f0`
    has no division in it, so it is `sinf`, matching prepare's use of the
    sine).
12. **Levels**: wet dB, plus `2 * scale * (max(envdB - thr, 0) + offset *
    thr)` in envelope mode 2, to clamp(10^(dB/20), 0, 5.6). Dry and wet
    glide. Width: `side = (L - mid) * S3`; out = `(mid +- side) * wet + dry *
    input`.

## 5. The pitch shifter (`FUN_1801821f0`, `FUN_180182910`, `FUN_180183180`)

Three in the engine at `+0x190b70` (feedback, mono), `+0x190e68` and
`+0x191158` (feedforward, stereo). Prepare sets them to mode 3, 150 ms
grains, feedback gain 0, mix 1, clip gain 5 (clip on for the feedback one).
Per frame in mode 3:

1. A delay of `sr * 4.01` samples, length gliding by `4.41 / sr`, linear
   read. Case 6 sets it in ms clamped to `[sr/1000, size - 1]`, case 10 to
   `15 / tempo * v * sr + 1`.
2. A two-tap filter `((c + 1) x + (1 - c) x_prev) / 2`, `c = min(2 -
   ratio, 1)`, into a buffer of `sr * 0.8` samples.
3. A countdown spawns a grain (20 slots) every `(int)(len / density + 0.5)`
   frames, less a random jitter. The grain starts `5 + (ratio - 1) len`
   samples behind the write, less a random offset, and steps by the ratio
   `2^(semitones/12)`. The random numbers are MSVC `rand` steps on one
   static seed (`0x1803f67d4`, initial `0x3920a`) that every shifter in
   the library shares.
4. Each grain reads with four-point Lagrange under a `sin^2` window of 1024
   points, interpolated.
5. Clip, when on: `5 * c(0.04 x)` with `c(x) = (2 - |x/2|) x/2` inside
   `+-2`, `+-1` outside.
6. The delay is written with the input plus the shifter's own feedback,
   which three biquads and a gain of 0 make silent.
7. Out = `mix * grains + (1 - mix) * input`.

Case 15 (feedforward pitch) also sets density 2.25, no time jitter, place
jitter 0.3 (0 at unison) and a grain length of `sr * 0.15` less
`6.25 (p - 12)` above an octave, or less `6.25 p + 75` below one.

## 6. What `luxeverb.rs` does

All of the above for states of version 7 to 9. Left out, and silent in FL
too: the three biquads in each shifter's own feedback, whose gain is 0;
the meters. Where it can differ from FL:

- FL runs the process with round-toward-zero arithmetic; Rust rounds to
  nearest. CRT `powf`, `sinf`, `cosf`, `log10f`, `expf` are Rust's.
  Last-bit differences, which a long tail can carry.
- The shifters' random seed is the library's static, so in FL two
  Luxeverbs (or any other user of that static) draw from one stream.
  `luxeverb.rs` gives each plugin its own, starting at `0x3920a`.
- The envelope's sidechain with no sidechain buffer reads FL's output
  buffer; here it reads the input.
- After a sample-rate change FL pushes the controls without the sync
  pass, so the synced delays win until the next tempo event; here the sync
  pass follows every rate change too. `inferred`

Unit tests: the prime table, the level and decay curves, the matrix's
orthogonality, the damping shelf's -60 dB at DC and Nyquist over the two
decays, the diffuser's energy, both interpolators on a line, the shifter
moving a tone an octave, the peak filter's gain, the measured RT60 against
the decay control, and every corpus and preset state parsing and playing
finite and below 8.

## Open

- No null test: no render test ships and FL cannot run here.
- The feedforward controls (39..48) have no version 9 state in the corpus,
  so their order and the 1719 fill are read from the loader only.
