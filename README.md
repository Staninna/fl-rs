# fl-rs

A cross-platform DAW in Rust that opens FL Studio projects. It plays them by rebuilding FL's stock plugins from FL's own binaries, one plugin at a time.

## Progress

<!-- progress:start -->
| | done | of | |
|---|---:|---:|---|
| Stock generators built, of all FL ships | 14 | 51 | `█████░░░░░░░░░░░░░░░` 27% |
| Stock effects built, of all FL ships | 36 | 90 | `████████░░░░░░░░░░░░` 40% |
| Built plugins with nothing approximated | 31 | 50 | `████████████░░░░░░░░` 62% |
| Built plugins with their own doc | 46 | 50 | `██████████████████░░` 92% |
| Plugin uses in shipped projects that are built | 824 | 1006 | `████████████████░░░░` 81% |
| Shipped projects with any sound | 111 | 165 | `█████████████░░░░░░░` 67% |
| Shipped projects that play in full | 32 | 165 | `████░░░░░░░░░░░░░░░░` 19% |
| Effect slots hosted where sound reaches | 921 | 1021 | `██████████████████░░` 90% |
| Generator channels with notes that play | 173 | 215 | `████████████████░░░░` 80% |
| Shipped projects saved back byte for byte | 165 | 165 | `████████████████████` 100% |
| Event types in shipped projects with a reading | 146 | 147 | `████████████████████` 99% |
| Event readings FL's loader and PyFLP agree on | 98 | 147 | `█████████████░░░░░░░` 66% |

The plugin rows count against everything FL ships and everything built. The
rest count what FL's 165 shipped projects contain, so 100% there is not 100%
of FL:

- Nothing approximated: the plugin's row in `docs/plugins.md` names no
  approximation. It is read from the binary, not yet checked against FL's output.
- Plugin uses count once per project, and include third-party VSTs and
  visualisers that will never be built, so this row cannot reach 100%.
- A project has sound when anything comes out of it, even with half its plugins
  passed through. It plays in full when it has sound, every plugin is hosted,
  nothing is approximated, every sample is found and decoded, and every
  automation target and clip stretch is applied. Neither means it sounds like FL.
- The project, slot and channel counts come from the last corpus render, which
  needs FL's files, so CI cannot rerun it; `tools/progress.py --render` records
  them in `docs/plugins.md`.
- Saved back byte for byte: unknown bytes are carried through, not understood.
- Event types: the shipped projects use 147 of the 186 in
  `docs/event-dictionary.md`, which also lists what FL's loader and PyFLP know.
  5 readings conflict with PyFLP and 2 rest on PyFLP alone
  (`docs/parity.md`).

Built so far, most used in the shipped projects first. `docs/plugins.md` says
what is exact and what is approximated in each.

- Generators: 3x Osc, Sytrus (approximate), Harmless (approximate), FPC, Harmor (approximate), DirectWave, Slicex (approximate), Autogun (approximate), FL Keys, Fruity DX10, Ogun, BassDrum, BooBass, Fruit Kick.
- Effects: Fruity Parametric EQ 2, Fruity Limiter, Maximus, Fruity Reeverb 2, Fruity Delay 3, Fruity WaveShaper, Fruity Stereo Shaper, Fruity Peak Controller, Fruity Soft Clipper, Fruity PanOMatic, Fruity Flangus, Fruity Love Philter, Fruity Fast Dist, Pitcher, Fruity Delay 2, Gross Beat, Fruity Compressor, Transient Processor, Fruity Delay Bank, Effector, Fruity Balance, Fruity Multiband Compressor, Fruity Blood Overdrive, Soundgoodizer, Vocodex, Fruity Chorus, Fruity Convolver, Fruity Stereo Enhancer, Luxeverb, Spreader, EQUO, Hyper Chorus, Pitch Shifter, Fruity Filter, Frequency Splitter, Fruity Fast LP.

`tools/progress.py` writes this block from the docs.
<!-- progress:end -->

## Layout

