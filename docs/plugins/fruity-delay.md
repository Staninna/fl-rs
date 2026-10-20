# Fruity Delay: state, parameters, signal chain

Recovered from `Fruity Delay_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000). Ghidra project `/root/ghidra/fx/proj` (program
`Fruity_Delay_x64.dll`), decompiles under `/root/ghidra/fx/out/delay*/`
(`tools/ghidra`-style `DecompileDeep.java`: the SDK slots, the constructor,
the engine and its mode loops, with callees). Corpus: one use in the 165
`.flp` (The Ogun Dream), six shipped presets, no render test.

Confidence tags: `loader`, `dfm`, `corpus`, `preset`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006ca7f0` -> constructor `FUN_006cac40`, class
reference `0x006ca608` -> VMT `0x006ca6d0` (`TTestPlug`, size 0x1e8).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006ca840 |
| 2 | Idle_Public | 0x006c5e90 |
| 3 | SaveRestoreState | 0x006cab40 |
| 4 | GetName | 0x006cba00 |
| 5 | ProcessEvent | 0x006caa90 |
| 6 | ProcessParam | 0x006cb040 |
| 7 | Eff_Render | 0x006cbac0 |

Plugin object:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[6] | the parameters, saved |
| 0x138 | f32[6] | each parameter as the wheel's 0..1 (`FUN_006cb260`); 0x138 is the input gain |
| 0x150 | f32 | input gain the last block ended on |
| 0x154 | f32 | the song's tempo (ProcessEvent `FPE_Tempo`, `FUN_006caf40`), 140 at first |
| 0x158 | i32 | the wheel's tempo, 0 for the song's |
| 0x15c | i32 | a sixteenth note in frames |
| 0x160 | i32 | steps |
| 0x164 | i32 | mode: 0 normal, 1 inverted stereo, 2 ping pong |
| 0x168 | struct | low pass: +4 f32 coefficient (0x16c), +0x28/+0x30 f64 the two sides' states |
| 0x1d0 | ptr | the delay engine (`FUN_006c74e0`) |
| 0x1d8 | i32 | the host's block size (Dispatcher `FPD_SetBlockSize`) |

The engine (`FUN_006c74e0`, a ring of stereo f32 frames): +8 base, +0x10
read, +0x18 write, +0x20 end, +0x28 length in bytes, +0x2c frame size 8,
+0x30 feedback (0.5 at first), +0x34 feedback the last block ended on (0 at
first), +0x38 the `TAudioRenderer`, and method pointers for the modes: a
plain set (`0x006c7f10` normal, `0x006c8080` inverted, `0x006c81f0` ping
pong) and, when the CPU flag at `0x00728d30+0x16` is set, an SSE set
(`0x006c8fc0`, `0x006c8d70`, `0x006c9210`).

## 1. State layout (event 213, 28 bytes)

`SaveRestoreState` writes `i32 1` and then `this+0x120`, 24 bytes. On
restore it reads the version and leaves the plugin alone when it is above 1;
otherwise it reads the 24 bytes and runs the GetValue/UpdateValue loop.

| byte | type | idx | parameter (DFM object) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | | version | 1 | 1 | 1 | loader, corpus |
| 4..7 | i32 | 0 | Input (`InputWheel`) | 0..1024 | 338 | 608 | loader, dfm, corpus |
| 8..11 | i32 | 1 | Feedback (`FeedbackWheel`) | 0..1024 | 512 | 223 | loader, dfm, corpus |
| 12..15 | i32 | 2 | Cutoff (`CutoffWheel`) | 0..1024 | 1024 | 1024 | loader, dfm, corpus |
| 16..19 | i32 | 3 | Tempo (`TempoWheel`) | 0..1024 | 0 | 279 | loader, dfm, corpus |
| 20..23 | i32 | 4 | Steps (`StepsWheel`) | 1..16 | 3 | 4 | loader, dfm, corpus |
| 24..27 | i32 | 5 | Mode (`ModeWheel`) | 0..1024 | 0 | 809 | loader, dfm, corpus |

