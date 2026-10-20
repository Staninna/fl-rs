# Frequency Splitter: handover notes

## Status

- `crates/plugins/src/frequency_splitter.rs` is written: state parser, IIR crossovers, linear-phase FIR mode, level glide, mutes, 12 unit tests. `cargo check -p plugins --tests` is clean. All 12 tests pass, the corpus test included (`FL_RS_CORPUS=... cargo test --release -p plugins frequency_splitter`).
- Registered in `crates/plugins/src/lib.rs`: `pub mod frequency_splitter;` and a `by_name` branch after Flangus. I added the row `("frequency splitter", 113, 17)` to `every_state_in_fl_s_projects_loads`. The Pitch Shifter agent set `lengths.count()` to 37 at the same moment, and 37 already covers both new plugins. Re-run that test to confirm once their row settles.
- Not done yet: `docs/plugins/frequency-splitter.md`, the Frequency Splitter row in `docs/plugins.md`, clippy and fmt on my file, and the engine corpus render (before 9 slots passed through, after not measured).

## Corpus

17 project states (Astes - Bien Duro 14, Atthar 2, Sid De La Cruz 1) and 9 presets plus 3 database copies. All are version 4, 113 bytes. Every target mixer track is 0, so no shipped state sends a band anywhere. Four Astes states use linear phase (inserts 50, 54, 56, 82).

## The shell (`Frequency Splitter_x64.dll`, Delphi, base 0x400000), confirmed

- `CreatePlugInstance` 0x73f140 calls constructor `FUN_00740280` with class ref `PTR_PTR_0073ef58`. VMT 0x73f020: Dispatcher 0x73f380, SaveRestoreState 0x73fac0, GetName 0x741cd0, ProcessEvent 0x740de0, ProcessParam 0x740e30, Eff_Render 0x7423b0. `FUN_00741e10` loads `engine_x64.dll`. PlugInfo is at 0x7a2204 with NumParams 14.
- State: i32 version (read when < 5), 17 i32 at this+0x1dc (params 0..13, then the 3 target tracks at +0x214), i32 monitor, i32 (discarded), i32 window toggle bits, i32 mute bits (bits 0..5 = low/mid/high send, low/mid/high main; 1 = muted), then a u8 flag byte (v>=2), an i32 (v>=3) and 5 i32 (v>=4, analyser settings). Lengths: v1 88, v2 89, v3 93, v4 113.
- ProcessParam maps the parameters as follows:
  - params 0/1 go to engine 0/1 as min/max of the two wheels, as share v/65536.
  - Slopes go as v/7.0. The engine picks [6,12,18,24,36,48,72,96] by eighths.
  - Param 4 goes as a float (mode = int(p*1.998)). Param 5 goes as a float (2 bands when below 0.5).
  - Quality goes as v/5.0 (int(p*5.994)). Speed goes as v/65536.
  - Levels 8..13 go as (v+1800)/3600.
  - Mutes are sent from the bit field (`FUN_00740050`) as engine params 14..19. Solos (20..25) are never set.
- Eff_Render: the engine processes 2 inputs into 8 outputs. Output 6/7 (main) is the insert's output. Band i (outputs 2i, 2i+1) is added to mixer track `targets[i]` through PlugHost vtable+0x128 when the target is above 0.

## The engine (`engine_x64.dll`, MSVC VST, base 0x180000000), confirmed

- Constructor `FUN_1801e7ab0` builds a 0x11f0 object. setParameter is `FUN_1801e6d00` (26 params). processReplacing is `FUN_1801e56d0`. setSampleRate is `FUN_1801e55c0`: the glide becomes 1-0.1^(200/sr), all params are re-sent, and the filters are marked dirty.
- Freq is `powf(2, p*9.058894)*30`. Level is 2p below 0.5, else 1+(p-0.5)*13.886564. Main gains (+0x1138..) glide each frame toward level*!mute.
- IIR mode: crossover objects at +0x130/+0x650/+0xb70 (`FUN_1801e7f10` runs one, `FUN_1801e8190` designs one).
  - The design runs up to 8 biquads on 4 lanes (low pass L/R, high pass L/R). Biquads use RBJ in `FUN_1801e33b0`; one pole uses tan.
  - Q tables are in the source. 12 and 36 dB invert the high pass. 18 dB uses the allpass pair at +0x500/+0x508.
  - Three bands: c0 at f1, c1 on c0's high at f2, c2 on c0's low at f2. Low = c2.lp + c2.hp, mid = c1.lp, high = c1.hp.
- Linear mode `FUN_1801e4890`/`4730`/`4ce0`/`2c90`/`2ad0`:
  - FFT N = 512<<q, scaled by the rate's octaves (`FUN_1801e6310`). Latency is 3N/4.
  - The magnitude curve uses quick log2/exp2: -6 dB at the crossover, slope on the stop side, complementary on the pass side.
  - Mid = input delayed 3N/4 minus low minus high. In two-band mode, high = delayed minus low.
  - Two filter sets crossfade over speed*sr frames. A new set is primed with N frames of history.

## Guessed, not confirmed

- Settled since (2026-10-20): the window is the half sine, the FFTs scale neither way, and a filter is overlap-added in blocks of N/2. The filters are now the shared convolver in `crates/plugins/src/linear_phase.rs` (`docs/plugins/maximus.md` section 5.8).
- Main gains are assumed to start at 0 and glide in, because the engine object comes from operator new and those fields are never initialised.
- FTZ/DAZ are not reproduced. libm sin, cos, tan and powf may differ by an ulp.

## Band routing and the engine

The engine cannot carry these sends, but no corpus state needs them. The plugin has three stereo band outputs, and each goes to an absolute mixer track number (1..999). `Effect::aux_output` and `Hosted.aux` in `crates/engine/src/mix.rs` give one stereo second output. That output goes to one insert, counted among the inserts the plugin's own insert sends to. Routing the bands would need two engine changes:

1. A list of second outputs per plugin.
2. Addressing by absolute insert index, with that insert reached and ordered after the source.

The plugin reports `Fidelity::Approximate` when a target is set.

## Next steps

1. Write `docs/plugins/frequency-splitter.md` from this file, and fill in the `docs/plugins.md` row: IIR exact up to libm; linear phase approximated (window and scale inferred); band sends not routed, none in the corpus.
2. Run `cargo clippy -p plugins --all-targets -- -D warnings` and `cargo fmt` on the file.
3. Run the engine corpus render for the before/after slot numbers and check the levels.
4. Confirm `every_state_in_fl_s_projects_loads` passes with the Pitch Shifter row in place.