- `crates/flp`: `.flp` and `.fst` files at the event level. Round-trips every shipped FL Studio project and preset byte for byte.
- `crates/model`: the project as Rust types (channels, patterns, arrangements, inserts), converted from and back to the event stream without losing an event.
- `crates/dsp`: gain, pan, biquad and interpolating sample readers.
- `crates/plugins`: FL Studio's stock plugins, rebuilt from `docs/plugins/`, behind an `Effect` trait and a `Generator` trait. [Progress](#progress) lists the ones built so far and `docs/plugins.md` keeps the score. The sample players read their samples from FL's own install or the project's files at runtime. The engine runs the effects in the mixer's slots, and a Peak Controller's outputs drive whatever the project links them to.
- `crates/engine`: offline renderer and realtime playback, with plugin delay compensation and hosting for the stock generators and effects in `crates/plugins`, including sidechain inputs. Sequences the playlist and plays sampler channels and audio clips, to a buffer or through cpal to the default output device; every shipped project renders, and [Progress](#progress) says how many have sound. Automation clips drive channel and insert volume and pan, send levels and the tempo.
- `crates/ui`: the windows as egui panels. They draw a project and a transport position, and emit commands. So far: the top bar with transport, and a playlist with move, resize, mute, delete and duplicate edits under undo. The channel rack has LEDs, knobs, insert targets and the step sequencer with a velocity lane. The mixer has strips with faders, pans, meters, sends, routing and the slot panel. The piano roll draws, selects, moves, resizes, mutes and deletes notes with a lane for velocity, pan, fine pitch, release and mod X/Y. The browser is a tree of samples, projects and presets under FL's data directory and the project's folder: a double click plays a sample or opens a project, and a sample dragged onto the channel rack goes to a channel or makes a new one.
- `apps/fl-rs`: the desktop app. It opens a project on a worker thread, plays it through the engine, saves it back and renders it to WAV.
- `docs/parity.md`: how much of the `.flp` event dictionary is understood.
- `docs/plugins.md`: the 141 stock plugins, which have render tests, how many shipped projects use each, and the status of each. `docs/plugins/*.md` hold the per-plugin maps and `docs/plugins/host-voice.md` the host side of the voice path (pan law, pitch reference, velocity).
- `docs/automation-234.md`: automation clip points, curve modes, value mapping and how clips bind to their targets.
- `docs/records-224-233.md`: note and playlist item record layouts, read from the loader and the piano roll and playlist code.
- `docs/rec-225.md`: how mixer insert volume, pan, sends, EQ and slot state are encoded in the 225 initial-value records.
- `docs/event-dictionary.md`: every event ID with what FL Studio's loader does with it, read from the binary, merged with what PyFLP documents.
- `tools/`: the scripts behind the docs: the event census, the Ghidra scripts, `plugstate/` for plugin state maps, `check-corpus.sh` and `progress.py`.
- `corpus/manifest.sha256`: pins the test corpus, which is FL Studio's own `Data/` directory and stays out of this repo.
- `.scratch/`: the working state between sessions: the handoff, the phase plans and the per-plugin handover notes. See [Continuing development](#continuing-development).

## FL Studio's files

This repo holds none of Image-Line's files. The corpus tests, the corpus render, the app's samples and all the reverse engineering read an FL Studio 26.1.7 install that you supply.

1. Get the FL Studio 26.1.7 installer from image-line.com. The work so far used `flstudio_win64_26.1.7.5654.exe`.
2. Install it. On Windows and macOS, run the installer. On Linux, run it under Wine:

       WINEPREFIX="$HOME/fl-studio" wine flstudio_win64_26.1.7.5654.exe

   FL then sits in `$HOME/fl-studio/drive_c/Program Files/Image-Line/FL Studio 2026`. The installer is NSIS in a form 7-Zip cannot unpack, so installing it is the only way in.
3. Check that its `Data/` directory is the one this repo is pinned to:

       tools/check-corpus.sh "<FL Studio>/Data"

   Another FL version ships other projects, and the counts the tests assert will not match.
4. Point the tests and the app at it:

       export FL_RS_CORPUS="<FL Studio>/Data"   # corpus tests and render
       export FL_RS_DATA="<FL Studio>/Data"     # the app's samples and browser

The plugin DLLs the docs were read from sit in the same install: `Plugins/Fruity/`, `Shared/` and `FLEngine_x64.dll`. `tools/README.md` has the Ghidra and state-dump scripts.

## Tests

    cargo test --workspace

Set `FL_RS_CORPUS` to run the corpus tests too. Without it they skip themselves.

On Linux the build needs the ALSA headers (`libasound2-dev` on Debian and Ubuntu). The tests that play through the output device skip themselves when there is none.

The `ui` snapshot test draws through wgpu and needs an adapter. Without a GPU, Mesa's lavapipe will do (`mesa-vulkan-drivers`). After a change that is meant to show, write the snapshot again with `UPDATE_SNAPSHOTS=true cargo test -p ui`.

The corpus render plays every shipped project offline and prints how many have sound, which generators stay silent and which effects are passed through. It takes about three minutes in release:

    cargo test --release -p engine --test corpus -- --nocapture

## Running

    FL_RS_DATA="<FL Studio>/Data" cargo run -p fl-rs -- song.flp

A project named on the command line starts playing once it has loaded. `FL_RS_DATA` is where samples are looked up and where the browser starts, and without it samples are looked up beside the project. Use `--release` for playback; the debug build is too slow for realtime audio. `demo_cmd.txt` lists demo projects that play well.

## Continuing development

Start with `.scratch/handoff.md`: what was done last, what is half done, and which plugins to build next by how many shipped projects need them. Then:

- `.scratch/plan-phase4.md` is the method for one plugin, from the DLL to Rust.
- `.scratch/agent-rules.md` is the brief each plugin agent gets: which files it owns in a shared tree, what "done" means for a plugin, and the rule that the audio path never allocates.
- `.scratch/notes/<plugin>.md` is the handover for a plugin left half done: DLL addresses, what is confirmed and what is guessed, next steps.
- `docs/plugins.md` is the scoreboard. A row says what is exact and what is approximated.

Adding a plugin, in short: map its state and DSP from the DLL into `docs/plugins/<name>.md`, write `crates/plugins/src/<name>.rs` behind `Effect` or `Generator`, register it in `crates/plugins/src/lib.rs` (and `generator.rs` for a generator), fill in its row in `docs/plugins.md`, then run the corpus render through `tools/progress.py` so its counts are recorded:

    cargo test --release -p engine --test corpus -- --nocapture 2>&1 | python3 -I tools/progress.py --render

That updates `docs/plugins.md` and the README; commit both.

Commit `.scratch/` with the code it describes, so the next clone starts where you stopped. Its paths name the machine the work started on; read them as your own FL install.

The progress block updates itself. On every push to `main` that touches `docs/plugins.md`, `docs/parity.md`, `docs/event-dictionary.md`, `tools/progress.py` or this README, a workflow runs `tools/progress.py` and commits the README if the block changed. Pull before your next push. The numbers CI cannot work out are the corpus render's (projects with sound and in full, effect slots hosted, generator channels playing), since the render needs FL's files; the `--render` command above records them.
