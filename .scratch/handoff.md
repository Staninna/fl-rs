# Handoff, 2026-10-22

Branch `claude/fervent-carson-e2hf4w`, PR Staninna/fl-rs#1. fmt, clippy, every workspace test with the corpus and `progress.py --check` pass at the head. Corpus render at `d807176`: 122 of 165 projects have sound and 82 play in full, up from 112 and 33 when this branch began. 1112 of 1147 reached effect slots are hosted, and 209 of 227 generator channels with notes play.

## On this branch

- Perf: the mixer runs the strips of each wave of the routing graph on a worker pool (`crates/engine/src/pool.rs`, `mix.rs`; `FL_RS_THREADS` sets its size, 1 is serial). Every harness hash is unchanged. The sine bank is FL's AVX kernel to its sum order and restart period (`dsp::additive`).
- New ports, each with its doc under `docs/plugins/`: Center, Mute 2, Phase Inverter, GMS, MiniSynth, BeepMap, Drumaxx, FL Slayer, Fruity Slicer 2, Kepler Exo, Fruity Granulizer, Wave Traveller, Fruity 7 Band EQ, Emphasizer (shell; LimiterLite is missing from the install), Patcher, and the Envelope, Formula, Keyboard and Voltage controllers.
- Larger pieces: Sytrus's unison, waveshapers, effects, filters, plucked operators, oversampling, global envelopes and portamento; linear phase for Parametric EQ 2 and Frequency Splitter; FL's sample interpolator (`dsp::interpolator`, at a render quality that is inferred); note colour, slide notes and live channel pitch for generators; automation targets that FL plays by setting nothing, Tension in controller formulas, and the sampler filter's mod X/Y (`docs/automation-234.md` lists what the render still counts unsupported).
- `tools/`: a script that renders every demo project to WAV, and a record of the commit each demo rating was made with.

## Left half done, with notes in `.scratch/notes/`

- Morphine and Sakura (`morphine*.rs`, `sakura*.rs`): source files written, which built on the tree they were written on, but not in `lib.rs`, `generator.rs` or `port.rs`, so they are not compiled. `morphine.md` and `sakura.md` list the lines to add and what is untested. Sakura matched its DLL run under unicorn to about 3e-5.
- Harmor: `harmor-wip.patch` is a diff left uncommitted when usage ran out. It has not been built on this tree.
- Fruity Convolver's EQ curve (`fruity-convolver.md`) and Transporter's DSP, TransientLooper, which is missing from the install (`transporter.md`). Slicex's declicker and Vintage Phaser's gaps are listed in their docs under `docs/plugins/`. FLEX is not started and has no note.
- Toxic Biohazard and PoiZone are not started.

The sections below are earlier states, kept for their detail.

## Handoff, 2026-10-19

The repo is on GitHub as `Staninna/fl-rs`, branch `main`. Everything is committed and CI is green on Linux, macOS and Windows. fmt, clippy and `cargo test -p plugins` with the corpus pass at `20f0570`, where a release corpus render hears 112 of 165 projects and 33 play in full. On 2026-10-18 the whole history was rewritten to the GitHub identity `Stan <26458805+Staninna@users.noreply.github.com>`, so hashes from before that date no longer resolve.

## Parametric EQs, 2026-10-20

- Fruity Parametric EQ 2's engine resampler (204 corpus states) and its matched HQ designer (61 states) are read from Frequency Splitter's `engine_x64.dll` and rebuilt in `crates/plugins/src/parametric_eq_2_engine.rs`. The designer was checked against the DLL's own code run under unicorn (`tools/plugstate/engine_emu.py`): the same to the bit with the DLL's libm. The plugin now reports 44 frames of latency with the engine resampler, and sleeps as FL does (only bands counting down are emptied).
- Fruity Parametric EQ (the first one) is rebuilt, Exact.
- Linear phase (8 states, 3 hosted slots) is the one part of Parametric EQ 2 left stood in for. `.scratch/notes/fruity-parametric-eq-2.md` hands it over; the same engine convolver is Frequency Splitter's, whose window guess it confirms and whose scaling it settles (IPP FFTs that do not scale).
- Corpus render at `c8145a9`: 112 with sound, 34 play in full (was 33), 932 effect slots hosted (was 928), 46 approximated (was 113).

