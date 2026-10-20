# Emphasis: state, parameters, signal chain

Recovered from `Emphasis_x64.dll` (FL Studio 26.1.7, Delphi shell, image
base 0x400000) and `Shared/ildsp_x64.dll` (MSVC C++, image base
0x180000000, the library of Spreader, Luxeverb and Pitch Shifter too).
Headless Ghidra 11.4.2: the shell's slots and callees, the limiter's
constructor, `setParam`, `process` and the classes it is built from, plus
capstone listings of the cases Ghidra's switch recovery dropped. Corpus:
Astes - Bien Duro, DEDZII - NIGHT TERROR, Mehran Abbasi - With You (version
8), P.Priime - Use Me (version 10), the render test `Emphasis.flp` (version
7); 21 presets, all version 10. The port is `crates/plugins/src/emphasis.rs`
(the shell and the `Limiter` class) and `emphasis_dsp.rs` (the classes).

Emphasis shares nothing with Maximus. The shell keeps the parameters and
the GUI (LUFS meters, the gain reduction history); every sample goes
through one instance of the C++ class `Limiter`.

## The shell

`CreatePlugInstance` @ `0x00704b90` -> constructor `0x00704c00`, VMT
`0x00704918` (`TTestPlug`, size 0x230).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x00705720 |
| 3 | SaveRestoreState | 0x00706090 |
| 5 | ProcessEvent | 0x00705db0 |
| 6 | ProcessParam | 0x007063a0 |
| 7 | Eff_Render | 0x00707240 |

`0x16c` holds the limiter's interface, `LimiterImpl` (vtable
`0x1803dff40`): `+0x08` init(rate, max block) -> `Limiter` slot 2,
`+0x10` setParam(index, float) `0x1801ab050`, `+0x18` getParam, `+0x20`
process `0x1801ab070` -> `FUN_180167330`, `+0x28` latency (reported to FL
as PDC by `FUN_00707910`), `+0x58..0x98` meters.

`Eff_Render` (`0x00707240`) deinterleaves, calls process on planar
buffers, interleaves, then reads meters. Nothing else touches the sound.

### State (event 213)

`SaveRestoreState` writes: `i32 version` (10), the 13 parameters as i32
(`+0x18c`), `i32` LUFS meter mode (`editor+0x870`), `i32` (version > 6,
`editor+0x878`), `u8` low latency (version > 8), `u8` instant recovery and
`u8` DC blocker (version > 9). 60 bytes up to version 6, 64 for 7 and 8,
65 for 9, 67 for 10; FL reads up to 10. Before version 8 the envelope
order is `clamp(p2 + 1, 1, 4)`. A version below 9 loads the three switches
as 0.

After reading, the shell sets every parameter through ProcessParam in
order, then 11, 12 and 10 again, then instant recovery, the DC blocker and
low latency (`FUN_00705270`, `FUN_007052e0`, `FUN_00705230`).

### Parameters (`FUN_00706860`, DFM wheels)

| # | control (DFM) | range | value to `setParam(#, x)` |
|---|---|---|---|
| 0 | BypassSwitch | 0..1 | raw |
| 1 | DeltaSwitch | 0..1 | raw |
| 2 | EnvOrderWheel | 1..4 | raw - 1 |
| 3 | TruePeakSwitch | 0..1 | raw |
| 4 | channel mode, L/R, M/S | 0..1 | raw |
| 5 | emphasis mode: Versatile, Loud, Transient, Steady | 0..3 | raw |
| 6 | InputGainWheel | 0..10000 | `24 n` dB (`FUN_007043f0(n, 0, 24, 1)`) |
| 7 | LinkingWheel | 0..10000 | `n` |
| 8 | EmphAmountWheel | 0..10000 | `n` |
| 9 | OutputGainWheel (ceiling) | 0..10000 | `FUN_00705110`: `n == 1 ? 0 : 20 log10((exp(n ln 11) - 1) * 0.1)` dB, `-inf` where that gain is not above 0 (an integer compare of its bits) |
| 10 | LoudnessCompBtn (level matching) | 0..1 | raw |
| 11 | OversamplingCombo, Off 2x 4x 8x 16x | 0..4 | raw |
| 12 | EmpHardnessWheel | 0..9 | raw |
| 0x19 | low latency (options menu) | byte | raw |
| 0x1a | FastRecoveryBtn, instant recovery | byte | raw |
| 0x1b | DC block | byte | raw |

`n = raw / 10000`. `ln 11` is a `.bss` float written at startup
(`0x005f0f37`, `ln(11.0)`); 0.1 at `0x00752944`. FL's "Default" preset is
`[0,0,1,0,0,0,0,10000,10000,9912,0,2,7]`, switches `0,1,1`; the port starts
there.

