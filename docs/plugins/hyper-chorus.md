# Hyper Chorus: state, parameters, signal chain

Recovered from `Plugins/Fruity/Effects/Hyper Chorus/` of FL Studio 2026:
`Hyper Chorus_x64.dll`, a Delphi shell (image base 0x400000), and
`Engine_x64.dll` beside it, an MSVC 2019 C++ DSP engine (image base
0x180000000, PDB path `QuadrachorEngine.pdb`) that does all the sound. Two
headless Ghidra imports, plus objdump listings of the shell around the
engine calls. Corpus: 20 states in 7 `.flp` files and the 12 shipped
presets. Every one is version 2, 52 bytes.

Hyper Chorus links none of the shared Delphi classes in
`docs/plugins/fl-fx.md` (TStereoFlanger, TDelay_32FS, TFruityReverb) and
shares nothing with Fruity Chorus. Frequency Shifter also keeps its DSP in
an `Engine_x64.dll` beside it, but that is a different engine whose one
export is named `dispatch`.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `asm`, `inferred`.

## How the shell drives the engine

The engine exports one function, `DispatchToEngine(op, engine, a, b, f32
value)` at `0x180001050`. The shell loads the DLL from its own directory
(`0x006ea220`), keeps the function at `this+0x15c` and the engine at
`this+0x154`.

| op | what it does | shell caller |
|---|---|---|
| 0 | create; `*a` = new engine (`FUN_180001550`) | `0x006ea220` |
| 1 | destroy | `0x006ea350` |
| 2 | sample rate = `value` | Dispatcher, `0x006e86b8` |
| 3 | set parameter `a` to `value` (0..1) | `0x006e90e0`, from ProcessParam |
| 4 | read parameter `a` into `*b` | `0x006e9080`, from the constructor |
| 5 | render `value + 0.25` frames, interleaved stereo, from `a` to `b` | Eff_Render `0x006e9f40` |
| 6, 8 | parameter name | |
| 7 | parameter text | |
| 9 | reset | Dispatcher, `0x006e86fd`, when the host switches the plugin off |

The engine takes a critical section around op 3 and op 5. Op 5 also sets
MXCSR to `| 0xe040` for the block: flush to zero, denormals are zero, and
rounding toward zero. `asm`

## 1. State layout (event 213, 52 bytes)

`SaveRestoreState` @ `0x006e8960`. Save writes `i32 2`, the 11 parameters
from `this+0x168` (44 bytes), then the visualiser gain slider's position.
Restore reads the version and leaves the plugin alone when it is over 2.
Otherwise it reads the 44 bytes and the gain, then runs GetValue and
UpdateValue|UpdateControl (0x11) through ProcessParam for each of the 11
parameters, in order. Nothing is range-checked; ProcessParam clamps. `loader`

The wheel's `Tag` is the parameter index (`dfm`). The delay wheel has no
tag and is parameter 0.

| byte | idx | parameter (DFM object) | range | default | corpus (20) | presets |
|---|---|---|---|---|---|---|
| 0..3 | | version | | 2 | 2 x20 | 2 |
| 4..7 | 0 | Delay (`DelayWheel`) | 0..1000 | 400 | 500 x6, 0 x3, 1000 x2, 294..562 | 0..1000 |
| 8..11 | 1 | Feedback (`FeedbackWheel`) | 0..1000 | 500 | 500 x6, 0 x5, up to 919 | 0..919 |
| 12..15 | 2 | Mod rate (`ModRateWheel`) | 0..1000 | 400 | 500 x5, 0..736 | 0..689 |
| 16..19 | 3 | Mod amount (`ModAmountWheel`) | 0..1000 | 400 | 500 x6, 205..783 | 259..713 |
| 20..23 | 4 | Movement (`MovementWheel`) | 0..1000 | 300 | 500 x5, 179..819 | 136..1000 |
| 24..27 | 5 | Move rate (`MovementSpeedWheel`) | 0..1000 | 300 | 500 x5, 173..805 | 300..1000 |
| 28..31 | 6 | Delay lines (`DelayLinesSwitch`, "4" / "8") | 0..1 | 0 | 0 x16, 1 x4 | 0, 1 |
| 32..35 | 7 | Low pass (`LowPassWheel`) | 7140..22000 | 22000 | 22000 x17, 19247 x2, 14250 | 10419..22000 |
| 36..39 | 8 | High pass (`HighPassWheel`) | 120..10900 | 120 | 120 x8, up to 10900 | 120..10900 |
| 40..43 | 9 | Mix (`DryWetWheel`) | 0..1000 | 500 | 500 x4, 0..1000 | 350..900 |
| 44..47 | 10 | LFO phase difference (`LFOPhaseDifferenceWheel`) | 0..1000 | 100 | 1000 x8, 500 x7, 0..800 | 0..1000 |
| 48..51 | | visual gain (`GainWheel`, tag -1, "Visual gain") | -120..120 | 0 | 0 x18, 54 x2 | 0 |