## Plugin parameters for automation, 2026-10-20

The corpus automates generator parameters the engine could not play. Three commits:

- 3x Osc: FL numbers the instrument tab's parameters from 23 (`0x73ad35` hands 23 to `0x72c440`): five blocks of 17 words at 23..107, cutoff 110, resonance 111, type 113; 21, 22, 108, 109, 112 are nobody's. Parameter 110 in the corpus is the tab's cutoff and plays. `docs/plugins/3x-osc.md` section 2.
- Sytrus: all 597 REC parameters, decoded as `0x868c90` walks the parts, ranged by their controls, and applied as `0x867930` and its recomputes do. Mod X / Mod Y glide to the wheel per control tick when ModSmoothBtn is on (1 % left after 0.209 s, `0x8658b7`); the main pitch retunes sounding notes. Unison, chorus, delay, EQ and portamento parameters are stored only. `docs/plugins/sytrus.md` section 6.
- Harmor: articulator knobs (95..342) read and write the envelope / LFO knobs as FL does, smoothing 0..1001, and the 53 globals have FL's ranges (`harmor_state::GLOBAL_RANGES`).

The corpus summary's "unsupported targets" (clips in the first 10 s) went from 140 to 133, clips evaluated from 187 to 194; across whole songs every Sytrus 0x8000+ and 3x Osc 0x8000+ binding now resolves. What is left is engine work, not plugin work: raw channel controls 4 (channel pitch, -4800..4800 cents, 36 bindings on 3x Osc channels and more on samplers), 17 (90 on Sytrus channels, unknown), 16, 0x600/0x604, and 0x2000 + n on Sytrus channels (0x2012, 0x2013, 0x2000, 0x2226, in `Synth_dfr - Overcome - Sytrus` and `nucleon - Housework`), which look like an older numbering of plugin parameters but are not confirmed from FL's REC dispatcher (`FUN_01046a70` in FLEngine). MiniSynth and FLEX are not built.

## Perf work, 2026-10-19

Heavy demo projects dropped out in the app. Done so far, each measured with `crates/engine/examples/profile.rs` (README, Tests):

- Dev builds optimise `dsp`, `engine` and `plugins` (`c4dbd55`). Unoptimised, Ookay - Thief played at 0.4x realtime.
- An automation range is taken in f64 (`6df1664`). Harmor left part parameter 106 and globals 772 and 774 unbounded, so their range read back as the whole i32 and `high - low` overflowed in five projects. Harmor now clamps every parameter to FL's ranges (see "Plugin parameters for automation" below).
- The `profile` feature on `engine` times each generator and slot per call (`af47740`); the harness prints the six costliest per project with `--features profile`.
- Fruity Convolver's 128-tap direct filter sums 16 frames side by side (`976e553`) and the sine bank runs 8 partials side by side (`7d196fb`). Both keep all 165 harness hashes. On one core, Stroll Up went from 0.9x to 1.3x realtime and Harmless Galactic from 1.9x to 2.6x.

- The engine mixes with flush-to-zero and denormals-are-zero set (`d7b27bc`), as FL's engine does: `FLEngine_x64.dll` keeps MXCSR `0x9f80` at `0x13e4d50`, ORs in `0x40` at `0x63ab2b` when the CPU has DAZ, and loads it at `0x63a6d0` from 71 places. Before, one corpus run took 3.1 billion floating point assists, mostly in Vocodex, Frequency Splitter, Fruity Convolver, LuxeVerb, Sytrus and Harmor's effects. The user chose this over keeping the bits: 10 of 165 harness hashes changed, only below about 1e-38. `Engine::process_with` is the one place the workspace allows `unsafe` (the lint is `deny`, not `forbid`).

All together, against the code before this work, medians of three runs at 512 frames: the corpus takes 25% less time, and 16 projects have a call over budget instead of 23 (13 at the 1024 frames PipeWire uses here). Stroll Up plays at 2.2x realtime instead of 0.6x, Vocodex demo at 5.5x instead of 2.1x, and Ookay - Thief and Harmless Galactic no longer run over.

