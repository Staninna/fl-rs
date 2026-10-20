# Wave Candy: the sound passes untouched

Wave Candy is FL's scope, spectrum and meter. Its `Eff_Render` reads each
block for the display and hands the block on as it came, so in fl-rs it is
a `PassThrough` (`crates/plugins/src/pass_through.rs`). The display, its one
parameter and its state are left out: none of them reaches the sound.

Read from `Wave Candy_x64.dll` (FL Studio 26.1.7, Delphi, image base
0x400000) with capstone listings; tag `asm`.

## The way in

`CreatePlugInstance` @ `0x737540` constructs the class whose reference is at
`0x737360`, VMT `0x737428`: Dispatcher `0x7376e0`, `Eff_Render` (slot 7)
`0x7399c0`. `TFruityPlugInfo` @ `0x796e58`: "Wave Candy", Flags `0x400000`
(`FPF_Reserved1` only, so FL calls `Eff_Render`), 1 parameter.

## `Eff_Render` @ `0x7399c0` `asm`

`Eff_Render(this, Source, Dest, Length)`:

1. Unless the display is paused (`this+0x1bb`), `0x72ba60(this+0x1c0,
   Source, Length)` feeds the analyser. It moves `Length` frames from
   `Source` into the analyser's own ring with `ippsMove_8u` (pointer
   `0x798720` -> `0x7a5e98`, bound by name at `0x5aa7c1`), and when the ring
   is full moves its tail to its head; every write is to the ring. `Source`
   is only read.
2. When `Source` and `Dest` differ, `ippsCopy_64s(Source, Dest, Length)`
   (pointer `0x798d00` -> `0x7a5e90`, bound at `0x5aa7a2`): one 64-bit
   element a stereo frame, so `Dest` is `Source` bit for bit.

Nothing else touches either buffer, so the output is the input, exact.
