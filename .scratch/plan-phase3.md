# fl-rs phase 3: the UI

Phase 2 gave us a project model that round-trips every shipped `.flp`, and an engine that renders and plays sampler channels, audio clips, the playlist, the mixer and (step 5, in progress) automation, with realtime output through `engine::Transport`. Phase 3 puts FL Studio's windows on top of it.

## Scope

A desktop app, `fl-rs`, that opens an `.flp`, shows it the way FL does, plays it, lets you edit the parts the engine can play, and saves back without losing an event. Windows, in build order:

1. **Shell.** Main window with FL's top bar: transport (play, stop, pattern/song switch, tempo), position display, master peak meter, menu with open, save, save as, render to WAV. Window manager for the panels below, with FL's default layout and F-key toggles (F5 playlist, F6 channel rack, F7 piano roll, F8 browser, F9 mixer).
2. **Channel rack.** Channel list with name, mute, volume and pan knobs, insert target; the step sequencer grid for the selected pattern; pattern selector.
3. **Playlist.** Tracks with pattern and clip items at their positions, mute, trim handles, a playhead, drag to move and resize, zoom and scroll. Automation clips drawn as curves.
4. **Mixer.** Insert strips: name, fader, pan, mute, peak meters from the engine, send routing, slot list (plugins as named stubs until phase 4).
5. **Piano roll.** Notes for the selected channel and pattern, velocity lane, draw/select/move/resize with FL's tools, ghost notes from other channels.
6. **Browser.** The sample tree under the FL data directory and the project's folder, drag a sample onto a channel.

Out of scope: plugin windows (phase 4), the Patcher, Edison, Newtone, video, performance mode, themes beyond FL's default dark look.

## Stack

egui 0.36 with eframe and wgpu, the choice approved in the phase 0 plan. Dockable panels and the chrome come from storytold's `craft-ui` (MIT OR Apache-2.0), pinned by commit as a git dependency, with the DAW widgets (knob, fader, meter, step cell, note grid, timeline ruler) written here in a `ui-widgets` module. Fonts: egui's defaults plus one monospace for the hex-style displays FL uses. Saving uses `craft-atomic` for crash-safe writes.

## Crates

| crate | job |
| --- | --- |
| `ui` | egui panels and widgets, no engine ownership; takes `&model::Project` and a command sink |
| `app` | the binary: owns the `Arc<Project>` and the `Transport`, applies edit commands, undo stack, file dialogs through `rfd` |

Edits go through one `Command` enum applied on the app side, which produces the next `Arc<Project>` snapshot and hands it to the transport (`set_project`); that gives undo for free and keeps the UI stateless about the model.

## Order of work

Each step ends with the app running on Linux, macOS and Windows in CI (build only; screenshots by hand), and an MP4 recording of the flow for you.

1. Shell plus transport over a loaded project: open, play, stop, seek by clicking the position bar, render to WAV. (This alone makes fl-rs a player for every shipped project.)
2. Playlist, read-only first, then move/resize/mute edits.
3. Channel rack with the step sequencer; edits write back into the pattern's notes.
4. Mixer with live meters and fader/pan/send edits mapped to the 225 records.
5. Piano roll.
6. Browser and drag-and-drop.
7. Save and save-as with the round-trip guarantee: a `.flp` opened and saved unchanged is byte-identical.

## Validation

Visual parity with FL is by eye against screenshots of FL's own windows (I can take those from the FL manual's images since FL itself won't run here). Functional checks are egui_kittest snapshot tests for each panel against a fixture project, and the save round-trip test over the corpus.

## Sizing

Steps 1 and 2 are a week or so each. Piano roll and playlist editing interactions are where the time goes; the rest is mostly layout. Full FL look-alike polish is open-ended and gets its own pass after the windows work.

## Decisions for you

1. Theme: match FL's default dark theme closely (colours sampled from FL's artwork in `Artwork/`, which stays out of the repo), or start with egui's dark theme and restyle later. I recommend sampling the colours now, because layout decisions depend on the look.
2. Undo granularity: per command (every knob tick is an undo step) or FL's style (coalesced while dragging). I recommend coalesced.
