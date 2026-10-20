# A mixer insert's processing: EQ, gain matrix, polarity, swap

Read from `FLEngine_x64.dll` of FL Studio 2026 (26.1.7; Ghidra base
0x400000, PE image base the same). `docs/rec-225.md` has how the controls
reach the insert; this page has what the insert does with them. The engine
side is `crates/engine/src/eq.rs` (the EQ), `Curves::insert_spread` in
`crates/engine/src/curves.rs` (pan and separation) and `mix_strip` in
`crates/engine/src/mix.rs` (the order, polarity and swap).

Confidence: `read` means read from the decompiled C or the disassembly,
`inferred` a reading no second source confirms.

## The insert struct

`*PTR_DAT_015e06a0` is the array of 502 inserts, 0x1478 bytes each. The
fields this page uses:

| offset | what | set by |
|---|---|---|
| `+0x18` | switched on (byte) | `FUN_012c1810`, bit 3 of the flags |
| `+0x34` | the insert's bus: interleaved stereo f32 | |
| `+0x80..+0x8c` | the gain matrix, four f32: `+0x80` left to left, `+0x84` right to right, `+0x88` right to left, `+0x8c` left to right | `FUN_0127b050` |
| `+0x90..+0x9c` | the matrix as the last block ended, for the glide | `FUN_006a38c0` |
| `+0x1d4 + b * 0x88` | EQ band `b`, 0..2 (below) | `FUN_0127b390` |
| `+0x384`, `+0x388`, `+0x38c` | raw volume, pan, separation (225 controls 0x1FC0..0x1FC2) | REC dispatcher |
| `+0x390` | polarity reversed (byte) | bit 0 of the flags |
| `+0x391` | left and right swapped (byte) | bit 1 of the flags |

### The flags

The project loader `FUN_00f46490` reads the flags word of 236
`InsertID.Flags` (offset 4 of its 12 bytes) and, in its branch for event
27, the same bits from that event's data:

    insert[0x390] = (flags & 1) != 0;        // polarity
    insert[0x391] = (flags & 2) != 0;        // swap left and right
    insert[0x13f5] = (flags & 4) != 0;
    FUN_012c1810(insert, (flags & 8) != 0);  // switched on
    insert[0x380] = (flags & 0x10) == 0;
    insert[0x2d] = (flags & 0xc0) >> 6;      // 236 only
    insert[0x2c] = (flags & 0x400) != 0;     // and on: 0x800 -> +0x1b, 0x1000 -> +0x1a,
                                             // 0x2000 -> +0x2e, 0x4000 -> !+0x1d1, 0x8000 -> !+0x1d0

