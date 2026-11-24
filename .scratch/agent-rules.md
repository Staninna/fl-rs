# Rules for agents working in /shared/code/fl-rs

fl-rs is a Rust DAW that plays FL Studio projects by rebuilding FL's stock plugins from their binaries. Read `README.md` and `.scratch/plan-phase4.md` (the method per plugin) before you start.

## Where things are

- FL's install: `/shared/code/fl-studio-target/winehome/prefix/drive_c/Program Files/Image-Line/FL Studio 2026`. Plugin DLLs under `Plugins/Fruity/`; some keep their DSP in a separate DLL beside them or in `Shared/`.
- Its `Data/` directory is the corpus: 165 shipped projects, render-test projects in `Data/System/Render tests/`, presets under `Data/Patches`.
- Reverse engineer with the REA MCP tools (load them via ToolSearch) or headless Ghidra with the scripts in `tools/ghidra`. `tools/plugstate` has state-dump helpers. Several agents run Ghidra on this 30 GB machine at once: close binaries and projects when you are done with them.
- Finished plugins to copy the style of: `crates/plugins/src/chorus.rs` + `docs/plugins/fruity-chorus.md` (small effect), `crates/plugins/src/gross_beat.rs` (tempo-synced effect), `crates/plugins/src/harmless.rs` + `crates/plugins/src/generator.rs` (generator), `docs/plugins/harmor.md` (a deep map).

## Shared tree

Other agents edit other files in this same working tree right now.

- You own the files your task lists. Outside them, add only the minimal registration lines (a `mod` line and a `by_name` branch in `crates/plugins/src/lib.rs`, an entry in the generator registry in `generator.rs`, a row in a test table), and re-read the file just before each such edit.
- Keep your files compiling at every save: stub first, then fill in.
- If the build fails in a file you do not own, wait a minute and retry. Leave that file to its owner.
- Update only your own plugin's row in `docs/plugins.md`. Leave `README.md` alone.
- Git is read-only for you (status, diff, log, show). The coordinator commits. Add, commit, stash, checkout, restore or reset can destroy other agents' work.
- The repo is public on GitHub (`Staninna/fl-rs`), and people outside this tree work on it too. Before you start, run `gh issue list --state all --search "<plugin>"` and `gh pr list --state all --search "<plugin>"`, and put what you find in your report. When someone has the plugin in flight, stop and report before you write code. `gh` is read-only for you too: open, comment on or close nothing.

## Worktree mode (2026-11-24 onward)

When the coordinator starts you in your own git worktree (your cwd is not the main checkout), the "Shared tree" rules above give way to these:

- Your worktree is yours. Commit there, atomically, as you go (`git add <files>`, `git commit`), with the attribution lines the coordinator gives you. Never push, never switch branches, never touch the main checkout or another worktree.
- Still keep edits outside your own files minimal (registration lines, one `docs/plugins.md` row, one `port.rs` line): other agents edit the same shared files on their branches and the coordinator merges them all.
- Leave `README.md`'s generated progress block alone; the coordinator runs `tools/progress.py`.
- FL's install, unpacked by `tools/fl-extract.py`, is at `/root/fl-install` (also reachable as the `/shared/code/fl-studio-target/...` path the docs name). `FL_RS_CORPUS=/root/fl-install/Data`.
- Ghidra 11.4.2: `/root/re/ghidra_11.4.2_PUBLIC/support/analyzeHeadless`, JDK 21 installed. Keep projects under `/root/ghidra-projects/<your task>/`; set `MAXMEM=2G`. Python has capstone, unicorn and pefile (`tools/plugstate/` has emulator scripts to copy).
- The machine has 4 cores and 15 GB shared by several agents: build with `CARGO_BUILD_JOBS=2`, run the full corpus render only when you need its numbers (it takes about 4 minutes alone), and prefer `cargo test -p plugins <filter>` and single-project renders while iterating.
- Your worktree has its own `target/`; the first build takes a while.

## Headless

The user is working at this machine. Open no windows and play no sound: no `cargo run -p fl-rs`, no browsers, no wine, no GUI capture tools, no sudo.

- Test with `cargo test -p plugins` (set `FL_RS_CORPUS="<FL install>/Data"` for the corpus tests).
- Render the corpus offline with `FL_RS_CORPUS="<FL install>/Data" cargo test --release -p engine --test corpus -- --nocapture`.
- Skip `cargo test --workspace` and the `fl-rs` app tests: they open the sound card.
- Put temp files in your own directory under `/tmp/claude-1001/-shared-code-fl-rs/cd8fe48c-0ad3-4ca4-accb-d019123d9334/scratchpad/<your task name>/`.

## Done, for a plugin

- It parses every corpus state of it.
- It renders its render-test project and the corpus projects that use it with no NaN and at a sane level.
- Unit tests derived from the maths pass.
- `cargo clippy -p plugins --all-targets -- -D warnings` and `cargo fmt` are clean for your files.
- Its doc in `docs/plugins/<name>.md` maps the state and the signal chain with function addresses.
- Its `docs/plugins.md` row says what is exact and what is approximated, and its line in `crates/plugins/src/port.rs` gives the level that row bears out: `Exact` when nothing is approximated or left out, `Gaps` when the row names parts that are, `Approximate` when the sound itself is. `Identical` waits for a null test against FL. `python3 -I tools/progress.py --check` passes.
- Read the DSP from the binary rather than guessing. Where you approximate, say so in the row and in the plugin's `fidelity()`.
- The audio path allocates nothing. Size buffers in `set_state` and `set_sample_rate`. `process`, `tick`, `set_tempo` and `set_param` run on the audio thread (tempo automation calls `set_tempo` mid-song), so they must not allocate. `crates/plugins/tests/no_alloc.rs` checks every plugin in `port.rs` from its defaults (set `NO_ALLOC_TRACE=1` for where an allocation comes from), and `crates/engine/tests/no_alloc.rs` checks `Engine::process`.

- If your change shows in the app (a window, a widget, the colours), draw the README's screenshots again and commit them with it: `FL_RS_CORPUS="<FL Studio>/Data" cargo test -p ui --test screenshots -- --ignored`. Look at them first. `CLAUDE.md` has the rest.

## Report

End by delivering your report: under 300 words, saying what plays, what is approximated, corpus numbers before and after, every file you touched outside your own with the lines you changed there, and anything left open. A report that never arrives costs a round trip, so send it before you stop.
