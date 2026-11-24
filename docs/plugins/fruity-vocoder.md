# Fruity Vocoder: state, bands, signal chain

Recovered from `Fruity Vocoder_x64.dll` (Delphi, image base 0x400000), FL
Studio 2026. Ghidra project `/root/ghidra-projects/distructor` (`voc`).
Corpus: Edlan & Ella Noël - Song For You (129 bytes) and the render test
`System/Render tests/Fruity Vocoder.flp` (193 bytes); 6 presets.

Confidence tags: `asm`, `dec`, `corpus`.

## 1. How the plugin is wired

`CreatePlugInstance` @ `0x006cf350`, `TTestPlug` VMT `0x006cf210`
(instance 0x15190): Dispatcher `0x006cf460`, SaveRestoreState `0x006cf810`,
GetName `0x006d0cf0`, ProcessParam `0x006d08c0`, Eff_Render `0x006d0fb0`.
Everything is inside the plugin object: 128 modulator bands at `+0x150`
and 128 carrier bands at `+0xa950`, 0x150 bytes each.

There is no sidechain: the modulator is the left side of the input and the
carrier the right (`FUN_005e0a60`), swapped by the swap switch.

## 2. State (`0x006cf810`, `dec`)

| bytes | field | meaning |
|---|---|---|
| 0 | i32 | version 2; under 2 nothing more is read |
| 4 | `+0x15170` | bands: 4, 8, ... 128 (`4 << selector`) |
| 8 | `+0x15174` | filter order: stages a band runs |
| 12 | `+0x15178` | listen: 0 modulator, 1 carrier, 2 the mix |
| 16 | `+0x1517c` (u8) | swap modulator and carrier |
| 17 | f32[bands] | each band's level, `+0x294 + 0x150 i` |
| | i32[12] `+0x120` | the wheels (DFM `SynthEditorForm`, `Tag` = index) |

| wheel | DFM | range | default |
|---|---|---|---|
| 0, 1 | FreqMinWheel, FreqMaxWheel | 0..65536 | 0, 65536 |
| 2 | FreqScaleWheel | 0..128 | 64 |
| 3 | InvBtn | 0, 1 | 0 |
| 4 | ModFreqOfsWheel (formant, cents) | -1200..1200 | 0 |
| 5 | BWWheel | 5..200 | 50 |
| 6, 7 | AttWheel, RelWheel (ms) | 10..1000 | 20, 100 |
| 8 | HoldBtn | 0, 1 | 0 |
| 9, 10, 11 | ModVol, CarVol, WetVol | 0..160 | 0, 0, 128 |

Edlan's: 16 bands, order 2, the mix, wet 147. The render test: 32 bands,
order 2, bandwidth 27, wet 128.

## 3. Bands (`FUN_006d03a0`, `FUN_006cff90`, `FUN_006d00b0`, `dec`)

```
edge(v)  = (101^(v/65536) - 1) · 159.9 + 10 Hz          start and end, 10 Hz .. 16 kHz
scale    = round(10000^(p2/128) - 1) + 1                  1 .. 10000, 100 at the default
at(i)    = start + ((scale + 1)^(i/n) - 1) / scale · (end - start),   i = 0..n
note(i)  = cents(at(i)) (5700 = 440 Hz); the carrier's + formant, its band
           index mirrored with invert
band i   = centre hertz((note(i-1) + note(i)) / 2),  width span · (note(i) - note(i-1)) cents
span     = BW · order / 120000                            (octaves a cent)
on       = 10 Hz <= centre < 20000 · SR/44100, and for the modulator its carrier band on
gain     = band level · wet · 1500 / BW
mix      = listen 0: modulator 1; 1: carrier 1; 2: knob(ModVol), knob(CarVol) and wet knob(WetVol)
knob(v)  = (11^(v/128) - 1) / 10, 1 at 128
attack, release = exp(ln 0.01 / (ms · SR/1000))
```

A band's filter (`FUN_005dbb00`, `FUN_005db530` type 2): RBJ band pass of
0 dB peak, `alpha = sin w · sinh(ln2/2 · octaves · w / sin w)`, run in
doubles, direct form I (`FUN_005dbc10`), `order` times with a state each.

## 4. Render (`0x006d0fb0`, `dec`)

```
acc = 0
for each band that is on:
    m = bandpass^order(modulator)
    env: x = |m| · gain;  c = x <= env ? release : attack;  env = (env - x) c + x   (x87, FUN_006d0e70)
    c = bandpass^order(carrier)           (FUN_006d0ef0)
    acc += c · env                        (FUN_005df640; with hold the last env held)
acc += modulator · mod + carrier · car
out = acc on both sides                   (FUN_005e0aa0)
```

## 5. Implementation notes (crates/plugins/src/fruity_vocoder.rs)

- Built: all of the above.
- Approximated: the x87 follower runs in f64; FL clears a tiny envelope
  only when the next input is exactly 0 (its test reads the next sample as
  the high half of a 64-bit word), and so does this; FL's flush of tiny
  filter states after a block is left out; Delphi's `Exp`, `Ln`, `Power`
  and `sinh` are Rust's.
- The meters (`+0x13c`, `+0x1517f`) are left out: they make no sound.
