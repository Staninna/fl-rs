# Pitch Shifter: state, parameters, signal chain

Recovered from `Plugins/Fruity/Effects/Pitch Shifter/Pitch Shifter_x64.dll`
(FL Studio 2026, Delphi native plugin) and the two engines it loads from
`Shared/ildsp_x64.dll`. The shell holds the state and the 13 controls and
turns them into floats. All the sound comes from the engines. Corpus: 15
states in the shipped `.flp` files (7 projects, all version 7, 75 bytes,
both engines in use, none with tempo sync on) and 30 preset states. A
render test exists: `Data/System/Render tests/Pitch Shifter.flp` (general
engine, +12 semitones). The corpus render plays it; its output has not been
compared with FL's.

Everything below was read in Ghidra unless the text says otherwise. The PSOLA
engine's set cases 4 and 8 (the delay clamps) were read from objdump. Guesses
are marked as guesses.

## How the plugin is wired

The shell is `TTestPlug`, VMT `0x72bb08`.

| function | address |
|---|---|
| SaveRestoreState | `FUN_0072c630` |
| ProcessParam (control to 0..1) | `FUN_0072cda0` |
| push one control to the engines | `FUN_0072d0f0` |
| PSOLA formant with correction | `FUN_0072d020` |
| tempo from ProcessEvent | `FUN_0072ccf0` |
| Eff_Render | `FUN_0072de80` |
| detector window to PSOLA | `FUN_0072dfa0` |

It creates two engines from `Shared/ildsp_x64.dll`:

| engine | factory | class | size | prepare | set | process | clear |
|---|---|---|---|---|---|---|---|
| PSOLA | `PSOLA_CreateInstance` 0x1801ab540 | `PSOLAImpl` | 0x850 | `FUN_18017f700` | `FUN_1801804b0` | `FUN_180180830` | `FUN_1801ab6b0` |
| general | `GeneralPitchShifter_CreateInstance` 0x1801aa290 | `GeneralPitchShifterImpl` | 0x2f8 | `FUN_180161fc0` | `FUN_180162760` | `FUN_180162f90` | `FUN_1801aa3a0` |

The PSOLA process function calls three helpers: the split and gate
`FUN_180181590`, the pitch detector `FUN_180181730` and the synthesis
`FUN_180181d60`. Its vtable slot 5 returns the latency, the field at `+0x98`.

The shell keeps both engines alive and sends every control to both.
`Eff_Render` runs one of them: PSOLA when the state's effect field is 0, the
general engine for any other value. The other engine is not run.

The sample-rate case of the dispatcher (case 4) prepares both engines, pushes
every control, then calls `FUN_0072dfa0`. That sends PSOLA set case 9 with a
detector window of 1024 (fast) or 2048 (accurate) samples, read from
`DAT_00786568`, and reports the latency. A tempo event sends PSOLA case 7 and
general case 9.

The general engine is not the shifter class inside Luxeverb's shimmer reverb.
It has its own functions, 0.2 s grain buffers where Luxeverb's are 0.8 s,
delay modes 1 to 4, a tanh-table clip, feedback biquads that run, and a
`rand` static of its own at `0x1803f67d0`, seeded with 0x3920a. The two are
of one family: the four-point Lagrange read and the MSVC `rand` step are the
same code.

## 1. State layout (event 213, versions 6 and 7)

`SaveRestoreState` (`FUN_0072c630`) reads version 6 (69 bytes) and version
7 (75 bytes). It ignores any other version.

| byte | type | field |
|---|---|---|
| 0..3 | i32 | version, 6 or 7 |
| 4..55 | i32[13] | the controls, in the order of section 2 |
| 56..59 | i32 | the formant correction wheel again, a copy of control 11 |
| 60 | u8 | tempo sync, non-zero is on |
| 61..64 | i32 | effect: 0 PSOLA, 1 general |
| 65..68 | i32 | pitch detector: 0 fast, 1 accurate |
| 69..72 | i32 | version 7 only: display length (editor) |
| 73, 74 | u8 x2 | version 7 only: two display bytes (editor) |

The display fields do not reach the sound. After a load the shell pushes
every control to the engines and then sends the detector window. The engines
are not cleared.

## 2. Control map (`FUN_0072d0f0`)

