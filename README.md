# fl-rs

A cross-platform DAW in Rust that opens FL Studio projects.

## Layout

- `crates/flp`: `.flp` and `.fst` files at the event level. Round-trips every shipped FL Studio project and preset byte for byte.
- `crates/model`: the project as Rust types (channels, patterns, arrangements, inserts), converted from and back to the event stream without losing an event.
- `crates/dsp`: gain, pan, biquad and interpolating sample readers.
- `crates/plugins`: FL Studio's stock plugins, rebuilt from `docs/plugins/`, behind an `Effect` trait and a `Generator` trait. Generators so far: 3x Osc, BassDrum, Fruit Kick, BooBass, Autogun, FPC, DirectWave, Slicex and FL Keys (the sample players read their samples from FL's own install or the project's files at runtime), and Sytrus, Harmless and Harmor (approximate; `docs/plugins.md` says where). Effects so far: Balance, Fast Dist, Soft Clipper, Filter, Fast LP, Delay 2, Delay 3, Stereo Enhancer, Compressor, Parametric EQ 2, Limiter, Reeverb 2, WaveShaper, Stereo Shaper, PanOMatic, Peak Controller, Maximus, Soundgoodizer, Love Philter, Convolver, Transient Processor, Multiband Compressor, Delay Bank, Flangus, Blood Overdrive, Chorus, Spreader, Effector, Gross Beat, Luxeverb, Pitcher, Vocodex, EQUO and Hyper Chorus. `docs/plugins.md` keeps the score. The engine runs them in the mixer's effect slots, and a Peak Controller's outputs drive whatever the project links them to.
- `crates/engine`: offline renderer and realtime playback, with plugin delay compensation and hosting for the stock generators and effects in `crates/plugins`, including sidechain inputs. Sequences the playlist and plays sampler channels and audio clips, to a buffer or through cpal to the default output device; every shipped project renders and 108 of 165 are audible with the stock plugins built so far. Automation clips drive channel and insert volume and pan, send levels and the tempo.
- `crates/ui`: the windows as egui panels. They draw a project and a transport position, and emit commands. So far: the top bar with transport, and a playlist with move, resize, mute, delete and duplicate edits under undo. The channel rack has LEDs, knobs, insert targets and the step sequencer with a velocity lane. The mixer has strips with faders, pans, meters, sends, routing and the slot panel. The piano roll draws, selects, moves, resizes, mutes and deletes notes with a lane for velocity, pan, fine pitch, release and mod X/Y. The browser is a tree of samples, projects and presets under FL's data directory and the project's folder: a double click plays a sample or opens a project, and a sample dragged onto the channel rack goes to a channel or makes a new one.
- `apps/fl-rs`: the desktop app. It opens a project on a worker thread, plays it through the engine, saves it back and renders it to WAV.
- `docs/parity.md`: how much of the `.flp` event dictionary is understood.
- `docs/plugins.md`: the 141 stock plugins, which have render tests, how many shipped projects use each, and the status of each. `docs/plugins/*.md` hold the per-plugin maps and `docs/plugins/host-voice.md` the host side of the voice path (pan law, pitch reference, velocity).
- `docs/automation-234.md`: automation clip points, curve modes, value mapping and how clips bind to their targets.
- `docs/records-224-233.md`: note and playlist item record layouts, read from the loader and the piano roll and playlist code.
- `docs/rec-225.md`: how mixer insert volume, pan, sends, EQ and slot state are encoded in the 225 initial-value records.
- `docs/event-dictionary.md`: every event ID with what FL Studio's loader does with it, read from the binary, merged with what PyFLP documents.
- `tools/`: the scripts behind the docs: the event census, the Ghidra scripts, and `plugstate/` for plugin state maps.
- `corpus/manifest.sha256`: pins the test corpus, which is FL Studio's own `Data/` directory and stays out of this repo.

## Tests

    cargo test --workspace

Set `FL_RS_CORPUS` to an FL Studio `Data/` directory to run the corpus round trip too.

On Linux the build needs the ALSA headers (`libasound2-dev` on Debian and Ubuntu). The tests that play through the output device skip themselves when there is none.

The `ui` snapshot test draws through wgpu and needs an adapter. Without a GPU, Mesa's lavapipe will do (`mesa-vulkan-drivers`). After a change that is meant to show, write the snapshot again with `UPDATE_SNAPSHOTS=true cargo test -p ui`.

## Running

    FL_RS_DATA="<FL Studio>/Data" cargo run -p fl-rs -- song.flp

A project named on the command line starts playing once it has loaded. `FL_RS_DATA` is where samples are looked up and where the browser starts, and without it samples are looked up beside the project.
