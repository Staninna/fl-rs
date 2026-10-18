# Drumpad handover

## Where the DSP is

`Plugins/Fruity/Generators/Drumpad/engine_x64.dll` (449 KB, MSVC C++, RTTI names `DrumpadSynth`, `DMPadGenerator`, `CSynthesizer`, `CVoiceGenerator`, `CFLWrapper`). `Drumpad_x64.dll` is the generic Delphi wrapper. Same framework as BassDrum's engine (SSLF state, `bassdrum::velocity_level` velocity). Nothing in `fl_fx.rs` / `menvelope.rs` applies: those are Delphi classes.

Decompile of the whole `.text`: scratchpad `drumpad/all.c` (headless Ghidra, `tools/ghidra/DecompileRange.java`), annotated asm via `drumpad/dis.py <start> <end>` (resolves rip constants). Ghidra project under `drumpad/gh` (closed).

## Confirmed from the binary (asm checked)

- Wrapper vtable `0x18005a1a0`: SaveRestoreState `0x180002f10` (SSLF, u32 len, chunk), ProcessParam `0x180003420` (value = int/256 clamped 0..1; get = int(f*256)), TriggerVoice `0x1800048f0` (key = int(InitLevels.Pitch*0.01) clamp 0..127; vel byte = int(127*ln(1+10*Vol)*0.328458726) clamp; returns key as handle), Voice_Release `0x180004a10` (synth note off = no-op), slot fill `0x180003dd0`, key cleanup `0x180004840`.
- Config at `0x180075de0`: 38 params (`+0x11c`), polyphony 99 (`+0x124`).
- Chunk load `0x180022de0`: magic DMP1..3/DPD1..3; sets p17=0, 128-byte name if len-4>0x80, p12=1, p17=0, then up to 38 f32. v1/v2 remap not implemented. Save `0x180022d00` writes 0x11c bytes. Corpus: all DMP3; projects 284-byte chunk, presets/render test 288 + 4 trailing bytes (39th float ignored by loader).
- Param names `0x180023450`, defaults `0x180023290`, display `0x180023330`.
- Synth vtable `0x18005c300`: note on `0x1800077a0` → `0x18001e570` → poly alloc `0x18001d7b0` (voice+0x20 = powf(vel/127, 0.5), exponent at synth+0x38d8 = 0.5). Note off = no-op. Render `0x180022b30`: voice i reads per-KEY slot i (synth+0x20/+0x220 L/R, +0x420 semis); bend = powf(2, clamp(semis,-12,12)/12).
- Velocity mod blend `0x1800229f0`: slots p30..33 pick param n+1, targets p34..37, result (1-x)base + x target.
- Voice: ctor `0x18001f1a0` (rng seed: global LCG *0x1c3c718d - 0xaf2c25b, 1000 steps/voice), params→patch `0x18001f4d0`, trigger `0x18001fca0` (gain table: triangle of phase, power 20m^4+1), reset `0x1800201c0`, 38 kHz render `0x180020460`, resampler/output `0x180020f50` (256-frame passes, linear interp, gain glide 0.001, gain -1 = unset).
- Membrane: 128 lines of (512-4i) slots x 4 lanes; lane k = delay 512-k (verified by simulating the pointer/shift scheme, `drumpad/lanes.py`); per lane one-pole lowpass c^2, write lp*(-decay^0.3)+mallet.
- Filters: LOW highpass `0x180018bf0` (sincosf, mixed-alpha coefficients), RBJ peak/high shelf `0x180018d50` with A = 10^(dB/20) (dB doubled), HIGH shelf type 2, MID peak type 1 retuned every 6 samples, SOF self-oscillating 4-pole ladder (feedback -1e5, sat v/(1+0.005|v|)), hold envelope, lo-fi, silence end (25 ms after sounding, 0.5 s otherwise).
- CRT ids: `0x180031d40` powf, `0x1800306a0` pow, `0x1800434f0` sincosf, `0x1800456f0` sinf, `0x180044df0` cosf, `0x180045de8` sinh (double). `0x1800452f0` taken as logf (constant 1/ln 21 fits); not disassembled to the end.

## Guessed, not confirmed

- `0x180045de8` is sinh and `0x1800452f0` is ln (from constants/usage only).
- FinalLevels.Pitch semantics for the slot pitch (taken from docs/plugins/host-voice.md): slot semis = fine/100.

## Built (not registered, not compiled yet)

- `crates/plugins/src/drumpad_voice.rs`: Patch, lane gains, LOW/biquad, Voice (membrane emulating FL's pointer scheme exactly, render, resampler). Unit tests inside.
- `crates/plugins/src/drumpad.rs`: state parse, params, 32-voice pool, per-key slot quirk, tail, fidelity, tests incl. corpus test.
- Approximations chosen: 32 voices instead of 99 (steal clears lines), lines sized per state (`room`), per-instance RNG start, DMP1/2 rejected.
- Nothing outside my files touched. No `mod` lines added yet, so `cargo check` does not see these files; they have never compiled.

## Next steps

1. Add `pub mod drumpad; mod drumpad_voice;` to `lib.rs` and a `(crate::drumpad::NAME, || Box::new(crate::drumpad::Drumpad::default()))` entry in `generator.rs` (re-read both first; other agents edit them).
2. `cargo test -p plugins drumpad` and fix compile errors (likely: `let ... && let` chain in `process` needs edition 2024, `Fidelity` needs PartialEq for the corpus test assert, borrow in `Voice::membrane`).
3. Run corpus test with `FL_RS_CORPUS`, check room per voice and max concurrent voices in the two projects; raise VOICES if the render shows steals.
4. Render corpus (`cargo test --release -p engine --test corpus`) before/after; Madonna demo and EDM-House template plus `Render tests/Drumpad.flp`.
5. Write `docs/plugins/drumpad.md` from this note and the module docs; update the Drumpad row in `docs/plugins.md`.
6. clippy + fmt.
