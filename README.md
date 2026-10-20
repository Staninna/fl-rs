# fl-rs

> This project is built and run entirely by vibe coding: the code, the docs, the commits and the reverse engineering all come out of an AI agent's sessions. If you don't like that, you know where the back button is.

A cross-platform DAW in Rust that opens FL Studio projects. It plays them by rebuilding FL's stock plugins from FL's own binaries, one plugin at a time.

## Progress

<!-- progress:start -->
| FL's own plugins | done | of | |
|---|---:|---:|---|
| Instruments rebuilt | 15 | 51 | `██████░░░░░░░░░░░░░░` 29% |
| Effects rebuilt | 40 | 90 | `█████████░░░░░░░░░░░` 44% |
| Rebuilt and proven to sound identical to FL | 0 | 55 | `░░░░░░░░░░░░░░░░░░░░` 0% |
| Rebuilt with nothing known to be missing | 23 | 55 | `████████░░░░░░░░░░░░` 41% |
| Rebuilt and documented | 55 | 55 | `████████████████████` 100% |

Rebuilt means read out of FL's plugin binary and written again in Rust. No
plugin is proven identical yet: that needs a null test against FL's own
render. The levels are explained under the plugin list below.

| The 165 projects that come with FL | done | of | |
|---|---:|---:|---|
| Projects that make any sound | 112 | 165 | `██████████████░░░░░░` 67% |
| Projects that play with nothing missing | 35 | 165 | `████░░░░░░░░░░░░░░░░` 21% |
| Plugins they load that are rebuilt | 830 | 1006 | `█████████████████░░░` 82% |
| Instrument channels with notes that play | 180 | 215 | `█████████████████░░░` 83% |
| Mixer effects that run, on tracks audio reaches | 935 | 1028 | `██████████████████░░` 90% |
| Projects saved back unchanged, byte for byte | 165 | 165 | `████████████████████` 100% |

These count only what FL's own projects contain, so 100% here is not 100%
of FL.

- Any sound means something comes out, even with half the plugins skipped.
  Nothing missing means every plugin runs, none is approximate, every sample
  loads, and all automation and clip stretching is applied. Neither means it
  sounds like FL yet.
- Plugins count once per project, and include third-party VSTs and visualisers
  that will never be rebuilt, so that row stays under 100%.
- Saving back byte for byte carries unknown bytes through without understanding
  them.
- The sound, channel and mixer counts come from the last full render of the
  projects. That needs FL's files, so CI can't rerun it;
  `tools/progress.py --render` records it in `docs/plugins.md`.

| The `.flp` project format | done | of | |
|---|---:|---:|---|
| Kinds of record in FL's projects we can read | 146 | 147 | `████████████████████` 99% |
| Kinds FL's loader and PyFLP both read the same way | 98 | 147 | `█████████████░░░░░░░` 66% |

FL's projects use 147 of the 186 record kinds in
`docs/event-dictionary.md`. Of the readings not on both, 41 come from FL's
loader alone, 2 from PyFLP alone, and 5 disagree with PyFLP
(`docs/parity.md`).

Every plugin by how close the rebuild is to FL's, the ones FL's projects use
most first:

- **Identical** (0), a null test against FL's own render matches sample for sample: none yet.
- **Exact** (23), every part read from the binary and rebuilt, nothing known to be missing: Fruity Limiter, Fruity Reeverb 2, Fruity Delay 3, Fruity Stereo Shaper, Fruity Soft Clipper, Fruity Flangus, Fruity Fast Dist, Fruity Delay 2, Fruity Compressor, Transient Processor, Fruity Balance, Fruity Multiband Compressor, Fruity Blood Overdrive, Fruity Chorus, Fruity Stereo Enhancer, Spreader, Fruity Filter, BassDrum, BooBass, Fruity Fast LP, Fruity Delay, Fruity Reeverb, Fruity Squeeze.
- **Exact with gaps** (26), exact but for the parts its row in `docs/plugins.md` names as approximate or left out: Fruity Parametric EQ 2, Maximus, 3x Osc, Fruity WaveShaper, Fruity Peak Controller, Fruity PanOMatic, FPC, Fruity Love Philter, DirectWave, Pitcher, Gross Beat, Fruity Delay Bank, Effector, Soundgoodizer, Vocodex, Fruity Convolver, Luxeverb, FL Keys, EQUO, Hyper Chorus, Pitch Shifter, Fruity DX10, Frequency Splitter, Fruit Kick, Drumpad, Fruity Send.
- **Approximate** (6), the sound itself is an approximation: Sytrus, Harmless, Harmor, Slicex, Autogun, Ogun.
- **Not rebuilt** (49), with how many of FL's projects use each: Control Surface (17), FLEX (15), Patcher effect (12), Patcher generator (12), ZGameEditor Visualizer (9), Wave Candy (7), Fruity NoteBook 2 (6), Emphasis (5), Vintage Chorus (5), Fruity Slicer (5), GMS (5), Edison (4), Frequency Shifter (4), Fruity Flanger (4), Morphine (4), Distructor (3), Fruity Parametric EQ (3), Fruity Phaser (3), Fruity X-Y Controller (3), Low Lifter (3), Fruity Envelope Controller (3), Kepler (3), MIDI Out (3), Sakura (3), Fruity Free Filter (2), Fruity NoteBook (2), Fruity Wrapper effect (2), Hardcore (2), Transporter (2), Fruity Granulizer (2), Fruity Wrapper generator (2), Minisynth (2), Toxic Biohazard (2), Wave Traveller (2), Emphasizer (1), Fruity Bass Boost (1), Fruity Formula Controller (1), Fruity HTML NoteBook (1), Fruity Scratcher (1), Fruity Vocoder (1), Fruity dB Meter (1), Razer Chroma (1), Vintage Phaser (1), BeepMap (1), Drumaxx (1), Fruity DrumSynth Live (1), Fruity Keyboard Controller (1), Fruity Voltage Controller (1), PoiZone (1). 37 more no project uses.

