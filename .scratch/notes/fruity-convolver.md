# Fruity Convolver: handover for the layers (2026-11-24)

`crates/plugins/src/convolver.rs` plays every state. Built since the last
handover: the EQ curve (`FUN_0087b2b0`, `docs/plugins/fruity-convolver.md`
section 3.1): the editor's frequency axis, the gain per bin, the Gaussian
smoothing of the spectrum (`FUN_00624620`, IPP calls named by the
start-up loader's strings), the kernel (`FUN_00624910`: inverse FFT, a
half turn, Hann window) and the centred convolution and mix. The three
corpus slots with a drawn EQ (Bien Duro; Square a Saw Lightning and
Oxygen, `IMP Centered (long)` at 100 % wet) now play it. Addresses are in
`Fruity Convolver_x64.dll` (image base 0x400000); the Ghidra project was
`/root/ghidra-projects/smallgaps/convolver` on the machine this was read
on.

Not exact, and said in the doc and the row:

- The FFTs are `convolver.rs`'s radix-2 float FFT where FL's are IPP's;
  the result agrees to float rounding, not to the bit.
- The curve is read off its points by `dsp::curve::Curve`, which is how
  Love Philter's DLL draws a `TCustomMEnvelope`; that the Convolver's
  runner `FUN_00791500` (moved on by `x - previous x` from 0, value at
  context `+0x38`) gives the same was not checked. Read it next if a null
  test shows a difference.

## Left: the layers (`FUN_00867c40(layers, IR)`)

No corpus state draws one (only the Blur presets do). `FUN_00867b40(layers,
7) < 1` gates it (likely "any of the first three not flat"; not read). Per
frame `i` of the IR: each of layers 0, 1, 2 advances its runner by `dt =
layer0+0x68 / len` (`FUN_00791500`) and gives `v0, v1, v2` (context
`+0x38`); `FUN_00622470(m, v0 * 2, v1 * 2, v2 * 2, 0)` (the
pan/volume/separation matrix, `parts::stereo_gains`; `0x867f18` is 2.0),
then the frame goes through the 2x2 matrix (`PTR_DAT_008f6ef8`, one
frame). With no layer list the matrix is `FUN_006222f0(m, 2.0, ...)`. So
layer 0 is pan, 1 volume, 2 separation, each centred at 0.5. Read
`FUN_00867b40` and `FUN_00622470`'s use here, then build it before the
crop in `Convolver::build`; `layers_flat` already gates `fidelity()`.
