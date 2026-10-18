# fl-rs: a cross-platform FL Studio clone in Rust

## What you asked for

A Rust DAW that runs on Linux, macOS and Windows, is compatible with FL Studio, and has the full pro feature set. No order preference. REA plus the FL binary as the evidence source, storytold's Craft repos as reusable parts.

## What I have in hand

- FL Studio 26.1.7.5654 (Windows trial) installed under Wine at `/shared/code/fl-studio-target/winehome/`. 27,401 files. `FL64.exe` is a 583 KB stub; the 21 MB `FLEngine_x64.dll` is the real program. Shared DSP libs sit beside it (`ildsp_x64.dll`, `elastique_x64.dll`, `dsp_ippv2_x64.dll`, `Reverb_x64.dll`).
- A built-in test corpus: 165 `.flp` project files (82 of them in `Data/System/Render tests/`, one per stock plugin, plus 46 demo songs and templates) and 8,990 `.fst` preset files. Image-Line owns these, so they stay out of the repo; tests reference them by path and a sha256 manifest.
- REA 6.4.0 registered with Claude Code and driven by Ghidra 12.1.2. A Ghidra pass over `FLEngine_x64.dll` is running now, with a snapshot at `/shared/code/fl-studio-target/rea/`.
- storytold: everything is `MIT OR Apache-2.0`, egui 0.36 + eframe + wgpu, cpal 0.16. `soundcraft` (their Pro Tools clone, 6 days old) has a callback-owned mix engine, session snapshots via `Arc`, a piano roll, mixer strips, and CLAP/VST3 host crates. `craft-libs` has atomic file writes and a settings crate. None of it is on crates.io; all git deps. They have no proprietary binary project format cracked yet (`.ptx`, `.prproj`, `.aep` are all at 0%), so their method helps for testing and structure, not for the `.flp` work.

## Ground rules

- Interoperability work only. I read the binary to learn the file format, the parameter semantics and the DSP behaviour, and reimplement in Rust. No FL code, assets, presets or samples get copied into the repo, and nothing touches FL's licensing or trial limits. "All pro features" means we rebuild them, with the stock plugins as native Rust code.
- Licence `MIT OR Apache-2.0`, so the storytold crates and the usual Rust audio crates (cpal, symphonia, rubato, clack, vst3-sys) all fit.
- PyFLP is GPL-3. I use its docs as a map of known event IDs and read its code only to confirm a reading of the bytes; no code crosses over.
- Every claim about FL behaviour gets a source: an REA evidence ID, a corpus file and offset, or an FL-rendered WAV.

## Shape of the project

One cargo workspace:

| crate | job |
| --- | --- |
| `flp` | `.flp` and `.fst` read and write, event level, byte-exact round trip |
| `model` | the project as Rust types: channels, patterns, playlist, mixer, automation |
| `engine` | realtime audio: cpal output, callback-owned graph, lock-free UI link (soundcraft pattern) |
| `dsp` | oscillators, filters, envelopes, resampling, the shared building blocks |
| `plugins` | stock FL generators and effects, one module each, same parameter IDs as FL |
| `host` | CLAP and VST3 hosting (fork of soundcraft's crates if they hold up) |
| `ui` | egui/eframe/wgpu, FL's window set: browser, channel rack, piano roll, playlist, mixer |
| `app` | wires it up |

## Phases

Each phase ends in a demo you can run and a parity table update. Phases 0 and 1 are detailed here; later phases get their own plan when they start.

### 0. Scaffold

Workspace, CI matrix on Linux, macOS and Windows, corpus manifest, `docs/parity.md` with the `.flp` event table at 0%.

### 1. `.flp` in and out

Goal: read every one of the 165 corpus files into typed events and write them back byte for byte. Known event IDs from PyFLP's docs; unknown ones recovered from `FLEngine_x64.dll` with REA (string tables and the event dispatch switch). Fuzz the parser. Output of this phase is the complete event dictionary for FL 26, which no public source has.

### 2. Model and engine

Load a project into `model`, play it: channel rack with sampler channels, patterns, playlist, tempo, mixer tracks with inserts, sends and routing, automation clips. Offline render to WAV first, realtime output second. Validate against FL exports of the demo songs with stock sampler channels only.

### 3. UI

egui, FL's layout and keyboard model. Channel rack, piano roll, playlist, mixer, browser. Step sequencer first, piano roll second.

### 4. Stock plugins

Ordered by the render-test corpus. For each plugin: REA on its DLL for the parameter map and signal chain, reimplement in `plugins`, compare our render of its render-test `.flp` against FL's WAV export (null test with a tolerance per plugin). Start with 3x Osc, Sampler, Fruity Filter, Parametric EQ 2, Fruity Reeverb 2, Fruity Delay. Harmor, Sytrus and the other big synths come last; each is a project on its own.

### 5. Third-party plugins

CLAP and VST3 hosting, bridged in-process, plugin state saved inside `.flp` as FL does.

### 6. The long tail

Patcher, Edison, Slicex, Newtone, ZGameEditor, video, FL Studio Mobile, Performance mode, everything else the parity table still lists.

## Honest sizing

Phases 0 to 2 are a few weeks of focused work. Phase 4 alone is dozens of plugins, each one its own reverse-engineering job. Full parity with a 28-year-old product is a multi-year project. The parity table keeps score so the gap is always visible.

## Decisions I need from you

1. egui for the UI, taking soundcraft's widgets as the starting point. The alternative is building a custom wgpu UI for FL's dense look, which is slower to start. I recommend egui.
2. VST2 hosting: the SDK is withdrawn and the licence forbids new products, so I skip it unless you say otherwise. Many old FL projects use VST2 plugins, so this is a real compatibility hole.
3. The Craft crates as git dependencies pinned by commit, with a vendored fork if they churn. Fine with you?