The presets: Default `[338, 512, 1024, 0, 3, 0]`, Ambient, Double attack,
Half step at 140, Lowpass, Single echo; all version 1.

## 2. Parameters (`ProcessParam` @ 0x006cb040 -> `FUN_006cb260`)

`n = clamp((v - Min) / (Max - Min), 0, 1)` as f32, stored at
`0x138 + 4 i`, then:

```
0 input     gain = n                                     (ramped, 0x150)
1 feedback  engine+0x30 = f32(n * 0.99)
2 cutoff    a = f32(f32(exp(n * ln 21) - 1) * 0.05)      // 0 .. 1, 1 = open
3 tempo     n == 0: 0x158 = 0, sixteenth = Round(f32(rate * 15) / song tempo)
            else    0x158 = Round(n * 220 + 60), sixteenth = Round(f32(rate * 15) / 0x158)
4 steps     Round(n * 15 + 1)
5 mode      Round(n * 3 - 0.5)                           // 0.5 at 0x007081d0
```

`Round` is Delphi's, ties to even. Constants: 0.99 @ `0x006cb420`, 21.0 @
`0x006cb428`, 1.0 @ `0x006cb430`, 0.05 @ `0x006cb438`, 220/60 @
`0x006cb440`/`0x006cb444`, 15/1 @ `0x006cb448`/`0x006cb44c`, 3 @
`0x006cb450`. The mode names are at `0x0071a648`: "Normal", "Inv. stereo",
"Ping pong".

A tempo, steps or block size change calls `FUN_006c7380`: the line is
`(block + sixteenth * steps)` frames, and when that length changes it is
allocated again and cleared (`FUN_006c7320`), with the write pointer
`sixteenth * steps` frames past the read pointer.

## 3. Signal chain (`Eff_Render` @ 0x006cbac0)

```
ramp(input: 0x150 -> 0x138, cap = renderer+0x34, len)        // FUN_005dfd10
add Source * input gain into the ring at the write pointer   // FUN_006c7800 / 78b0, wrapping
mode loop (ring, Dest, Source, len, &filter):
    ramp(feedback: engine+0x34 -> +0x30, cap, len)
    per frame:
        e = ring[read]; ring[read] = 0
        normal:   Dest = Source + (eL, eR);  back = (eL, eR)
        inverted: Dest = Source + (eR, eL);  back = (eL, eR)
        ping pong:Dest = Source + (eR, eL);  back = (eR, eL)
        for each side: y += (back * fb + ring[write]) - y) * a;  ring[write] = |y| < 2^-24 ? 0 : y
        fb += fb step; advance both pointers, wrapping
```

`ring[write]` holds the input added before the loop, so each frame writes
`lowpass(echo * feedback + input * gain)`. The dry input passes at full
level whatever the input wheel says.

The SSE loops keep the filter state in f64 (normal, ping pong); the
inverted SSE entry calls the plain loop, which keeps it in f32 and flushes
at `<= 2^-24` (`0x0070bf2c`). The cap is `TAudioRenderer+0x34`,
`0.001 * 44100 / rate`.

Dispatcher `FPD_Flush` (2) clears the ring, snaps the input gain to its
wheel and zeroes the filter; the feedback ramp is not touched.

## Port notes

- The ring is kept `sixteenth * steps` frames long, which gives the same
  samples as FL's `block + delay` ring with the input added a block ahead.
  It is reserved for 16 sixteenths at 10 BPM so that tempo changes do not
  allocate.
- `inferred`: the input-add routines behind `0x0072a490`/`0x0072a498` were
  not read; the port adds `input * (start + n * step)`, as Balance's matrix
  routine does. The CPU flag is taken to be set (any x64 CPU has SSE2).
