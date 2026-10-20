# Fruity Parametric EQ 2: handover for linear phase

Everything else in the plugin is rebuilt from the binary (cookbook, matched
HQ designer, both 2x resamplers, sleep). Linear phase is the one part still
stood in for: 8 corpus states, 3 hosted slots in the corpus render. It runs
the IIR bands as they are (minimum phase, no latency). Read
`docs/plugins/fruity-parametric-eq-2.md` section 3 first.

## Confirmed (2026-10-20)

Plugin side, `FUN_006eddc0` (decompile read, not transcribed):

- `N = FUN_006edd00(step 0x36c, SR)`: `512 << step`, halved under 44.1 kHz,
  else shifted by `trunc(log(SR * c1) * c2 + c3)`; constants at
  `0x006edd68`, `0x007615f8`, `0x006edd70`; at most 65536. Latency reported
  `3N/4`.
- Kernel when `0x378` (dirty) is set and engine op 0xe says the last fade is
  done: for `k = 0..N/2`, `f = max(DAT_006ee570, k * SR/N)`; when
  `f <= host+0x30` (`float(0.4999 SR)`) it is snapped to the centre of the
  first band whose GUI flag `this + 0x1371 + b*0x228` is set and whose
  centre lies between the previous bin's `f` and this one; then
  `w = float(f * (host+0x18) * 2pi)`, `SinCos(w)`, `SinCos(2w)`, and the
  product over enabled bands and their sections of
  `|b0 z^2 + b1 z + b2|^2 / |z^2 + a1 z + a2|^2` (float accumulation, the
  section's own b/a doubles, so it follows the matched or cookbook design
  in use). `dB = 10 log10(product)` clamped to `[DAT_006ee588,
  DAT_006ee58c]`, then back to a magnitude by the quick 2^x polynomial at
  `0x006ee5a0..b8`. Bins above the top get `DAT_006ee560`. Then
  `kernel[N/2] = kernel[N/2 - 1]`.
- Then, under the host lock: engine op 5 (SR), op 6 (window seconds from
  the table at `0x0074e708` by `0x374`: 0.1, 0.25, 0.01, 0.02), op 2 (N),
  op 3 (kernel, N/2 + 1 floats).

Engine side (`engine_x64.dll` in Frequency Splitter's folder,
`mydispatcher` @ `0x1801e1e50`, jump table `0x1801e2710`):

- op 0 (`0x1801e1ec6`): context of 0x48 bytes, SR 44100 at +0x30, window
  0.1f at +0x34, fade length +0x3c = 0x1000, then `FUN_1801c8850(ctx,
  256)`.
- op 2 = `FUN_1801c8850(ctx, N)`: when N changes, a kernel buffer of
  N/2 + 1 floats, two convolvers (`FUN_1801e3040`, 0xa8 bytes: [0] N,
  [1] N/2, [2] N/4) each designed from the (zero) kernel, two history rings
  of N floats.
- op 3: copy N/2 + 1 floats to the kernel buffer and flag it new.
- op 4 = `FUN_1801c8a70` per frame: when a new kernel is flagged and the
  fade is done, clear the idle convolver (`FUN_1801e2a50`), design it
  (`FUN_1801e2c90`), prime it with the last N frames of history, start a
  fade of `(int)(SR * window)` frames and make it the playing one. Each
  frame goes into the history; during a fade both convolvers run and the
  output is a linear crossfade by `faded / fade_len` (`1 - t` for the
  first convolver when it is the new one); else only the playing one.
- `FUN_1801e2c90` (design): an FFT object of N (`FUN_1801de130`: a float
  buffer of N with 1.0 at N/2, and two IPP complex FFT specs made by
  `FUN_1801e1890` with order `log2 N` and flag 8 = `IPP_FFT_NODIV_BY_ANY`,
  so neither direction scales), `FUN_1801e1960` (IPP calls, arguments lost
  by the decompiler: read them from the disassembly), then the N/2 taps
  from offset N/4 are multiplied by a half sine window `sinf(i * pi /
  (N/2 - 1))`, i = 0..N/2-1, the vector path taking `1/(N/2 - 1)` by
  `rcpps` and one Newton step (so the window is CPU dependent in its last
  bits), then copied into a complex buffer of N (taps in the first half,
  zeros after) and forward FFT'd (IPP).
- `FUN_1801e2ad0` (per frame): stores L and R into two complex input
  arrays at the running index (real parts), returns the frame from two
  output float arrays at that index, and every N/2 frames runs the IPP
  FFT / multiply / inverse FFT (arguments again lost by the decompiler).
- op 0xd clears; op 0xe reports whether the fade is done.

Frequency Splitter's linear phase mode (`crates/plugins/src/frequency_splitter.rs`,
`Convolver`) uses the same `FUN_1801e2c90` / `FUN_1801e2ad0` and guessed
the window and the scaling. The two findings above confirm the sine window
and settle the scaling: IPP's FFTs here do not scale.

## Next steps

1. Read the IPP calls in `FUN_1801e1960` and `FUN_1801e2ad0` from the
   disassembly (which FFT direction, which buffers, the multiply), and the
   2^x polynomial and clamps in `FUN_006eddc0`, and the code that sets the
   GUI flag at `+0x1371`.
2. Rebuild the engine convolver once, shared with Frequency Splitter
   (whose `Convolver` is close), and the kernel builder in
   `parametric_eq_2.rs`; report `3N/4` latency.
3. For reference output, the engine can run under unicorn
   (`tools/plugstate/engine_emu.py` is the start): it needs the CRT
   allocators (`operator_new` 0x1801c9110, `_calloc_base` 0x1801cf438,
   free 0x1801cf440) hooked, and IPP will take whatever path the emulated
   CPU's cpuid picks. FFT rounding will not match FL's to the bit on any
   path but FL's own CPU's.
