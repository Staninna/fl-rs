# Fruity dB Meter: the sound passes untouched

Fruity dB Meter shows the level of each side. Its `Eff_Render` keeps the
peaks and hands the block on as it came, so in fl-rs it is a `PassThrough`
(`crates/plugins/src/pass_through.rs`). The meter is left out: it does not
reach the sound.

Read from `Fruity dB meter_x64.dll` (FL Studio 26.1.7, Delphi, image base
0x400000) with capstone listings; tag `asm`.

## The way in

`CreatePlugInstance` @ `0x6b3090` constructs the class whose reference is at
`0x6b2eb0`, VMT `0x6b2f78`: Dispatcher `0x6b30f0`, `Eff_Render` (slot 7)
`0x6b3570`. `TFruityPlugInfo` @ `0x701dd0`: "Fruity dB meter" / "dB meter",
Flags `0x10400000` (no `FPF_NoProcess`, so FL calls `Eff_Render`), no
parameters.

## `Eff_Render` @ `0x6b3570` `asm`

`Eff_Render(this, Source, Dest, Length)`:

1. Unless the meter is held (`this+0x129`), `0x5df550(Source, Length, &l,
   &r)` finds the peak of each side with SSE `andps`/`maxps` over the
   block. It only reads `Source`; its two stores are to `l` and `r`. A peak
   above the one kept at `this+0x120` or `this+0x124` replaces it.
2. When `Source` and `Dest` differ, `ippsCopy_64s(Source, Dest, Length)`
   (pointer `0x703088` -> `0x710e90`, bound by name at `0x5a3cb2`): one
   64-bit element a stereo frame, so `Dest` is `Source` bit for bit.

Nothing else touches either buffer, so the output is the input, exact.