`ProcessParam` (`FUN_0072cda0`) turns each control `v` into
`x = clamp((v - min) / (max - min), 0, 1)`, worked out in doubles.
`FUN_0072d0f0` then sends `x`, or a number made from it, to set cases of the
engines. Pitch and formant become semitones as `st = (x - 0.5) * 24`, so
-1200..1200 cents is -12..12 semitones, and both engines turn semitones into
a ratio `2^(st / 12)`.

| # | control | range | default | PSOLA set case | general set case |
|---|---|---|---|---|---|
| 0 | pitch, cents | -1200..1200 | 0 | 0 pitch `st`; and 1 the corrected formant, unless correction is full | 0 pitch `st` |
| 1 | formant, cents | -1200..1200 | 0 | 1: `st` when correction is full, else the corrected formant | none |
| 2 | analysis channel: left, right, both | 0..2 | 0 | 2: `v + 1` (1 left, 2 right, 3 both) | none |
| 3 | mix | 0..2400 | 2400 | 3: `x` | 5: `x` |
| 4 | grain duration, ms * 100 | 2000..15000 | 8500 | none | 1: `v / 100` ms, so 20..150 ms |
| 5 | density * 100 | 100..500 | 300 | none | 2: `v / 100`, so 1..5 |
| 6 | jitter (grain timing) | 0..100 | 30 | none | 3: `x` |
| 7 | rand (grain position) | 0..100 | 20 | none | 4: `x` |
| 8 | delay | 1..768 | 768 | sync off: 4, `999 x + 1` ms; sync on: 8, `16 x` sixteenths | sync off: 6, the same ms; sync on: 10, the same sixteenths |
| 9 | feedback | 0..100 | 0 | 5: `x` | 7: `x` |
| 10 | delay mode | 0..3 | 0 | 6: `v + 1` | 8: `v + 1` |
| 11 | formant correction | 0..65536 | 0 | 1: the corrected formant | none |
| 12 | pitch mode: relative, C4, C5, C6 | 0..3 | 0 | 10: `v + 1` | none |

The defaults come from the one untouched state in the corpus, in a Mehran
Abbasi project, with the PSOLA engine and the fast detector. They match the
editor form's wheels. That this state is FL's default is a guess.

**Formant correction.** `FUN_0072d020` gives the PSOLA formant as

```
formant = clamp(formant_st + pitch_st * (1 - x11), -12, 12)
```

At correction 0 the formant follows the pitch. At correction 1 (65536) it
stays where the formant control puts it. "Full" in the table means
`x11 >= 1`: then control 1 sends its own `st` and control 0 leaves the formant
alone. The general engine has no formant.

**Delay.** With sync off, the delay is 1 to 1000 ms. The engines clamp it to
between 1 ms and the length of their delay line. With sync on it is 0 to 16
sixteenths. The engine works it out as `v * rate * 15 / tempo` samples, with
the tempo it holds when the delay is set. A later tempo change does not move
the delay until the control is pushed again. The general engine clamps the
synced delay to the line's length only; PSOLA also keeps it at 1 sample or
more.

## 3. General engine signal chain (`FUN_180162f90`)

Per frame, both sides:

1. **Delay read.** When the delay mode is not 1, read the delay line (below).
2. **Shifter input.** Mode 2: input plus last frame's feedback. Mode 3: the
   delayed signal. Otherwise the input.
3. **Pre-filter.** A two-tap low pass into the grain buffer,
   `buf = ((c + 1) x + (1 - c) x_prev) / 2` with `c = min(2 - ratio, 1)`.
   It passes the input as it is for ratios up to 1 and averages two samples
   at ratio 2. The grain buffers hold `0.2 * rate` samples a side.
4. **Grain spawn.** A countdown runs per sample. When it reaches 0 the engine
   draws `rand` and sets it to `trunc(len / density + 0.5)` plus up to
   `jitter * 0.04535147 * rate` samples (2000 samples at 44.1 kHz at full
   jitter). If one of the 20 grain slots is free, a new grain starts at
   `write - back - 5`, less up to `rand * 0.04535147 * rate` samples from a
   second draw. `back` is `(ratio - 1) * len` above ratio 1 and 0 otherwise,
   so a grain that reads faster than the buffer fills does not pass the write
   head. The grain keeps the ratio it starts with as its step. With all 20
   slots in use the spawn is skipped.