The defaults come from the shell's constructor (`0x006e8b90`). It reads
each parameter back from the engine with op 4 and stores `Min +
Round(v * (Max - Min))`, from the engine constructor's 0.4, 0.5, 0.4, 0.4,
0.3, 0.3, 0, 1, 0, 0.5 and 0.1. Then it sends all 11 through ProcessParam,
so a new plugin tells its engine every parameter once. `Default.fst` is
different: it has 500 everywhere but the filters. `loader`, `preset`

The gain slider scales the visualiser and nothing else. Its `OnChange` is
`GainWheelChange`, not `OnParamChange`. `dfm`

## 2. Parameter semantics

ProcessParam @ `0x006e9120`: on `REC_UpdateValue` the integer goes to
`this+0x168[idx]`, and the engine gets `v = clamp((n - Min) * wheel+0x3cc,
0, 1)`, in f64, then rounded to f32. `wheel+0x3cc` is `1 / (Max - Min)`;
the constructor's rounding is its inverse. `asm`

The engine's set call (op 3) stores `v` at `engine+0x48[idx]` and queues it
(`FUN_180002290`). The queue holds (index, now, target). A parameter that
is not queued yet goes in with `now` read back from `engine+0x48`, which is
the value just stored, so `now = target`. A parameter already queued keeps
its `now` and takes the new target. At each sample of a render, before the
DSP, the queue runs from its last entry to its first (`FUN_180001740`):

```
left = target - now
if |left| >= 1e-4:  now += left * glide;  apply(idx, now)
else:               apply(idx, target);   drop the entry
```

`glide = 1 - 0.09999999999999998^(1 / (0.05 SR))` after op 2 (1.0 before):
90% of the way in 50 ms. So a value told once lands at the next sample. A
value told twice before a block glides from the first to the second. That
is what a state load into a new plugin does: the constructor queued the
defaults, so the state glides in from them over about 200 ms. `asm`

`apply` is `FUN_180001ee0`:

| idx | what changes |
|---|---|
| 0..5, 10 | both quads, `FUN_1800024e0`, below |
| 6 | eight lines = `v >= 0.5` |
| 7 | low-pass at `714 * 2^(4.9454355 v)` Hz, 714 Hz .. 22 kHz (`FUN_180002170`) |
| 8 | high-pass at `12 * 2^(6.50515 v)` Hz, 12 .. 1090 Hz (`FUN_180002200`) |
| 9 | `w = (2v)^2 / 2` under 0.5, else `1 - (2 - 2v)^2 / 2`; wet `sqrt w`, dry `sqrt(1 - w)` |

The text the engine gives the filters prints the same Hz. A high-pass
whose stored `v` is 0 prints "HP Off", and the render skips it (section 3).
`asm`

Each quad (four lines a side) reads its four rows of the table at
`0x18002ba10`, eight f32 per line: delay ms, LFO Hz, depth, movement Hz,
each as low and high. The first quad has rows 0..3, the second rows 4..7.
`along(k) = (high - low) * v + low`, in f32:

| line | delay ms | LFO Hz | depth | movement Hz |
|---|---|---|---|---|
| 0 | 2 .. 6 | 1.5 .. 2.47 | 20 .. 80 | 0.33 .. 3.56 |
| 1 | 3.5 .. 8.25 | 1.57 .. 3.41 | 20 .. 80 | 0.29 .. 2.66 |
| 2 | 6.5 .. 10 | 0.38 .. 3.64 | 20 .. 80 | 0.26 .. 3.78 |
| 3 | 5 .. 12.8 | 1.29 .. 3.68 | 20 .. 80 | 0.14 .. 2.98 |
| 4 | 7.1 .. 16.6 | 3.41 .. 5.03 | 20 .. 80 | 0.44 .. 5.06 |
| 5 | 9.1 .. 15.1 | 2.47 .. 5.37 | 20 .. 80 | 0.66 .. 8.08 |
| 6 | 8.3 .. 16.8 | 3.01 .. 6.02 | 20 .. 80 | 0.93 .. 6.02 |
| 7 | 9.6 .. 17.7 | 3.3 .. 6.5 | 20 .. 80 | 0.41 .. 10.22 |

