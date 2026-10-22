# tools

Scripts behind `docs/parity.md`. None of them run in CI.

- `play-demos.sh [Data dir]`: plays every project under `Demo projects` one after another, printing whether each is a demo song or another demo project. Closing the window starts the next. `--rate` asks how each one played through fzf and keeps the answers in `.scratch/demo-ratings.md`.
- `demo-render.sh [Data dir]`: renders the same projects to WAV through `fl-rs --render`, into `target/demo-renders/<commit>/`.
- `flp_census.py <Data dir>`: tallies event IDs across every `.flp` under a directory. Run with `python3 -I`.
- `ghidra/DumpFlEngine.java`: Ghidra headless script. Dumps all strings with referrers, all functions, and decompiles every function that references the given strings. First run imports and analyses the binary; later runs reuse the project with `-process <file> -noanalysis`.
- `ghidra/DecompileAddrs.java`: decompiles the functions at the given addresses, optionally plus their callees.
- `ghidra/DisasmAddrs.java`: prints instruction listings with constant-pool values, for the `tan`/`pow` arguments the decompiler drops.
- `ghidra/DecompileCallers.java`: decompiles the functions that call the given addresses (one level up), and writes the caller list to `callers.txt`. `listonly` skips the decompile.
- `ghidra/ConstRefs.java`: finds every initialised f32/f64 word equal to the given constants and decompiles the functions that reference them; the way to a field writer when only a magic number is known.

Both live in `/shared/code/fl-studio-target/ghidra/scripts/` next to the others.

Headless invocation, with `JAVA_HOME` pointing at a full JDK 21:

    analyzeHeadless <project dir> <project> -import FLEngine_x64.dll \
      -scriptPath tools/ghidra -postScript DumpFlEngine.java <out dir> FLdt,FLhd
