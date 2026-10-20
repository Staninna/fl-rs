# Drumaxx: handover

Started and stopped in one session: mapped far enough to size it, nothing
written in Rust. It is much more than one plugin's afternoon: Drumpad's
voice sixteen times over, a mixer, cut groups and a pattern sequencer that
keys start and stop.

## Where it is

- `Plugins/Fruity/Generators/Drumaxx/Drumaxx_x64.dll` is the Delphi shell;
  the plugin is `engine_x64.dll` beside it (577 KB, MSVC C++, its PDB path
  says `Drumaxx_x64.dll.pdb`). It is FL's C++ wrapper for its mobile synths
  (`CFLWrapper`, `CPluginRenderer`, `CSynthesizer`), the same wrapper as
  Drumpad's, BassDrum's and MiniSynth's engines (`docs/plugins/drumpad.md`,
  `docs/plugins/minisynth.md`): SSLF state, ProcessParam `int / 256`, the
  TriggerVoice velocity byte, the per-key slot fill.
- RTTI: `DMSynth` (vtable 0x180077f58), `DMPadGenerator` (the voice,
  0x180077f18), `CKickPad`, `DMEditor`, `DMInstance` (0x180077eb8),
  `CRijndael` (an AES, used for the `.dmpatch` files, not for the state).
- Corpus: 135 presets (`Data/Patches/.../Drumaxx`) and 1 project state, the
  render test `System/Render tests/Drumaxx.flp`; every state is 21644
  bytes. The dump script is the one in
  a scratch script of that session (not kept): it is
  `tools/plugstate/dump213.py` with case-insensitive names in both
  encodings, writing each payload to a file.

## Is it Drumpad's voice?

The same parameters by name (Mallet Amplitude, Decay, Noise, Noise RP,
Membrane Decay/Cutoff/Tension/Phase/Material/Size/Shape, LOW, MID, HIGH,
SOF, Lo-Fi, Hold, Pitch, four Velocity Mods, plus Noise Retrig, EG Retrig,
Solo) and the same machinery: a 38000 Hz render (`DAT_180078540`), 512
membrane lanes and 128 lane gains filled with `FUN_180046690` (sinf) at
trigger, the velocity slots that pick parameter `n + 6` (Drumpad: `n + 1`),
the voice's active flag at `+0x49e4`.

But it is another build: no Drumaxx voice function matches Drumpad's
instruction for instruction (compared with Capstone, rip constants by
value), and the voice's fields sit elsewhere (Drumaxx tests `+0x3890` for
its lane table where Drumpad's trigger tests `+0x49fd`). Reusing
`drumpad_voice.rs` needs each function read against it:

| Drumaxx | what | Drumpad's (docs/plugins/drumpad.md) |
|---|---|---|
| 0x18002c970 | parameters to patch, with the velocity blend `FUN_180035700` | 0x18001f4d0, 0x1800229f0 |
| 0x18002d1a0 | trigger: patch, lane gains, filters, seed | 0x18001fca0 |
| 0x18002d620 | reset (rate -> `38000 / rate` at `+0x49dc`) | 0x1800201c0 |
| 0x18002d8b0, 0x18002e3b0 | render and resampler, by their size (not read) | 0x180020460, 0x180020f50 |
| 0x18001bd70, 0x18001bed0 | filter setups called by the trigger | 0x180018bf0, 0x180018d50 |

The patch mapping read so far agrees in shape with Drumpad's table
(`drumpad.rs` header) with the parameter numbers moved: pad parameter
`p` of pad `n` is plugin parameter `44 n + p`, and the blend reads slots at
`+0x23..+0x26` and targets at `+0x27..+0x2a` of the pad.

## The synth (`DMSynth`)

- Parameters: 720 (`0x2d0`): 16 pads of 44, then 16 globals. Index 0x2c2
  (706) is the swing the sequencer reads; 5 is read as Noise Retrig.
- State (`FUN_1800303a0`, after `SSLF` and the length): a magic, `DMS4`
  (`0x444d5334`, saved as `4SMD`; `DMS1`/`DMD1` go to `0x18002ec80`,
  `DMS2`/`DMD2` to `0x18002f8b0`, `DMS3..5`/`DMD3..5` are this layout),
  256 bytes of kit name when the chunk is longer than 0x104, the 720
  parameters as f32, then 16 patterns of 0x402 bytes (two header bytes,
  then 0x200 u16 cells: 64 steps by 16 pads, high bit = hit, low 7 bits =
  velocity), then 0x800 bytes to `+0x673c` (pad names, by the UI), then 16
  bytes of cut groups (`+0x8f4c..+0x8f5b`), then more single bytes not yet
  read.
- Notes (`0x180031cc0` -> `FUN_180034e00`, key and velocity 0..1): a key
  equal to a pattern's trigger key (byte `+0x401` of pattern `i`) starts
  pattern `i` (`+0x6354 = i`); a note off on that key stops it. Other keys
  pick a pad by the map in the rest of `FUN_180034e00` (not read).
- Pad trigger `FUN_180035490`: velocity to the pad's voice, then every
  other pad in the same non-zero cut group is cut (`voice + 0x4c = 1`).
- Render `FUN_180034280`: per pad, gains `(1 - pan) vol slotL` and `pan
  vol slotR` from `param_1 + 0x858 + 4 pad` (vol) and `+0x40` (pan);
  while a pattern runs, steps of `rate * 15 / tempo * (1 +- swing / 2)`
  frames (16ths, swing on alternate steps), each step triggering the pads
  whose cell has its high bit, at velocity `(cell & 0x7f) / 127`, unless
  muted (`44 pad + 2` not 0) or another pad is soloed (`44 pad + 3`). The
  mix of the pads into the outputs is `FUN_180034990` (not read).

## Next steps

1. Read `0x18002d8b0` and `0x18002e3b0` against `drumpad_voice.rs`; if
   they match up to field offsets, make the voice reusable with the
   parameter offset and the `n + 6` slot base as arguments.
2. Read `FUN_180034e00`'s key-to-pad map and `FUN_180034990`'s mix
   (outputs: a pad can go to another mixer track, "Output track offset").
3. Write `drumaxx.rs`: 16 voices, one per pad, the state, the sequencer.
4. The render test plays notes into it; check which keys before relying
   on the pattern path.