5. **Grains.** Each live grain reads its buffer at its place with the
   four-point Lagrange interpolator, weighted by a 1024-point `sin^2` window
   read linearly by how far the grain has run. Its place moves by its step.
   It stops when its length runs out.
6. **Clip.** The grain sum goes through a 1001-point `tanhf` table over -5..5,
   read at `100.2 x + 500` with a straight line between points. That is
   `tanh(1.002 x)` to within 2e-4. Prepare switches the clip on and nothing
   in the shell turns it off.
7. **Feedback.** When the mode is not 1, three biquads in a row (transposed
   direct form II) filter the feedback source on each side, and the result
   is scaled by the feedback control. The coefficients are fixed tables, one
   set each for 44.1, 48, 88.2, 96 and 192 kHz and one for any other rate.
   They pass 1 kHz and cut the lows; their exact shapes are not named. If
   the clip were off, the feedback would go through the clip instead.
8. **Delay write and output.** `out = mix * wet + (1 - mix) * input`.

The delay modes (control 10 plus 1):

| engine mode | shifter reads | feedback source | written to the line | wet |
|---|---|---|---|---|
| 1 | input | none | nothing | shifted |
| 2 | input + feedback | delayed | shifted | shifted |
| 3 | delayed | shifted | input + feedback | shifted |
| 4 | input | delayed | shifted + feedback | shifted + feedback |

So mode 2 shifts each echo again, mode 3 puts the delay ahead of the shifter,
and mode 4 is a feedback echo of the shifted sound that is not shifted again.
Any other mode value would feed zeros.

The delay line is stereo and holds `trunc(4.01 * rate)` samples. Its read
length glides to the target by `(target - cur) * 4.41 / rate` each frame it
is read, and the read interpolates linearly between two samples. It starts
at `0.1 * rate`. The glide runs only when the mode is not 1.

## 4. PSOLA engine signal chain (`FUN_180180830`)

Per frame:

1. **Delay read.** As in the general engine, when the mode is not 1. In modes
   2 and 3 the target is shortened by the latency, and kept at 1 sample or
   more. Mode 4 reads the full target.
2. **Shifter input.** Mode 2: input plus feedback. Mode 3: delayed.
   Otherwise the input.
3. **Voiced/unvoiced gate** (`FUN_180181590`). The analysis channel picks
   left, right or the mean of both. A biquad high pass filters it, with fixed
   coefficients per rate (the same six rate cases). The engine tracks a
   zero-crossing rate:
   `zc = |sign - sign_prev| * 0.05 + zc * 42115.5 / rate`. While `zc <= 0.6`
   the gate rises by `66.67 / rate` a sample toward 1, else it falls toward
   0, so a full swing takes 15 ms. Voiced is `gate * input` and unvoiced is
   `(1 - gate) * input`, on both sides, with one gate for both.
4. **Latency lines.** Six rings (voiced, unvoiced and dry, each side) of
   `trunc(1.2 * frame_len)` samples. Each frame the engine writes the newest
   sample and reads the oldest. The late voiced part goes into the source
   ring that grains read, `0.09070294 * rate` samples a side (4000 at
   44.1 kHz).
5. **Pitch detector** (`FUN_180181730`). The voiced part, not yet delayed, on
   the analysis channel, goes into a frame under a `sin^2` window of
   `frame_len` samples. `frame_len` is the shell's window (1024 or 2048)
   times `rate / 44100`, capped at 8915. When the frame is full:
   - A zero-padded FFT of 4096 points, 8192 above 89999 Hz, divided by the
     size. The engine raises each bin's power to 0.1.
   - Up to 100 local peaks between 60 and 1500 Hz, each placed and sized on a
     parabola through three bins.
   - The highest peak is found. Every peak before twice its index is a
     candidate. For each candidate `f0` (in bins) the engine sums
     `cos(2 pi k / f0) * spec[k] * exp(-(k - f0)^2 / (2 f0)^2)` over `k` from
     `f0 / 4` to a quarter of the FFT size. The candidate with the highest
     sum wins, ties going to the later one.
   - The result in Hz is `bin * rate / size`.

   Every sample the engine smooths the frequency,
   `f0s = f0 * s + f0s * (1 - s)` with `s = 1.1337868e-7 * rate` (0.005 at
   44.1 kHz), and keeps it between 60 and 1500 Hz. It takes the max first,
   so a NaN comes out as 60. The detector starts at 60 Hz.
