# fl-rs

A Rust DAW that plays FL Studio projects by rebuilding FL's stock plugins from their binaries. `README.md` has the layout, the tests and how to get FL's files.

## Working state lives in git

`.scratch/` is the project's memory between sessions and machines, and it is tracked:

- `handoff.md`: where things stand, what is in progress, what comes next. Read it before you start.
- `agent-rules.md`: the brief every plugin subagent gets. Point new agents at it.
- `plan-phase*.md`: the method per phase; `plan-phase4.md` is the current one.
- `notes/<plugin>.md`: the handover for a plugin left half done. Read it before touching that plugin.

Keep it current. When you stop, or hand work to another agent, rewrite `handoff.md` and the notes to match the tree, and commit them with the code they describe. A handover that is not committed is lost to the next clone.

The paths in `.scratch/` name the machine the work started on (`/shared/code/fl-studio-target/...`). On another machine, read them as "your FL Studio install".

## Others work on GitHub too

The repo is public at `Staninna/fl-rs`, so `.scratch/` shows only this tree's work; someone else may already have the plugin or bug you are about to pick up. Before you start a piece of work, and before you hand one to a subagent, search GitHub for it by plugin or subject:

- `gh issue list --state all --search "<name>"`
- `gh pr list --state all --search "<name>"`
- `git fetch origin` and `git branch -r` for branches named after it

When you find work in flight, build on it or pick something else, and tell the user which and why. Issues, comments and pull requests are public, so ask the user before you open or answer one. Before you push, `git fetch` and look at what landed on `main` since you started.

## Progress table

The block between the progress markers in `README.md` is generated: `python3 -I tools/progress.py` writes it from `docs/plugins.md`, `docs/parity.md` and the port levels in `crates/plugins/src/port.rs`, and CI reruns it on every push to `main`. A plugin is built when it has a port there; CI fails when `port.rs` and the `docs/plugins.md` rows disagree. Change the docs, then run the script. Run the corpus render through it, `cargo test --release -p engine --test corpus -- --nocapture 2>&1 | python3 -I tools/progress.py --render`, and it records the render's counts in `docs/plugins.md`; commit that with the README.

## Screenshots

`docs/screenshots/*.png` are the README's pictures of the app. A change to how anything looks (a window, the top bar, the colours, a widget, the start screen, a new window) is not finished until they are drawn again and committed with it:

    FL_RS_CORPUS="<FL Studio>/Data" cargo test -p ui --test screenshots -- --ignored

Look at each picture before you commit it. A new window or screen gets a `shot(...)` line in `crates/ui/tests/screenshots.rs` and a cell in the README's Screenshots table. Write the `ui` snapshots again too (`UPDATE_SNAPSHOTS=true cargo test -p ui`).
