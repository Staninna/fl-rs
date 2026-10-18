# fl-rs phase 2: model and engine

Phase 1 delivered `crates/flp`: every shipped project and preset round-trips, 146/147 event IDs have a reading, 21 payload structs decode exactly. Phase 2 turns that event stream into a project you can play.

## What phase 2 must do

Open any of the 165 shipped projects and render it to WAV with FL's own sampler-based parts audible and in time: tempo, patterns, the playlist, audio clips, channel volume/pan/pitch, mixer routing with insert volume and sends, and automation clips driving those. Realtime playback through the sound card follows the same path.

Out of scope here: every generator and effect plugin (phase 4), the UI (phase 3), third-party plugins (phase 5). A plugin channel or slot loads as a named stub that is silent or passes audio through, with its state bytes kept so the project saves back intact.

## Crates

| crate | job |
| --- | --- |
| `model` | `Project` as Rust types: `Channel` (sampler, layer, audio clip, automation, plugin stub), `Pattern` with notes per channel, `Arrangement` with tracks and playlist items, `Insert` with slots, routes and sends, `AutomationClip` with points. `from_flp(&flp::Project)` and `to_flp(&self)`. |
| `engine` | Offline renderer and realtime engine over the same graph: tick clock from tempo and PPQ, note scheduler, sampler voices with pitch shift by resampling, insert graph with gain, pan, sends, master. `render(&model::Project, sample_rate) -> Vec<f32>` and a cpal-driven `Transport`. |
| `dsp` | The pieces the engine needs: linear and windowed-sinc resampling (`rubato` for the sinc path), pan law, dB and gain conversion, a biquad for the sampler's filter. |

Audio file decoding goes through `symphonia` (WAV, FLAC, MP3, OGG). The thread model is soundcraft's: the cpal callback owns the graph, the UI side swaps in an `Arc<Project>` snapshot and sends commands over a channel, meters come back through atomics. No locks in the callback.

## Order of work

1. **model from events.** Build `model::Project::from_flp` for the delimiters and fields the dictionary already names: 64 opens a channel, 65 selects a pattern, 147 closes an insert, 99/238 arrangement and track, 233 playlist, 224 notes, 234 automation. Test: every corpus project loads without error, and `to_flp` of `from_flp` reproduces the original event stream for every project. Events the model doesn't understand ride along as raw events in order, so this round trip holds from day one.
2. **Offline render, sampler channels only.** Tempo, one pattern on the playlist, sampler channels playing their `.wav` with volume, pan and pitch. Test with synthetic fixtures: a generated `.flp` (we can write them now) plus a generated click sample, rendered, with note onsets checked to the sample against the tempo maths. Sample paths resolve against FL's `Data/Patches` for shipped projects and relative to the project otherwise.
3. **Mixer.** Inserts with volume, pan, routes and sends from 236 and 165/166, master. Test: a sampler routed through an insert at -6 dB lands at half amplitude.
4. **Playlist and audio clips.** Multiple patterns at positions, muted items, audio clip channels with start offset and stretch off. Test: a two-pattern playlist with a gap renders silence in the gap.
5. **Automation.** 234 clips as breakpoint curves applied to channel volume, pan, and insert volume at tick resolution.
6. **Realtime.** cpal output with the callback-owned engine, transport start/stop/seek, position and peak meters through atomics. Checked on Linux by ear and by a loopback test that records the output.

Each step ends with `cargo test` green and the corpus round trip still exact.

## Validation against FL

The render tests ship no reference audio. Two branches:

- If FL 26 runs under Wine, use its command-line render (`FL64.exe /R /E<out> <project>`) to produce references for the 82 render-test projects and the demo songs, store them outside the repo under `fl-studio-target/renders/`, and compare with a per-step tolerance (onset within 1 sample, RMS within 0.5 dB, then tighten). I am trying this now.
- If it does not, phase 2 validates against synthetic fixtures with known answers, and FL references wait for a Windows box. Phase 4 needs them anyway, so this gets resolved before plugins start.

## Dependencies

`symphonia 0.6`, `rubato 5`, `cpal 0.18`, `crossbeam-channel` for commands, `hound` for WAV output in tests. Craft crates are not needed in phase 2; `craft-atomic` comes in with the first save path in phase 3.

## Sizing

Steps 1 through 5 are a few days of focused work each; step 6 a day. The model round trip in step 1 is the long pole, because it has to carry 190 event kinds without losing one.

## Decision for you

Sample-accurate parity with FL's sampler (its pitch interpolation, its pan law, its envelope defaults) comes from reading `FLEngine_x64.dll`'s sampler code, the same way the dictionary was built. I plan to do that during step 2 rather than guess. It costs a Ghidra pass per question. Say so if you'd rather have an approximate sampler first and parity later.
