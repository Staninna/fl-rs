# Fruity Send: state, parameters, signal chain

Recovered from `Fruity Send_x64.dll` (FL Studio 2026, Delphi native plugin,
image base 0x400000), read with capstone listings (`tools/plugstate/pe.py`
for the constants, `dfm.py` for the wheels). Corpus: one use in the 165
`.flp` (I'll Be Your Madonna) plus the render test `Fruity Send.flp`; no
preset folder.

Confidence tags: `loader`, `dfm`, `corpus`, `asm`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x006bcb10` -> constructor `FUN_006bd1e0`, class
reference `0x006bc928` -> VMT `0x006bc9f0` (`TTestPlug`, instance size
0x168).

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x006bcb60 |
| 2 | Idle_Public | 0x006b9a60 |
| 3 | SaveRestoreState | 0x006bd0f0 |
| 4 | GetName | 0x006bda40 |
| 5 | ProcessEvent | 0x006b8e30 |
| 6 | ProcessParam | 0x006bd630 |
| 7 | Eff_Render | 0x006bdb80 |

Object fields:

| offset | type | meaning |
|---|---|---|
| 0x120 | i32[3] | the parameters: pan, volume, dry |
| 0x12c | i32 | target mixer track (an output number, see below), saved |
| 0x130..0x13c | f32[4] | the send matrix the last block ended on |
| 0x140..0x14c | f32[4] | the send matrix the wheels ask for: LL, RR, RL, LR |
| 0x150 | f32 | pan, `v / 128` |
| 0x154 | f32 | send volume, knob curve |
| 0x158 | f32 | dry level, knob curve |
| 0x15c | u8 | matrix to be worked out again before the next block |

## 1. State layout (event 213, 16 bytes)

`SaveRestoreState` writes and reads `this+0x120`, 16 bytes, verbatim; on
restore it runs the GetValue/UpdateValue loop for the three wheels and sets
the target wheel from `0x12c` (`asm`).

| byte | type | idx | parameter (DFM object, hint) | range | default | corpus | confidence |
|---|---|---|---|---|---|---|---|
| 0..3 | i32 | 0 | Send panning (`PanWheel`) | -128..128 | 0 | -54, 0 | loader, dfm, corpus |
| 4..7 | i32 | 1 | Send volume (`VolWheel`, Tag 1) | 0..320 | 256 | 256 x2 | loader, dfm, corpus |
| 8..11 | i32 | 2 | Dry level (`DryVolWheel`, Tag 2) | 0..256 | 256 | 256, 197 | loader, dfm, corpus |
| 12..15 | i32 | | Target mixer track (`SideChainSelect`, "Target mixer track") | 0..999 | 0 | 0, -1 | loader, dfm, corpus |

The `DryVolWheel` DFM has `DefaultValue = 320` but `Value = 256`; the
constructor copies each wheel's value (`+0x3b0`) into the parameters, so a
new instance has 256. The -1 target in I'll Be Your Madonna is outside the
wheel; the restore takes it as it is.

## 2. Parameters (`ProcessParam` @ 0x006bd630)

The value is stored at `0x120 + 4 i`, then:

```
0 pan     0x150 = v * (1/128)                                   (f32)
1 volume  0x154 = knob(v)
2 dry     0x158 = knob(v)
knob(v): x = v * (1/256) in f32; x == 1 -> 1
         else f32((exp(f64(x * f32(ln 11))) - 1.0) * f64(0.1f32))
```

`ln 11` is worked out at startup into `0x0071c46c` (`FUN_005e1f60`,
`ln(11.0)` stored as f32); `0.1f32` is at `0x006fa230`, `1/256` at
`0x006fa240`, `1/128` at `0x006fa23c` (`asm`). Pan and volume set the
"rework the matrix" flag at `0x15c`.

The matrix (`FUN_006bd4e0`), with `p` the pan and `V` the volume:

```
p >= 0: a = 1 - p; LR = (1 - a) * 0.5 * V; LL = a * V; RR = V - LR; RL = 0
p <  0: b = p + 1; RL = (1 - b) * 0.5 * V; RR = b * V; LL = V - RL; LR = 0
```

So the send pans by fading one side and moving half of what it loses into
the other, as FL's insert panning does.

## 3. Signal chain (`Eff_Render` @ 0x006bdb80)

```
if target (0x12c) > 0:
    if 0x15c: rework the matrix
    for each of the four gains: FUN_005e0180(&delta, &now, &start, aim, maxstep, len)
        // delta = (aim - now) / len, clamped to maxstep; now = where it ends
        // maxstep = TAudioRenderer+0x34 = 0.001 * 44100 / rate (equo.md)
    host.GetOutBuffer(Sender, target, &buf) (host vtable +0x128, flag 0)
    if buf: add Source * the ramped matrix into buf (function pointer 0x0071c4f8)
    host.GetOutBuffer(Sender, target, &buf) (flag 1)       // hand it back
if dry == 1: copy Source to Dest
else: Dest = Source * dry                                   (function pointer 0x0071c4d0)
```

The host call is the one Stereo Shaper makes for its difference output
(`fruity-stereo-shaper.md`): the target counts the outputs the host gives
the plugin, which for an effect in an insert are the inserts it sends to,
1 the first (the wheel's range is set from `FHD_GetNumInOut`, host
dispatcher 50 with index 1, in `FUN_006bd450`). The engine's
`aux_output`/`sidechain_output` path carries it.

Dispatcher ID 2 (flush) copies the aimed matrix over the current one.

The ramp is `FUN_005e0180`, the same as Balance's and Convolver's: the
port uses `parts::glide` with the renderer's cap.

## Not read

- The mixing routine behind `0x0071c4f8` is filled at startup and was not
  read; the port applies `now + n delta` from the first frame, as Balance's
  matrix routine does (`inferred`).