## The limiter (`ildsp_x64.dll`)

`Limiter_CreateInstance` @ `0x1801aafd0`, constructor `FUN_18016adc0`
(0xa02a80 bytes), vtable `0x1803ddc48`: slot 0 setSampleRate
`0x1801665b0`, 1 process wrapper, 2 prepare(rate, block) `0x180166970`
(the one the shell calls), 3 reset `0x180166f40`. `prepare` sets every
member up at the rate (the fast stage and its lookahead at the
oversampled rate), runs `FUN_18016a630` (fixed times, below) and
`FUN_18016a400` (every parameter set again).

### Members

| offset | class | role |
|---|---|---|
| 0x40..0xd8 | float buffers | block scratch: level, 16x level, 16x audio, dry, input copy, level-match, gains |
| 0xe0 + 0x250 ch | `GRAdaptiveRel` | slow stage |
| 0x580 + 0x1e0 ch | `GRStatic8` | fast stage, at the oversampled rate |
| 0x940 + 0x1c0 ch | `GRStatic1` | M/S stage |
| 0xcc0 + 0x1e0 ch | `GRStatic8` | last stage |
| 0x1080 + 0x100068 ch | `DelayLerp` | slow stage lookahead |
| 0x201150 + 0x100068 ch | `DelayLerp` | fast stage lookahead (oversampled) |
| 0x401220, 0x6012e0, 0x8013a0 (+0x100060 ch) | `DelayInt` | M/S lookahead, last lookahead, dry path |
| 0xa01460, 0xa01500 (+0x50 ch) | `Lowpass1` | level matching, 0.125 Hz |
| 0xa015a0 + 0x28 ch | true peak | factor 4, FIRs for 2/4/8 (`FUN_180172c80`) |
| 0xa015f0.., 0xa017d0.. | halfband sets | up / down for 2x, 4x, 8x, 16x |
| 0xa019b0.., 0xa01b90.. | halfband sets | the dry path's, for delta |
| 0xa01d70 | 4 floats | `ln 10`, `-1/9`, 10, `1/ln 11` |
| 0xa01d80, 0xa01dd0, 0xa01e20 | `Lowpass1` 20 Hz | bypass crossfade, input gain, output gain |
| 0xa01e78 + 0x58 k | `{0.2, Lowpass1 5 Hz}` | low and high power of each side |
| 0xa02078, 0xa020c8 | `Lowpass1` 100 Hz | the rebuild fade |
| 0xa02118 / 0xa02120 | u8, u64 | fade on, fade counter |
| 0xa02128 / 0xa02129 | u8, u8 | ready (faded out), dirty |
| 0xa02130 | `Lowpass1` 1000 Hz | program attack target |
| 0xa02180 | `ExpSmoother1` 50 ms | program emphasis |
| 0xa021d8 + 0x50 ch | `Lowpass1` 320 Hz | low/high split |
| 0xa02278.. | floats | the parameters, then derived values |
| 0xa02360 + 0x50 ch | `Highpass1` 5 Hz | DC blocker |
| 0xa02400.. | | LUFS meters (K-weighting, `FUN_1801710f0`), output RMS: display only, left out |

Parameter block: `0xa02278` bypass, `7c` delta, `80` order, `84` true peak,
`88` channel mode, `8c` mode, `90` gain dB, `94` linking, `98` amount, `9c`
ceiling dB, `a0` level matching, `a4` oversampling, `a8` hardness, `ac` meters
off (13, not sent), `b0`/`0xa02320` factor, `b4` program mode, `b8` input gain,
`bc` its inverse, `c0` output gain, `cc` slow attack, `d0`..`d8` emphasis
(0x16), `dc` low latency, `e0` instant, `e4` DC; `0xa022ec` delta, `f4` true
peak, `f8` channel mode, `0xa0234c` low latency, `50` instant as last taken.

### Classes (vtables at `0x1803dda68`..`0x1803dde28`)

Every `DSP` subclass has `+8` rate, `+0xc` 1/rate, slot 0 setRate, slot 1
process(in, out, n), slot 2 prepare, slot 3 reset.

- `Lowpass1` (`0x180171260`/`0x180171320`): `c = exp(-2 pi f / rate)` from
  `+0x48` f, `y = (y - x) c + x`. `Highpass1` (`0x180170c30`): `x - lp`.
- `ExpSmoother1` (`0x18016ea20`): coefficients `+0x40` release, `+0x44`
  attack, `exp(-2 pi / (rate T))`; the attack one where the state is below
  the input.