The insert reset `FUN_0127b780` clears both bytes. Read. Corpus: 12
inserts have bit 0 set (the genre templates' insert 5, `Astes - Bien
Duro.flp` inserts 1 to 3, one in `Edlan & Ella Noël - Song For You.flp`),
and one has bit 1 (`Astes - Bien Duro.flp` insert 57).

## The order an insert runs in

`FUN_01276ee0` processes one insert for one block:

1. The plugins in its slots, one after the other, on `+0x34`.
2. `FUN_01277550`: the insert's recording tap when `+0x2f` is 2.
3. **The EQ**, `FUN_01276d00`.
4. `FUN_012775a0`: the recording tap when `+0x2f` is 3.
5. **The gain matrix**, `FUN_01276e40`: volume, pan, separation, polarity
   and swap in one 2x2 matrix. The master skips it when either of two
   global options (`*PTR_DAT_015e3500`, `*PTR_DAT_015e2e98`) is set.
6. `FUN_012775f0`: the recording tap when `+0x2f` is 4.
7. `FUN_01276ac0`: the bus is copied out, for the sends.
8. When the insert is switched off, the bus is cleared
   (`*PTR_DAT_015e03a0`).

So the EQ comes after the slots and before the fader, and the sends take
the bus after both. Read.

## The gain matrix

`FUN_0127b050(insert)` runs after any write to volume, pan or separation.
With `v` the raw volume, `p` the raw pan, `s` the raw separation:

    if v == 12800 and p == 0 and s == 0:
        m80 = m84 = polarity ? -1.0 : 1.0;  m88 = m8c = 0
    else if v == 0:
        all four 0
    else:
        g = v / 12800 (f32, *PTR_DAT_015e4780 = 1/12800)
        gain = g == 1 ? 1 : (float)((exp(g * K) - 1) * 0.1)      // K below
        if polarity: gain = -gain                                  // sign bit flipped
        FUN_006a3a00(&m80, &m84, p * (1/6400), gain * (1/sin(pi/4)))   // ComputeLRVol
        fl = 0; fr = 0                                             // f32 0.0 (_UNK_0127b360)
        if p < 0: fl = 1 - m84 / gain
        if p > 0: fr = 1 - m80 / gain
        if s != 0:
            t = s < 0 ? s * (1/64)
                      : (float)(exp((s * (1/64)) * ln 5) - 1) * 0.25
            fl = min(fl + t, 1);  fr = min(fr + t, 1)
        m88 = m80 * fl * 0.5;  m80 = m80 * (1 - fl * 0.5)
        m8c = m84 * fr * 0.5;  m84 = m84 * (1 - fr * 0.5)
    if swap:
        (m80, m84, m88, m8c) = (old m8c, old m88, old m84, old m80)

Constants: 1/64 at `*PTR_DAT_015e3ed8`, 0.25 at `*PTR_DAT_015e0d98`, 0.5 at
`*PTR_DAT_015e4000`, 1/6400 at `*PTR_DAT_015e4d70`, 1.0 at `0x127b380`,
5.0 at `0x127b388` (its log through `FUN_0040c1d0`, which is `ln`).
`FUN_0040ae70` is `exp`. Read.

`FUN_01276e40` hands the matrix to `FUN_0067fd90`, which glides each of the
four from where it was to where it is over the block (`FUN_006a38c0`, with
a largest step per sample), and then to the routine at `*0x15f4a70`,
`0x6a13a0`, which runs it over the interleaved bus:

    out_left  = left * m80 + right * m88
    out_right = right * m84 + left * m8c

(`shufps xmm0, xmm0, 0x14` makes `[L, R, R, L]`, times `[m80, m84, m88,
m8c]`, and the high half is added to the low.) So `+0x88` carries the
right input into the left output: the far input of a pan to the left.
When the glide has nothing to do and `m88`, `m8c` are 0, it scales each
side alone, and with `m80 = m84 = 1` in place it does nothing. Read.

What the two terms mean:

- Pan: the near side keeps `1 - (1 - far)/2` of its gain and feeds
  `(1 - far)/2` across, where `far` is the far side's gain over the
  fader's (1 in the centre, 0 at a hard pan). A hard pan left puts both
  inputs in the left output at -3 dB.
- Separation adds `t` to that share on both sides, and stops it at 1.
  Above 0 it merges: at 64, `t = 1` and each output is the mean of the two
  inputs. Below 0 it separates: at -64, `t = -1` and each output is 1.5 of
  its own side less 0.5 of the other. The share is linear below 0 and
  `(5^(s/64) - 1) / 4` above. FL's hint (`FUN_006a4ac0`) prints
  "%d%% separated" for negative values and "%d%% merged" for positive
  ones (strings at `0x6a4c04` and `0x6a4c30`), so the "inferred: negative
  = merged" of `docs/rec-225.md` had it the wrong way round.
- Polarity turns the gain negative, which carries into all four terms.
- The swap trades the outputs: the left output gets what the right one
  would have.

### K

`*PTR_DAT_015e0350` points at the f32 at `0x15f49ec` in `.bss`. The
initialiser at `0x6a68e0` fills a row of such logs:

    0x6a690b: ln 601.0  -> f32 at 0x15f49e8
    0x6a6925: ln 11.0   -> f32 at 0x15f49ec      (K)
    0x6a6957: ln 3.0    -> f32 at 0x15f49f4, and 1 / it at 0x15f49f8
    ...

(`movsd xmm0, [0x6a6f30]` = 11.0; `call 0x40c1d0`; `cvtsd2ss`; `movss
[0x15f49ec], xmm0`.) So K = (float)ln 11, as the engine's `FlExact`
already took it. Read.

## The EQ

Three bands, each 0x88 bytes from insert `+0x1d4`:

| band offset | what |
|---|---|
| `+0x0` | raw gain, -1800..1800 (control 0x1FD0 + b) |
| `+0x4` | raw frequency, 0..65536 (0x1FD8 + b) |
| `+0x8` | raw width, 0..65536 (0x1FE0 + b) |
| `+0xc` | run counter: -1 runs for good, n > 0 runs n more frames, 0 is still |
| `+0x10` | the filter: type (i32) |
| `+0x14` | gain in dB (f32) |
| `+0x18` | frequency in Hz (f32), the value the hint prints |
| `+0x1c` | width (f32) |
| `+0x20..+0x5f` | state, eight f64: x1 L/R, x2 L/R, y1 L/R, y2 L/R |
| `+0x60..+0x87` | coefficients, five f64: b0, b1, b2, a1, a2 |

The insert constructor `FUN_01286f60` gives band 0 type 6, band 1 type 7,
band 2 type 8: low shelf, peak, high shelf. FL's hints agree: "EQ band 1
(low shelf)", "EQ band 2 (peaking)", "EQ band 3 (high shelf)" in the mixer
form. Read.

### From raw values to the filter

The dispatcher splits a control 0x1FD0..0x1FE7 as `band = n & 7`, `field
= n >> 3` (n = control - 0x1FD0), stores the raw value and calls
`FUN_0127b390(insert, band)`:

    hz    = (float)((double)(float)(exp((double)(float)(raw_f / 65536.0) * ln 101) - 1) * 159.9 + 10)
    width = (float)(raw_w / 65536.0)
    gain  = (float)raw_g * 0.01f
    counter = raw_g == 0 ? *0x193fab0 : -1
    FUN_0069d0e0(&band[+0x10], *(rate object))

So the frequency runs from 10 Hz at 0 to 16 kHz at 65536, through 90 Hz,
1.5 kHz and 8 kHz at FL's defaults 5777, 33145 and 55825. Constants at
`0x127b488` (1/65536), `0x127b490` (101.0), `0x127b4a0` (159.9),
`0x127b4a8` (10.0). The gain's range is that of the mixer form's sliders,
`Min = -1800, Max = 1800` on `MixEQLevel1Slider`..`3Slider`, whose `Tag`
is the control number; frequency and width are wheels with `Max = 65536`.
No clamp on the gain was found in the set path. Read.

`*0x193fab0` is set to `sample_rate / 5` at `0x12c6b98`: a band whose gain
goes to 0, or whose frequency or width moves while its gain is 0, runs on
for 200 ms. The reset at the start of play (`FUN_0127b530`) puts every
counter above 0 back to 0 and clears the state, so a band that starts at
0 dB does not run. Read.

`FUN_0069d0e0(filter, rate)`, with the rate object's `+0x28` =
`(1.0 / sr) * 2 pi` (f64) and `+0x30` = `(float)(sr * 0.4999)`
(`FUN_006a3ae0`, the object's SetSampleRate):

    if hz > rate.top: hz = rate.top                       // written back
    (sin, cos) = FUN_006649b0((double)hz * rate.w)
    A = FUN_004769f0(10.0, (double)gain * 0.025)          // 10^(gain/40)
    peak (7):    q = (float)(exp(width * ln(1000f + 1f)) - 1)   // FUN_006a2e70
                 alpha = sin / ((q * 0.0999 + 0.1) * 2)
    shelf (6, 8): beta = sqrt((A*A + 1) / (width * 1.9 + 0.1) - (A - 1)*(A - 1))
    FUN_0069cb10(filter, A, alpha or beta, sin, cos)

`FUN_0069cb10` is the RBJ cookbook in f64, normalised by a0, with
`beta * sin` as the shelves' `2 sqrt(A) alpha`:

    peak:  n = 1 / (alpha/A + 1)
           b0 = n (alpha A + 1)   b1 = a1 = n cos (-2)   b2 = n (1 - alpha A)   a2 = n (1 - alpha/A)
    low shelf:  t = (A-1) cos, l = beta sin, n = 1 / ((A+1) + t + l)
           b0 = n A ((A+1) - t + l)   b1 = n A 2 ((A-1) - (A+1) cos)   b2 = n A ((A+1) - t - l)
           a1 = n ((A-1) + (A+1) cos) (-2)   a2 = n ((A+1) + t - l)
    high shelf: n = 1 / ((A+1) - t + l)
           b0 = n A ((A+1) + t + l)   b1 = n A (-2) ((A-1) + (A+1) cos)   b2 = n A ((A+1) + t - l)
           a1 = n ((A-1) - (A+1) cos) 2   a2 = n ((A+1) - t - l)

So a shelf's slope is `S = 0.1 + 1.9 * width` and a peak's Q is
`0.1 + 0.0999 * (1001^width - 1)`: 0.1 to 100, 0.63 at FL's default width
17500. Constants at `0x69d268..0x69d2a8` (18.9, 0.1, 2.0, 10.0, 0.025,
1000.0, 0.0999, 1.9, 1.0) and `0x69d0b0..0x69d0d0` (1.0, -2.0, -0.0,
2.0). Types 1 to 5 are other filters this EQ does not use. Read.

### Running a band

`FUN_01276d00` runs the bands in order, in place on `+0x34`:

    for b in 0..3:
        if counter != 0:
            (*0x15f4920)(&band.filter, &band.state, bus, bus, frames)
            if counter > 0:
                counter -= frames
                if counter <= 0: counter = 0; clear the 0x40 bytes of state

`*0x15f4920` is set by the initialiser at `0x69fd40` to `0x69d500` when
bit 0x100 of the CPU feature word at `*PTR_DAT_015e4c78 + 0x12` is set
(taken to be SSE2, which every x64 processor has; inferred), else to
`0x69d420`. `0x69d500` is a direct form I biquad over interleaved stereo
f32, both sides at once in f64:

    x = (double)in
    acc = x1 * b1 - y2 * a2 + x2 * b2 - y1 * a1     // in this order
    y = x * b0 + acc
    out = (float)y
    x2 = x1; x1 = x; y2 = y1; y1 = y                // y1 keeps the f64

and then calls `*0x15f4b48` (`0x6a6160`) on the eight state values, which
zeroes each one under `2^-24` (the f32 at `0x13ec8cc`). Read.

## What the engine does differently

- The gain matrix moves at once to a new setting at the start of a
  block; FL glides it over the block. FL also builds the matrix with the
  volume inside `ComputeLRVol`; the engine multiplies the volume on after,
  which can differ in the last bit.
- `FUN_004769f0` (power), `FUN_006649b0` (sine and cosine), `exp`, `ln`
  and `sqrt` are FL's own; the engine uses Rust's, which may differ in
  the last bit of a coefficient.
- The master skips the matrix under two global options FL keeps outside
  the project; the engine always runs it.
- The recording taps (`+0x2f`) are not played.

## Functions read

| address | role |
|---|---|
| `FUN_01276ee0` | process one insert for a block: slots, EQ, matrix, copy out |
| `FUN_01276d00` | run the three EQ bands, count their tails down |
| `FUN_01276e40` | glide and run the gain matrix |
| `FUN_0127b050` | work the gain matrix out from volume, pan, separation, polarity, swap |
| `FUN_0127b390` | an EQ band's raw values to gain, Hz, width, counter; designs it |
| `FUN_0069d0e0`, `FUN_0069cb10` | biquad design (RBJ, types 1 to 8) |
| `0x69d500` | stereo interleaved f32 biquad, f64 inside (SSE2); `0x69d320` the mono one, `0x69d600` the f64 one |
| `0x6a13a0` | the 2x2 matrix over an interleaved bus |
| `0x6a6160` | zero state values under 2^-24 |
| `FUN_0067fd90`, `FUN_006a38c0` | glide the four matrix gains over a block |
| `FUN_006a3ae0` | the rate object's SetSampleRate: 2 pi / sr, 0.4999 sr |
| `FUN_01286f60` | insert constructor: band types 6, 7, 8 |
| `FUN_0127b530` | reset at the start of play: counters, EQ state |
| `FUN_006a4ac0` | separation hint: "separated" below 0, "merged" above |
| `0x6a68e0` | initialiser of the log constants, K = ln 11 among them |
| `0x12c6b60` | sets the EQ tail to sample rate / 5 |