What the worst left spend their time on (`--features profile`): Bien Duro and Bombs Away - I'm Awake spread it over many inserts (the top six parts under 30%), so running inserts in parallel would pay there. Harmless Essentials is 99.5% one Harmless and Square a Saw - Oxygen 66% one Harmor, so only a faster plugin helps those. Harmor's partial bank already runs eight partials side by side.

Next, for the user to choose:

- Spread Fruity Convolver's large partitions over the calls before their deadline (FL runs those over 4096 samples on a worker thread). It cuts its bursts, but every partition adds into one output buffer, so the order of the sums and the low bits change. The user said: decide after the denormal fix, with the harness's numbers.
- Run independent inserts in parallel (plan step 5): workers at real-time priority and in macOS's audio workgroup, a fallback to serial, the denormal flags on every worker, and tests for routes, sends and sidechains.
- Harmless and Harmor themselves: their own reverb, Maximus and partial banks.

Found on the way: 44 of 165 projects come out a little different at 1024 frames a call than at 512, and 55 at 256. A note splits the blocks, so plugins that work once per block (Harmless, Harmor, DX10, Drumpad, PanOMatic, Multiband Compressor and others) see other block lengths. FL's output depends on its buffer size for such plugins too, so this may be right; it is why the harness hashes are only compared at one call size.

Maximus's saturator is FL's `powf` and `atan` per sample, with no range where it can be skipped without changing bits, so it stays as it is.

## Since 2026-10-14

The five plugins below are committed, one commit each, after fmt and seven clippy fixes in `pitch_shifter.rs`. Ogun's commit carries the Autogun interpolator fix too: `Setup::linear` has to be filled in `autogun.rs`, which also holds Ogun's changes, so the two could not be split. Rows in `docs/plugins.md` are filled in from the notes.

Drumpad is hosted since `20f0570`. Building it fixed two bugs in the draft: `Voice::play` now takes its buffer out while it renders, and `trigger` lays line `i` out with `512 - 4i` slots, not `128 - i`. Every built plugin now has its doc, Drumpad and the four above included, and `docs/plugins/autogun.md` reads versions 0 to 19 and quality 0's linear interpolator as the code does. `.scratch/notes/` keeps the handovers these docs grew from.

Next: find what keeps 132 of the 165 projects from playing in full. The corpus render's per-project lines name what each left out or approximated; rank those before picking more plugins.

`docs/plugins.md` now carries the corpus render count ("111 of 165 shipped projects have sound"), and `tools/progress.py` builds the README's progress block (table and lists of plugins built) from it and `docs/parity.md`. Pipe every corpus render into `python3 -I tools/progress.py --render` and it updates that line itself.

The section below is the state as of 2026-10-14, kept for its detail on each plugin.

## Committed this session

Harmor, Vocodex (two sidechain inputs, mix.rs `side: [Option<usize>; 2]`), EQUO, Hyper Chorus, FL Keys, Autogun. Fixes: `MEnvelope::read` sets decay -1 for LFOs. `engine/src/sample.rs` `prepare()` calls `set_resource_dir` before `set_state`, which brought FL Keys' Grand Piano to life in realtime. A release corpus render at `971d670` hears 108 of 165 projects, and the README says so.

## Uncommitted, one commit per plugin

Every agent stopped mid-task and wrote its handover to `.scratch/notes/<plugin>.md`: DLL addresses, what is confirmed and what is guessed, next steps. Read the note before touching a plugin. Every one of them still needs `docs/plugins/<name>.md`, its row in `docs/plugins.md`, a README mention, clippy and fmt.

