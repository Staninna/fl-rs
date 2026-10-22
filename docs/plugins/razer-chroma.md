# Razer Chroma: the sound passes untouched

Razer Chroma lights a Razer keyboard from the sound. Its `Eff_Render` reads
the block for peaks and a spectrum and hands it on as it came, so in fl-rs
it is a `PassThrough` (`crates/plugins/src/pass_through.rs`). The lights,
its ten parameters and its state are left out: none of them reaches the
sound.

Read from `Razer Chroma_x64.dll` (FL Studio 26.1.7, Delphi, image base
0x400000) with capstone listings; tag `asm`.

## The way in

`CreatePlugInstance` @ `0x7016c0` constructs the class whose reference is at
`0x7014e0`, VMT `0x7015a8`: Dispatcher `0x7017e0`, `Eff_Render` (slot 7)
`0x7022c0`. `TFruityPlugInfo` @ `0x756c18`: "Razer Chroma" / "Chroma", Flags
`0x400000` (no `FPF_NoProcess`, so FL calls `Eff_Render`), 10 parameters.

## `Eff_Render` @ `0x7022c0` `asm`

`Eff_Render(this, Source, Dest, Length)`:

1. `0x5eb950(Source, Length, &l, &r)`, the peak of each side (SSE
   `andps`/`maxps`; its only stores are to `l` and `r`).
2. `0x6fcf70(this+0x148, Source, Length, l, r)`, the analyser behind the
   lights. It reads `Source` twice: `0x5eba80(Source, 2 * Length, ...)`, a
   test for silence that only compares, and `[rsi + i*8]`, `[rsi + i*8 + 4]`
   for every fourth frame. Every store it makes is to its own object or its
   stack.
3. The two peaks are kept at `this+0x154` and `this+0x158` when higher.
4. When `Source` and `Dest` differ, `ippsCopy_64s(Source, Dest, Length)`
   (pointer `0x7580f0` -> `0x765e90`, bound by name at `0x5aa662`): one
   64-bit element a stereo frame, so `Dest` is `Source` bit for bit.

Nothing else touches either buffer, so the output is the input, exact.