- `ExpSmoother` (`0x18016e740`): `+0x68` stages in a row on the same two
  coefficients `expf(-step / T)`, `step = 2 pi k / rate`, `k = 1 /
  sqrt(2^(1/N) - 1)` (`FUN_18016e820`). `ExpSmoother8` (`0x18016ec50`): eight
  stages, `step = 20.885 / rate` (`0x1803e0748`, the same `k` for 8).
- `PeakHold8` (`0x180165a90`): eight holds in a row, each keeps the largest
  input for up to `H = T * 0.125 * rate` samples. `PeakHold1`: one, used by
  the stages only for their meter.
- `GRStatic8`/`GRStatic1` (`0x1801701c0`/`0x18016fa60`): the share of the
  block whose 25 ms held level is at least 1.0019 (`0x1803e0688`) into
  `+0x1d8`/`+0x1b8` (a meter, but the program modes read the fast stage's);
  then `PeakHold8`, `max(., 1)` (`0x1803e0a30`), the smoother, `min(1 / ., 1)`.
  Times: `+0x1c8`/`+0x1a8` attack, `+0x1cc`/`+0x1ac` hold, `+0x1d0`/`+0x1b0`
  release.
- `GRAdaptiveRel` (`0x18016f1c0`): `PeakHold8` over 50 ms, then each sample
  a release worked out from the program:

  ```
  lp    = Lowpass1 20 Hz of (level / input gain)
  over  = 20 log10(max(lp, eps) / max(last gain, eps))
  gr    = 20 log10(max(last gain, eps))
  base  = over > 6 - g dB && first stage's gain rising ? 0.15 s : 0.6 s^2
  grow  = (gr < -0.33 g || over < -0.5 g) ? sqrt|gr + 0.33 g| * 10/N + 1 : 1
  release = base * grow           (5 ms with instant recovery)
  ```

  where `s = 1 + 2.5 y^2` (`+0x24c`, from case 0x13) and `g` the input gain
  in dB. One stage (`N = 1`) runs `ExpSmoother1` at `+0x178` with the
  release in doubles; more run `ExpSmoother` at `+0xf0`. The attack is
  `+0x244`. The gain is `min(1 / max(state, eps), 1)`.
- `DelayLerp` (`0x18016d7c0`): a 2^18 ring; the length glides to `+0x100048`
  at 5 Hz (`-10 pi / rate`), snapping to the rounded target within 0.1 %;
  read `buf[w - D] + (buf[w - D + 1] - buf[w - D]) (d - D)`. It starts at 0.
- `DelayInt` (`0x18016d4c0`): `trunc(rate * T)` samples.
- Halfbands (`FUN_180170630`, `FUN_180170aa0`): two chains of second-order
  allpasses in `z^-2`, `y = (x - y2) c + x2`, out `(A(x) + B(x[n-1])) / 2`.
  12 coefficients for the first stage (`0x1803e0a10` A, `0x1803e0a20` B),
  6, then 4, 4. Up (`FUN_18016ab60`): times the factor, each stage a zero
  after every sample. Down (`FUN_18016ac90`): last stage first, keeping the
  first of each two.
- True peak (`FUN_180172570` over `FUN_1801723f0`, design `FUN_180172220`):
  `96 F + 1` taps, `h[i] = max(0, sin(pi i / (L-1))) sinc(i / F - 48)`, an
  overlap-add interpolator; the level is the largest of the `F = 4` outputs.

### `setParam` (`FUN_180169150`)

The switch's tail calls (jump table `0x18016a38c`) were read from the
listing. Cases marked *rebuild* act only when ready (`0xa02128`); otherwise
they set dirty and stop the fade (`LAB_18016a306`). Their tail
(`LAB_18016a29c`) sets the dry delay to the latency and lets the fade in.

| case | does |
|---|---|
| 0 | bypass |
| 1 *rebuild* | delta; case 9 again |
| 2 | order: `FUN_18016e820(N = x + 1)` on both slow stages, states copied from the first; cases 8, 0x1a |
| 3 *rebuild* | true peak |
| 4 *rebuild* | channel mode: the M/S stage and its delay at 8 ms for L/R, else `0x1803da9c8[max(2, hardness)]` |
| 5 | mode; program = mode <= 1; cases 8, 6 |
| 6 | gain `exp(0.11513 g)`, inverse; slow stages' `g` and `1/gain`; case 9 |
| 7 | linking |
| 8 | amount `a`: Versatile `0x16(0) 0x14(1) 0x13(1)`, Loud `(a, 0, 1)`, Transient `(0, a, a)`, Steady `(a, 0, a)` |
| 9 | ceiling: `min(exp(0.11513 dB), 0.99967)`; with delta, `1/gain` or 1 with level matching |
| 10 | level matching; case 9 |
| 11 *rebuild* | factor `2^x`; fast stage and its delay at `max(rate F, 44100)` |
| 12 | hardness `T = 0.032 * 2^-x`: fast stage attack `0x1803da9f0[x] T`, hold and release `T`; fast delay `floor(rate T) F`; slow delay `floor(0.05 rate) - floor(rate T)`; M/S stage `min(T, 8 ms)`; latency |
| 0x13 | `y = 1 - x` (squared outside Versatile/Loud); `s = 1 + 2.5 y^2` |
| 0x14 | slow attack `(10^(0.8 x + 0.2) - 1) / 9 * 10 * 0x1803da9a8[order] + 0.05` s |
| 0x16 | knee slope `1 - ln(1 + 10 x) / ln 11`, knee `(gain - 1) slope + 1` |
| 0x19 *rebuild* | low latency |
| 0x1a *rebuild* | instant recovery: `10/(order+1)` growth, case 8, the 5 ms flag |
| 0x1b | DC blocker |

`FUN_18016a630` (prepare): slow hold 50 ms; slow lookahead `round(0.05
rate)`; M/S stage and delay 8 ms; last stage and delay 4 ms; level match
0.125 Hz; power smoothers 5 Hz; split 320 Hz; fades 100 Hz; DC 5 Hz;
program target 1000 Hz; program smoother 50 ms.

Latency (`FUN_18016a560`): the slow lookahead (unless low latency without
instant recovery) + the fast lookahead / F + the M/S delay in M/S + the
last delay. At 44.1 kHz and the usual settings 2205 + 176 samples.

### Process (`FUN_180167330`)

1. DC blocker on the input when on.
2. Versatile and Loud (`0xa022b4`): each sample, each side split at 320
   Hz, low and high power smoothed at 5 Hz, `r = sqrt(low / high)`,
   `t = tanh(K (1 - tanh r))` (K 10 Versatile, 40 Loud; a 7/6 Pade tanh),
   `m = min(t_L, t_R)`. When both fast stages held over 1.0019 for more than
   90 % of the last block the attack target is `max(sqrt(m) attack, 0.5)`
   (0.05 with instant recovery) and the emphasis input `m`, else the attack
   and 1; smoothed at 1000 Hz and 50 ms. After the block: Versatile sets the
   slow attack to the smoothed target and the emphasis to `sqrt(sqrt(m)
   min((24 - g)/24, 1))`; Loud the emphasis to `sqrt(sqrt(smoothed))`.
3. The dry path: the input through the latency `DelayInt`, and with delta
   and oversampling through the dry halfbands too.
4. Input gain, smoothed at 20 Hz; in M/S on `L + R`, `L - R` at half gain.
5. Level: `|x|`, linked `(max - x) link + x`, pressed above the knee
   `(x - knee) slope + knee`.
6. Each side: slow stage on the level; the slow lookahead on the sound
   (skipped with low latency unless instant recovery); sound times gain.
   Then the fast stage on `|sound|` at the rate, or on `|up(sound)|` at the
   factor, its lookahead on the (oversampled) sound, times gain, down.
7. M/S back to L/R.
8. Each side: in M/S the M/S stage and its delay; the last stage on the
   true peak or `|sound|`, its delay, times gain.
9. Output gain (the ceiling), smoothed at 20 Hz.
10. Level matching: `out *= min(sqrt(lp(in^2)) / sqrt(lp(out^2)), 2)`.
11. Delta: `dry - out`.
12. Bypass crossfade to the dry path at 20 Hz; unless bypassed, the fade:
    a counter of samples since the last rebuild, open after 0.1 s, through
    two 100 Hz poles. When it reaches 0 and a rebuild waits, reset and set
    every parameter again.

## The port

- Everything above is rebuilt, in `f32` where the DLL keeps `f32`.
- The plugin starts as FL's does after running a while (FL's mixer runs
  plugins while the song is stopped): the fade open, the gain smoothers and
  the bypass at their values and the lookaheads at their lengths. A fresh
  instance in FL fades in over its first 100 ms.
- Blocks are cut to 512 samples. The program modes and the stages' meters
  work once a block, so Versatile and Loud depend on the host's block
  length, which in FL is not known here; the plugin says so in
  `fidelity()`.
- Level matching on two silent inputs divides 0 by 0; FL's output would be
  NaN, the port takes the ceiling of 2 instead. No corpus state has it on.
- Left out: the LUFS and RMS meters and the gain-reduction history, which
  only the GUI reads.
- Read from the listings where Ghidra lost the arguments:
  `FUN_180172030` is `dst += x h` (SSE `mulps`/`addps`), `FUN_005dfeb0` is
  `ln x * 0.4342944819` (`0x007327f8`), and the dry path's 2x halfbands in
  `process` (`0x180167b30`, `0x180167bc0`) run as `FUN_18016ab60` and
  `FUN_18016ac90` do.
