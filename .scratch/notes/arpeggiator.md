# Arpeggiator: what is left

The channel arpeggiator plays (`crates/engine/src/arp.rs`, map in
`docs/arpeggiator.md`). Open:

- FL's chord table, behind `PTR_DAT_015e4f50` (pointer to a Delphi array
  of int arrays: intervals in semitones, length at `-8`). Nothing in the
  DLL writes it by that pointer, so it is filled at run time, probably
  from a chord or scale definition the piano roll's chord tools load
  (`FUN_00ded3b0` and `FUN_00df6e00`/`FUN_00df6f80` read it). Find its
  writer (a data-ref search on the variable the pointer names, not on the
  pointer), dump the entries, and `Settings::unbuilt` can drop chords above
  0. No corpus channel uses one.
- The global MT19937's state when a song starts. Seeded 0x12bd6aa in
  `FUN_011d3790` (project cleared); `FUN_00d18130`, layer random
  (`FUN_00d29240`) and three others draw from it too. No corpus channel
  plays random.
- The order of voices within a tick is FL's voice pool order
  (`FUN_00d2c560` allocates by index, `FUN_00d2c630` lists). The engine
  takes the order the notes come in the song. It decides which of the
  notes that start together is the master: its release velocity and colour
  go on every step note, and with slide or the time at 1447 its velocity
  and pan are what sound.
- Pending slide-note deltas that `FUN_00d26320` adds to a chord step's
  levels, and the channel's portamento on step notes.

How it was read: headless Ghidra on a copy of `ghidra/proj/flengine`,
`tools/ghidra/DecompileAddrs.java`, `DecompileCallers.java`,
`DataRefs.java`, plus a script that lists every instruction with a given
displacement (to find the readers of `chan+0x6e0` and the like).