6. **Analysis marks.** Once a period the engine sets a mark one period behind
   the source ring's write place, then sets the period to `rate / f0s`
   samples.
7. **Grain placement.** Once a synthesis period a grain starts at the last
   mark, if one of 20 slots is free. The synthesis period is
   `period / ratio` in relative mode, or a fixed note divided by the ratio in
   the other three: `rate * 0.0076445 / ratio` (130.8 Hz, FL's C4),
   `0.00382225` (261.6 Hz, C5) and `0.001911125` (523.3 Hz, C6). The grain
   is `2 * period / formant_ratio` samples long and reads the source ring at
   step `formant_ratio`. The window and the Lagrange read are the general
   engine's.
8. **Synthesis** (`FUN_180181d60`). The grain sum plus the late unvoiced part.
9. **Clip, feedback, delay write.** As in the general engine, with the same
   tanh table read at `(100.2 / gain) x + 500` with gain 1, and the same
   feedback filters and modes.
10. **Output.** `out = mix * wet + (1 - mix) * dry`, where dry is the late dry
    ring, so the dry part keeps time with the wet part.

**Latency.** The reported latency is the latency lines' length,
`trunc(1.2 * frame_len)`: 1228 samples for fast and 2457 for accurate at
44.1 kHz. The rings hold one sample less than that, so the output is
`latency - 1` samples late. The general engine adds none. Set case 9 (the
detector window) clears the frame and the latency lines and rebuilds the
frame window.

The prepare function sets a 2048-sample window at 44.1 kHz, 60 Hz for the
detector, a period of 500, relative pitch mode and mix 1. The shell
overwrites all of these straight after.

## 5. What `pitch_shifter.rs` builds

`crates/plugins/src/pitch_shifter.rs` has the shell (`PitchShifter`), both
engines (`General`, `Psola`) and the shared helpers: the Lagrange read, the
`sin^2` window, the tanh table, the feedback biquads with their per-rate
coefficient bits, the delay line and the grain slots. It plays whichever
engine the state's effect field picks and reports the PSOLA latency, or 0 for
the general engine. `max_latency()` is the accurate window's latency at the
current rate.

It loads versions 6 and 7 and refuses the rest. It also refuses a state of
the wrong length, or one with a control outside its range; what FL does with
an out-of-range control was not read. It reads the sync byte, the effect
word (0 is PSOLA) and the low byte of the detector word. A load pushes every
control and the detector window and leaves the engines' buffers alone. A
sample-rate change prepares both engines, sends them the plugin's last tempo,
and pushes everything. FL's prepare sets the tempo to 120; whether FL sends
the host tempo again after a rate change is unknown. `reset()` clears the
buffers, delay lines and feedback filters of both engines, as `FUN_1801aa3a0`
and `FUN_1801ab6b0` do.

Exact, as read: the state layout, the control map and the formant
correction, every set case the shell sends, both engines' process loops, the
delay modes, the gate, the detector's peak picking and harmonic sum, the
grain placement, the clip and the feedback filters.

Approximated:

- IPP's FFT is a radix-2 FFT in doubles, with the same division by the size.
  The CRT's `pow`, `cos`, `exp` and `tanh` are Rust's. Results may differ in
  the last bits.
- The tempo-sync load order. FL pushes the controls before it applies the
  state's sync byte, so a synced delay may still be set as milliseconds right
  after a load. The code applies the sync byte first. `fidelity()` reports
  Approximate when sync is on and the delay mode is not 0, and Exact
  otherwise. No corpus state has sync on.
- The general engine's `rand` is a library static in FL, so all instances
  in the process draw from one sequence. The code gives each instance its
  own seed, starting at 0x3920a. A prepare does not reset it.

Unknown or guessed:

- The defaults (section 2) are a guess from one untouched state.
- The PSOLA set cases 4 and 8 clamps were read from objdump, not Ghidra.
- What the feedback filters are meant to be. The code holds them as
  coefficient bits.
- Whether it matches FL. The corpus render runs with this plugin in place,
  but no render, the render test's included, has been compared with FL's
  output. The unit tests (11, including a play test of the 15 corpus states
  and every preset) pass.

The Lagrange read and the `rand` step also exist in `luxeverb.rs`. They are
copies, not shared code, and could move to the `dsp` crate.
