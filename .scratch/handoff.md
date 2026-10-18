# Handoff, 2026-10-18

The repo is on GitHub as `Staninna/fl-rs`, branch `main` (local branch `phase-4` tracks it). Everything is committed. fmt, clippy and `cargo test -p plugins` with the corpus are green at `372526f`. A release corpus render there hears 111 of 165 projects.

## Since 2026-10-14

The five plugins below are committed, one commit each, after fmt and seven clippy fixes in `pitch_shifter.rs`. Ogun's commit carries the Autogun interpolator fix too: `Setup::linear` has to be filled in `autogun.rs`, which also holds Ogun's changes, so the two could not be split. Rows in `docs/plugins.md` are filled in from the notes.

Still open for each of Ogun, Fruity DX10, Frequency Splitter and Pitch Shifter: `docs/plugins/<name>.md`. Drumpad is committed as a draft: not in `lib.rs` or `generator.rs`, never compiled. Next for it: add the mod lines and the generator entry, fix what fails to compile, run its tests.

`docs/plugins.md` now carries the corpus render count ("111 of 165 shipped projects have sound"), and `tools/progress.py` builds the README's progress table from it and `docs/parity.md`. Update that line after every corpus render.

The section below is the state as of 2026-10-14, kept for its detail on each plugin.

## Committed this session

Harmor, Vocodex (two sidechain inputs, mix.rs `side: [Option<usize>; 2]`), EQUO, Hyper Chorus, FL Keys, Autogun. Fixes: `MEnvelope::read` sets decay -1 for LFOs. `engine/src/sample.rs` `prepare()` calls `set_resource_dir` before `set_state`, which brought FL Keys' Grand Piano to life in realtime. A release corpus render at `bdcaa85` hears 108 of 165 projects, and the README says so.

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

## Next by corpus usage (at 372526f)

Silent generators: FLEX 13 (patches in obfuscated `.flexpack` archives), Drumpad 6 (drafted), Fruity DrumSynth Live 2, Fruity Slicer 2, GMS 2.
Passed-through effects: Fruity Wrapper 17 (third-party VSTs, out of scope), Patcher 15, Wave Candy 11 and ZGameEditor Visualizer 7 (visualisers, passing through is right), Emphasis 5.

Smaller follow-ups: Pitcher's MIDI Out routing, Emphasis, Vintage Chorus. `tools/plugstate/dump213.py` undercounts 8-bit preset names.

## Open with the user

- Patcher: a plugin graph inside one slot, which needs engine work. Plan it with the user before assigning it.
- FLEX: try to crack the `.flexpack` obfuscation, or leave FLEX out.
- `8e9ed44`'s message says "No shipped project stores one". That was inferred from no peak changing, not checked. The user knows, and it stays unamended.

## Standing instructions

- Headless only. No app runs, audio, browsers, wine, or anything that shows on screen.
- Use subagents freely: opus at high effort for plugin work, each owning its own files.
- Keep git current with atomic commits. `.scratch/` is tracked: commit the handoff and notes with the code they describe (`CLAUDE.md` says so too).