```
0 delay      along(delay) * 0.001 * SR              samples
1 feedback   v * 0.7
2 LFO step   along(LFO) / SR                         cycles a sample
3 depth      along(depth)^2 * 5e-5                   0.02 .. 0.32 of the delay
4 movement   v * 0.499
5 move step  along(movement) / SR
10 offset    v^3 * 0.25                              the right side's LFO is up to a quarter cycle ahead
```

Op 2 stores the rate and computes the high-pass fade length `trunc(0.01 SR)`
and its step. It recomputes nothing else: the delays, steps and filters keep
the rate they were set at until a parameter is applied again. `asm`

## 3. Signal chain (`FUN_180001740`, per frame)

```
run the queue (section 2)
quad A (in L, in R)                                   // FUN_180002920
if eight lines:
    quad B (in L, in R)
    wL = (sum A.outL + B.outL[3] + [2] + [1] + [0]) * 0.28117    // -11 dB
    wR = likewise
else:
    wL = sum A.outL * 0.56234;  wR = sum A.outR * 0.56234         // -5 dB
low-pass, direct form I, both sides
high-pass: see below
out = in * dry + w * wet
```

With four lines quad B does not run at all: its lines, LFOs and outputs
hold until it runs again.

One quad, per line `i` (`FUN_180002920`, SSE, four lines in one register):

```
shape(x) = (min(x,1) - min(x,1)^2) - (h - h^2),  h = max(x,1) - 1     // x = twice the phase
lfoL     = shape(2 phase)                  in -0.25 .. 0.25, a parabolic sine
phaseR   = frac(phase + offset)
pan      = shape(2 turn) * 4 * movement + 0.5
gainL    = sqrt(1 - pan) * 1.4144272;  gainR = sqrt(pan) * 1.4144272
dL       = lfoL * 4 * (delay * depth) + delay;  dR likewise from phaseR
ringL[i][w] = gainL * inL + feedback * outL[i]
outL[i]  = cubic(ringL[i], w - trunc(dL), frac(dL))
then the right side the same way
w += 1 (u16);  phase = frac(step + phase);  turn = frac(turn step + turn)
```

`cubic` is 4-point Catmull-Rom between `y1 = ring[j - 1]` and `y2 = ring[j
- 2]`, with `y0 = ring[j]` and `y3 = ring[j - 3]`:

```
c1 = (y2 - y0) * 0.5;  a = (y1 - y2) + c1;  c3 = (y3 - y1) * 0.5 + (a + (y1 - y2))
out = ((t c3 - (c3 + a)) t + c1) t + y1
```

So a line is heard `delay + 1` samples late, and its feedback comes round
every `delay + 2` samples. Each ring is 65536 f32 and the write place a
u16. Eight rings a quad: 2 MB each. The movement LFO pans each line
between the left input and the right one at equal power. The left output
is the left lines only. `asm`

The biquads (`FUN_180003030`, types 1 and 2) are the cookbook low-pass and
high-pass with `alpha = sin w`, which is a Q of 0.5, computed in f64 and
stored as f32. The engine adds `b1 x1` before `b0 x` in the low-pass and
in the high-pass fade, and `b0 x` first in the full high-pass.

High-pass: with its stored `v` at 0 and the fade at 0, it is skipped and
its state holds. Otherwise it fades over `trunc(0.01 SR)` samples
(`FUN_180001d20`): `out = x (1 - g) + y g`, `g = fade / length`, the fade
counting up while the high-pass is on and down after it is switched off.

Reset (op 9) empties both biquads, sets the fade to 0, and empties quad A's
rings and outputs with its write place at 0. The LFOs keep their phases.
Quad B is left as it is. `asm`

The LFO and movement phases start at `rand() / 32767` from the MSVC
runtime (`FUN_1800023c0`), four for quad A's LFOs, four for its movement,
then the same for quad B: 16 numbers from the thread's `rand()` sequence
for each new engine. The runtime is static in this DLL, so the first Hyper
Chorus FL loads on a thread gets the sequence from seed 1 and the next ones
carry on from there. `asm`, `inferred`

## 4. What `crates/plugins/src/hyper_chorus.rs` does differently

- Every instance starts its LFOs at the first 16 numbers of seed 1. In FL
  only the first instance on the loading thread does.
- f32 arithmetic rounds to nearest. FL's block rounds toward zero and
  flushes denormals, which can move the last bit of a sample.
- After a new rate it applies every parameter again, where FL keeps the
  old delays, steps and filters until the next parameter change.
- The state's parameters are range-checked, as the other plugins here do.
  FL reads any values and clamps them in ProcessParam.
