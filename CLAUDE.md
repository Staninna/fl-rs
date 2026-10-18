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

## Progress table

The block between the progress markers in `README.md` is generated: `python3 -I tools/progress.py` writes it from `docs/plugins.md` and `docs/parity.md`, and CI reruns it on every push to `main`. Change the docs, then run the script. Run the corpus render through it, `cargo test --release -p engine --test corpus -- --nocapture 2>&1 | python3 -I tools/progress.py --render`, and it records the count of projects with sound in `docs/plugins.md`; commit that with the README.