The levels live in `crates/plugins/src/port.rs`, and CI fails when it and
`docs/plugins.md` disagree on what is rebuilt.

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

The profile harness times every shipped project the way the app plays it: 20 s from the start and 10 s from a quarter, a half and three quarters in, in calls of 512 frames. Per project it prints how many times faster than realtime it plays, its 99th percentile and slowest call against the 10.7 ms a call may take, how many calls ran over, and a hash of everything it put out:

    FL_RS_CORPUS="<FL Studio>/Data" taskset -c 2 cargo run --release -p engine --example profile

`--frames 1024` sets the call size, `--whole` plays each song once from start to end, and a word keeps the projects whose path contains it. A change meant to keep the sound must keep every hash. Compare hashes taken at the same call size on the same machine: some plugins work once per block, so a different call size sounds a little different, and each OS's maths library rounds `sin` and `pow` its own way. The numbers move with a laptop's clock, so compare medians of three runs pinned to one core.

## Running

    FL_RS_DATA="<FL Studio>/Data" cargo run -p fl-rs -- song.flp

A project named on the command line starts playing once it has loaded. `FL_RS_DATA` is where samples are looked up and where the browser starts, and without it samples are looked up beside the project. The debug build optimises the audio crates, so it plays as fast as `--release`. `demo_cmd.txt` lists demo projects that play well.

## Continuing development

Start with `.scratch/handoff.md`: what was done last, what is half done, and which plugins to build next by how many shipped projects need them. It only knows about work committed here, so before you pick something up, search the [issues](https://github.com/Staninna/fl-rs/issues?q=is%3Aissue), [pull requests](https://github.com/Staninna/fl-rs/pulls?q=is%3Apr) and branches for it, to avoid doing the same work twice. Then:

- `.scratch/plan-phase4.md` is the method for one plugin, from the DLL to Rust.
- `.scratch/agent-rules.md` is the brief each plugin agent gets: which files it owns in a shared tree, what "done" means for a plugin, and the rule that the audio path never allocates.
- `.scratch/notes/<plugin>.md` is the handover for a plugin left half done: DLL addresses, what is confirmed and what is guessed, next steps.
- `docs/plugins.md` is the scoreboard. A row says what is exact and what is approximated.

Adding a plugin, in short: map its state and DSP from the DLL into `docs/plugins/<name>.md`, write `crates/plugins/src/<name>.rs` behind `Effect` or `Generator`, register it in `crates/plugins/src/lib.rs` (and `generator.rs` for a generator), give it a port level in `crates/plugins/src/port.rs`, fill in its row in `docs/plugins.md`, then run the corpus render through `tools/progress.py` so its counts are recorded:

    cargo test --release -p engine --test corpus -- --nocapture 2>&1 | python3 -I tools/progress.py --render

That updates `docs/plugins.md` and the README; commit both.

Commit `.scratch/` with the code it describes, so the next clone starts where you stopped. Its paths name the machine the work started on; read them as your own FL install.

The progress block updates itself. On every push to `main` that touches `docs/plugins.md`, `docs/parity.md`, `docs/event-dictionary.md`, `tools/progress.py` or this README, a workflow runs `tools/progress.py` and commits the README if the block changed. Pull before your next push. The numbers CI cannot work out are the corpus render's (projects with sound and in full, effect slots hosted, generator channels playing), since the render needs FL's files; the `--render` command above records them.
