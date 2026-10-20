# Edison and ZGameEditor Visualizer: why they are not pass-throughs

Both were checked for "the output is the input" with the other visual and
utility effects (Wave Candy, the notebooks, dB Meter, Razer Chroma, Control
Surface, X-Y Controller, hosted as `PassThrough`). Neither is: each can put
its own sound into the output, so each needs its state read before it can
be hosted. Both stay unhosted (passed through and counted as such).

Listings were read with capstone from the DLLs in `Plugins/Fruity/Effects/`
(FL 26.1.7, Delphi, image base 0x400000). The scripts (`plug2.py` finds the
plugin VMT from `CreatePlugInstance`, `info2.py` the packed
`TFruityPlugInfo`, `ripref.py` the writers of a function pointer) were in a
session scratchpad and are short to write again; `docs/plugins/wave-candy.md`
shows the method.

## Edison (`Edison_x64.dll`, 4 projects)

- `CreatePlugInstance` 0x87e880, class ref 0x87e678, VMT 0x87e740:
  Dispatcher 0x87e8e0, `Eff_Render` 0x882bb0. Info 0x902804: Flags
  0x32c08000 (`DemoVersion`, `Reserved1`, `CantSmartDisable`, bits 28, 29),
  3 parameters. No `NoProcess`.
- `Eff_Render` 0x882bb0, confirmed in the listing:
  - while recording (`this+0x410`) it appends the input to its sample
    (`this+0x3e0`), with record modes at `this+0x414` (bitmask 6 at
    0x8839d0 picks the threshold modes, peak via 0x636b00 against
    `+0x6f8+0x1204`), a "Song jump" marker at 0x8839e0, and a sample length
    limit at `this+0x40c`;
  - `this+0x41a` set: the input is copied away (`[0x9052e0]`) and the output
    zeroed (`[0x903cf0]`): Edison mutes its input;
  - otherwise `[0x9052e0]` copies Source to Dest (an IPP copy, not checked
    by name);
  - `this+0x3d8` set: a stream (0x621090) is mixed into Dest with
    `[0x904b50]` (an IPP add, not checked by name);
  - from 0x88308e, when `this+0x215` / `this+0x1a0` / `this+0x424` allow,
    a long loop renders the loaded sample (`this+0x168`, `this+0x180`
    position) into Dest: Edison's own playback.
- Next: map the state (SaveRestoreState, VMT slot 3) for the input-mute
  flag, the playback flags and whether a project can start with Edison
  playing; if the corpus states have all of them off, Edison is a
  pass-through there (and recording does not change the output).

## ZGameEditor Visualizer (`ZGameEditor Visualizer_x64.dll`, 9 projects)

- `CreatePlugInstance` 0x9b2b70 calls the constructor 0x9215d0 with the class
  ref at 0x916c70, VMT 0x916d38: Dispatcher 0x921a90, ProcessParam 0x922b20,
  `Eff_Render` 0x9232e0. Info 0xa3bf28: Flags 0x400020 (`WantNewTick`,
  `Reserved1`), 5072 parameters, 1024 output controllers (links can come
  from it; none in the corpus was seen with the controller-link listing,
  which found sources only in Control Surface, X-Y Controller, Peak
  Controller and Formula Controller).
- `Eff_Render` 0x9232e0, confirmed: first `ippsCopy_64s(Source, Dest, Length)`
  (`[0xa47a98]` -> 0xa54eb8, bound by name at 0x5c8672). Then, depending on
  flags of its scene (`this+0xc4` -> `+0x4779`, `+0x4750+0x1e0`, `+0x1383`,
  `+0x476c`, `+0x1380`): 0x9947d0(Source, Dest, Length) and
  0x90c640(..., Dest, ...), not read; then its own buffer at `this+0x134c`
  is zeroed (0x40ced0), filled by 0x7a1670, and handed with Dest and two
  gains of 1.0 to `[0xa479c0]` -> 0xa56618 (in `.bss`, bound at run time,
  setter not found). If that is an add, ZGE adds its own audio.
- Next: read 0x7a1670, 0x9947d0 and 0x90c640, and find who binds 0xa56618
  (`ripref.py` found no `lea` to it: look for a table of IPP names). If they
  only read Dest, or only write when a scene component the corpus does not
  use is on, ZGE is a pass-through.

## Also found on the way

- The 2 "Fruity Wrapper generator" uses in `docs/plugins.md` are the same two
  ZGE demo songs; every corpus Fruity Wrapper is a mixer slot
  (`docs/plugins/fruity-wrapper.md`).
