# Kepler: handover

Built on branch `claude/flrs-kepler-drumsynth`: `crates/plugins/src/kepler.rs`,
mapped in `docs/plugins/kepler.md` (port level Gaps). Both demo channels
that were silent (Edlan & Ella Noël - Song For You, Purps On The Beat -
Stroll Up) now sound.

## Left to do

- Hold (engine param 0x52): the voices should stay on after a note off
  until hold goes off. Read the note off path in `FUN_180019160` and the
  0x52 case of `FUN_180019a20`.
- Chorus noise (param 0x46, the state's second extra byte): the noise the
  engine adds in the chorus; not read yet (`FUN_180001a60`).
- The crossfade of the chorus when its mode changes (the frame
  `FUN_18001b940`, after the HPF).
- `FPE_MIDI_Vol` / `FPE_MIDI_Pan` through FL's wrapper (`FUN_1800184f0`
  and Gen_Render `FUN_180009740`): the engine's output gain 6.309 is kept.
- The song position for LFO and arpeggiator sync: the port counts beats
  from the first frame, FL hands the engine the song's ppq.
- The LFO1 rate and free arp rate tables' addresses are not written down
  (the values are in `kepler.rs`, read from the DLL).
- Automation smoothing of the parameters (`this+0x13d8`).

None of these is reached by the corpus states at their values, except the
sync position for Stroll Up's arp if it runs synced (it runs free).

## Tools

Ghidra projects `/root/ghidra/p_kepeng` and `p_kepler` on the machine this
ran on (rebuild with `analyzeHeadless ... -import`); `DecompileRange.java`
over 0x180001000..0x180200000 gives every function.