- **Ogun** (`ogun.rs`, plus edits in `autogun.rs` and `autogun_engine.rs`). It plays: its 7 silent channels with notes now have sound, and the agent counts 110 of 165 projects with sound. Its DLL is Autogun's code function for function, so `Ogun` wraps `Autogun` with `Kind::Ogun`. The `autogun_engine.rs` change, FL's linear interpolator for quality 0, also corrects Autogun, so it may belong in its own `fix(plugins)` commit before Ogun's. Clippy and fmt are clean on its files.
- **Fruity DX10** (`dx10.rs`). This is mda DX10 with a second modulator. 10 tests pass, and all 74 shipped states parse exact. Registered in lib.rs and generator.rs.
- **Frequency Splitter** (`frequency_splitter.rs`). 12 tests pass. It is registered in lib.rs with the row `("frequency splitter", 113, 17)`. Band routing to other tracks would need engine work, but no shipped state sets a target track, so the engine was left alone.
- **Pitch Shifter** (`pitch_shifter.rs`). Its DSP is two engines in `Shared/ildsp_x64.dll`, PSOLA and a general granular shifter, which is not Luxeverb's class. 11 tests pass. It is registered in lib.rs with the row `("pitch shifter", 75, 15)`. The Lagrange read and random step are copied from `luxeverb.rs` and could move into `dsp`.
- **Drumpad** (`drumpad.rs`, `drumpad_voice.rs`). Written but **not registered and never compiled**. Its DSP is in `engine_x64.dll`, the same framework as BassDrum. Next: add the mod line and generator entry, fix compile errors, then run its tests.

`lib.rs` holds the length-count assertion in `every_state_in_fl_s_projects_loads`. It is 37 in the tree: 35 plus Frequency Splitter plus Pitch Shifter. Commit Frequency Splitter with 36, then Pitch Shifter with 37. That test may need one re-run, since it failed once while the two rows were still landing.

## How to commit in a shared tree

- `lib.rs` and `generator.rs` mix lines from several plugins, so stage them partially: `python3 -I .scratch/stage_edits.py <spec>.json`, with spec `{"path": [[old, new], ...]}`. `old` must be the text at HEAD, and the file must not be staged already. `new` can be `{"row": "prefix"}` to take the live line that starts with that prefix.
- Verify the staged content alone in a scratch worktree, never in the live tree:
  1. `git worktree add --detach $W HEAD`, then `git diff --staged --binary | git -C $W apply --index`.
  2. In `$W`, with a scratch `CARGO_TARGET_DIR`: `cargo fmt --all --check`, `cargo clippy -q -p plugins -p engine --all-targets -- -D warnings`, `cargo test -q -p plugins`, `cargo test -q -p engine --lib --test fixtures`.
  3. Remove the worktree.
- Read each staged file's diff with its own standalone `RTK_DISABLED=1 git diff --staged -- <file>` call, then commit with a heredoc. A commit hook rejects the commit until every staged line has been read. If it rejects the first try, run the identical commit again.
- `cargo test --workspace` and the app tests open the sound card. Leave them out.
- Corpus render, about 3 minutes in release: `FL_RS_CORPUS="/shared/code/fl-studio-target/winehome/prefix/drive_c/Program Files/Image-Line/FL Studio 2026/Data" cargo test --release -q -p engine --test corpus -- --nocapture`. Its summary lines give the audible count and the most-silent and most-passed lists.
- `.scratch/agent-rules.md` is the rules file each plugin agent was given. Point new agents at it.

## Next by corpus usage (at 20f0570)

Silent generators: FLEX 13 (patches in obfuscated `.flexpack` archives), Fruity DrumSynth Live 2, Fruity Slicer 2, GMS 2, Kepler 2.
Passed-through effects: Fruity Wrapper 17 (third-party VSTs, out of scope), Patcher 15, Wave Candy 11 and ZGameEditor Visualizer 7 (visualisers, passing through is right), Emphasis 5.

Smaller follow-ups: Pitcher's MIDI Out routing, Emphasis, Vintage Chorus. `tools/plugstate/dump213.py` undercounts 8-bit preset names.

## Open with the user

- Patcher: a plugin graph inside one slot, which needs engine work. Plan it with the user before assigning it.
- FLEX: try to crack the `.flexpack` obfuscation, or leave FLEX out.
- `1bd9782`'s message says "No shipped project stores one". That was inferred from no peak changing, not checked. The user knows, and it stays unamended.

## Standing instructions

- Headless only. No app runs, audio, browsers, wine, or anything that shows on screen.
- Use subagents freely: opus at high effort for plugin work, each owning its own files.
- Keep git current with atomic commits. `.scratch/` is tracked: commit the handoff and notes with the code they describe (`CLAUDE.md` says so too).
