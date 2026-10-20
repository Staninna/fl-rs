# Edison and ZGameEditor Visualizer: hosted where they pass the input

Both are now hosted as pass-throughs for the states proven to leave the sound
alone, and turn every other state down as `StateError::Unhosted` (the host
passes those through and counts them). Maps: `docs/plugins/edison.md`,
`docs/plugins/zgameeditor-visualizer.md`; code `crates/plugins/src/edison.rs`,
`crates/plugins/src/zgameeditor.rs`.

## Left open

- Edison with audio in its sample: the second Edison of Asher Postman -
  Future House holds an 11758-frame sample at 48 kHz, with `this+0x420` at 3
  (MIDI notes can start the playback) in a project with a MIDI Out channel.
  To host it, find whether a MIDI Out note reaches it (the effect's MIDI
  input port is in its wrapper settings, not mapped yet; dispatch 0x15 sets
  `this+0x428`), then rebuild the playback: `0x622220` renders the play
  region at `this+0x180` with the speed glide `0x6114a0` and the gain
  matrix `0x622ba0`, added by `0x636100`.
- Edison muting its input (`this+0x41a`): none in the corpus. Silence plus
  the playback.
- ZGameEditor: an effect outside the 31 in `zgameeditor::SILENT` needs its
  `.zgeproj` read (no `AudioOutputModel`/`RenderTarget`/`Time`/`SampleRate`
  quartet, no `Sound`, `Music`, `MusicControl`, `Sample` or `Channel`
  component, no video transport) before it is added. Misc\AudioGenerator
  makes sound through the audio path at `0x9947d0`; video clips through
  `0x8a6230`; sound components through the mixer at `0x7a1670`.
- ZGameEditor's 1024 controller outputs are not driven (no corpus link).

Ghidra (cloud box, not in the repo): `/root/ghidra/ed/proj`,
`/root/ghidra/zge/proj`. Scripts used: `/root/work/dis.py` (capstone with
RIP targets and strings), `/root/work/scanoff.py` (writers of `[reg+off]`),
`tools/plugstate/ripscan.py` and `vmt.py` with their path fixed.
