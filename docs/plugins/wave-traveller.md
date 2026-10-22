# Wave Traveller: state, curves, voice

Recovered from `Wave Traveller_x64.dll` (FL Studio 2026, Delphi, image
base 0x400000). Headless Ghidra; decompiles of the VMT slots, the engine
object `CWWarper` and their callees. Corpus: 2 states in 1 project
(Secret Agent Rendezvous) and the render test, 9 presets; 332 to 37923
bytes.

Confidence tags: `loader`, `dfm`, `corpus`, `inferred`.

## How the plugin is wired

`CreatePlugInstance` @ `0x74a1f0` -> constructor `FUN_0074a240`; class
reference `*0x748000` -> VMT `0x7480c8` (`TTestPlug` on `TDelphiFruityPlug`,
0x168 bytes). The work is in a `CWWarper` (VMT `0x74d670`, 0x4e8 bytes,
made by `0x7516a0`) at `+0x154`.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x74a540 (1 quality: HQ sets `+0x480` to 2 and `+0x42c` to 64, 128 or 256 points; 4 sample rate -> `0x755840`) |
| 3 | SaveRestoreState | 0x74c030 |
| 5 | ProcessEvent | 0x74c540 (tempo -> `+0x448`) |
| 6 | ProcessParam | 0x74c5a0 (returns the value: no parameters) |
| 8 | Gen_Render | 0x74c410 -> `0x753fb0` |
| 9 | TriggerVoice | 0x74c1b0 -> `0x754fd0` |
| 10 | Voice_Release | 0x74c2d0 |
| 11 | Voice_Kill | 0x74c310 |

`CWWarper`: `+0x68` the 120 patches, `+0x428` the key the editor shows,
`+0x42c` the HQ points, `+0x430` the sample's rate from the host's sample
info, `+0x440` rate * 60 (float), `+0x444` rate, `+0x448` tempo, `+0x44c`
the DC blocker's coefficient `1 - 126 / rate`, `+0x450` the voices,
`+0x46c` the sample (stereo float frames), `+0x474` its frames, `+0x478`
its path, `+0x480` 1 realtime, 2 HQ.

A patch (`0x751530`, `0x7516a0` for the defaults): `+8` the position
table (1024 floats), `+0x1008` its count, `+0x100c` the volume table,
`+0x200c` its count, `+0x2010` start, `+0x2014` end (of the sample),
`+0x2018` beats (4), `+0x201c` Hz (1), `+0x2020` attack ms (1), `+0x2024`
release ms (1), `+0x2030` the speed is in Hz, `+0x2034` the name,
`+0x203c` the position curve's points, `+0x2044` the volume curve's,
`+0x204c` the mute points. A new patch: position (0, 0) to (1, 1),
volume (0, 1) to (1, 1), straight, and one mute point (on, 0).

## 1. State (`0x74c030`, `loader`, `corpus`)

Chunks of an i32 id and an i32 size, read until id 3000:

- 1000 (`0x74aee0`): i32 version (1000005), i32 the editor's key, i32
  length and the path in bytes. The size is right.
- 2000 (`0x74b340`): i32 key, f32 beats, Hz, attack, release, start, end,
  i32 Hz flag, i32 length and the name, i32 count and 8 bytes a mute
  point (a byte on, three of padding, f32 where), i32 count and 60 bytes
  a position point, i32 count and 60 bytes a volume point. FL writes the
  size as if a point were 8 bytes, so the size is short; the loader reads
  the chunk through. A patch that is the default is not written.
- anything else is skipped by its size.

A point (`0x737090` / `0x737200`), 60 bytes: f32 x at 8, y at 12, the
handle before at 20 and 24, after at 32 and 36, the tension at 40, i32
shape at 48, a flag at 56. The rest are heap words the writer leaves.

## 2. The curves (`0x7558c0`)

Each stretch from point `p` to `q` takes `n = Round((q.x - p.x) * 1023f)`
points of the table, the first on the last of the stretch before. Every
value is held to 0..1 as written. By `p.shape`:

| shape | values after the first (`y0`), `d = y1 - y0` |
|---|---|
| 1 | `+= d / (n - 1)` |
| 2 | `y0 + float(d * 0.5) * (cos(a + 3.1415926) + 1)`, `a += float(3.1415926 / (n - 1))` |
| 3 | the Bezier `p`, `p + 3 * after`, `q + 3 * before`, `q` in 256 points (`0x757620`), read at `x0 + (i + 1) (x1 - x0) / (n - 1)` in a straight line between them; stops past `q.x` |
| 4 | `y0 + d * sin(a)`, `a += float(1.5707963 / (n - 1))` |
| 5 | `y0 + d * (cos(a + 3.1415926) + 1)`, the same `a` |
| 6 | tension 0.5: `+= d / (n - 2)`; else `k = (1 - 2t) / t^2`, `a += 1 / (n - 1)` before each, `y0 + (d / k) (exp(a ln(k + 1)) - 1)` |
| 8 | 0 |
| others | nothing |

The count is the last index written.

## 3. A note (`0x754fd0`)

No note without a sample. The key is `Round(InitLevels.Pitch / 100)`,
0..119, and picks the patch (else a voice that is killed at once). The
voice copies the patch's tables and:

```text
speed = tempo * (count - 1) / (beats * rate * 60)                  (beats)
      = sample rate * Hz * (count - 1) / ((frames - 1) (end - start))   (Hz)
d     = FinalLevels.Pitch - (key - 60) * 100
speed = Final.Pitch < 0 ? speed / (1 - d / 1200) : speed * (d / 1200 + 1)
start, end = trunc(part * (frames - 1))
attack step = 1000 / (attack * rate), release step likewise
gains = ComputeLRVol(Final.Pan, Final.Vol * volume[0])
```

## 4. A sample of a note (`0x752740`, realtime)

1. The next mute point at or before `t / (count - 1)` sets on or off.
2. While `t` is past the volume counter, the counter moves one on and
   the volume is the volume table there.
3. The stage: attack adds its step and plays at 1; release takes its
   step and ends the note below 0; the end (below) plays the ring.
4. `place = (pos[i] + frac * (pos[i + 1] - pos[i])) * (end - start) +
   start`, `i = trunc(t)`.
5. Each gain goes 1% of the way to `ComputeLRVol(Final.Pan, Final.Vol) *
   volume`, or to 0 while off.
6. Each side is read at `place` by a six-point curve (constants at
   `0x753f68..0x753fa8`, zeros outside the sample), through
   `y = (x - x') + c y'` (`0x754eb0`), times its gain and the stage's
   level, into a ring of 256 and the output.
7. `t += speed`; past `count - 1` the note goes to its end.

At its end a note adds `(256 - k) / 256` of the ring's samples, newest
first, for 256 samples, and is over.

## 5. The sample

The host's `LoadSample` (vtable 0x68) is asked for 16-bit stereo at
44.1 kHz; the plugin keeps it as float frames `/ 32768` and one more frame
of the first frame's right side (`0x757e00`). The sample's rate goes to
`+0x430` from the host's `GetSampleInfo`.

## Not built

- The HQ reader (`0x754010`): a windowed sinc of 64 to 256 points.
- The host's conversion of a file at another rate than 44.1 kHz: a
  Hermite curve stands in, and fidelity says so.
