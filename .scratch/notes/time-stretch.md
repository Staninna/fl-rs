# Time stretch: what is left

The channel time stretch plays (`crates/engine/src/stretch.rs`,
`docs/rec-channel.md` "Time stretch"): FL's lengths, speeds and pitches for
every mode, read from `FUN_00d26890`, `FUN_00ff0640`, `FUN_00fea570`,
`FUN_00fea6e0` and `FUN_0067db00`/`FUN_0067db90` in `FLEngine_x64.dll`. The
stretch itself is a WSOLA standing in for élastique. Left, by how much it
would change what the corpus plays:

1. The stretch algorithm. FL calls `Shared/elastique_x64.dll` (zplane
   élastique: the e2 and e3 modes at load, Stretch and Stretch Pro as the
   voice plays) through `FUN_00a97410` (load time) and the object at
   `voice+0x2e8`, `+0xf8` (`FUN_0067db90` calls its VMT +8 with time ratio,
   pitch factor and formant factor). Not read; proprietary and deep. A
   better stand-in (a phase vocoder with transient handling) would get
   closer; nothing in the corpus can be null-tested without FL's output.
2. Slice stretch, slice map, and auto with `chan+0x370 == 2`: FL's own
   slicer (`FUN_00f36060`, `TDrumStretcher`), not read; stretched by the
   WSOLA. `chan+0x370` is set by an analysis of the sample (not found).
3. Stretch Pro's formant (215 @160): counted in `unplayed time stretch`.
   `FUN_0067db90` passes `pitch * 2^(1 - 2 formant)` as élastique's formant
   factor.
4. A clip or link on control 14 (the time), and the restretch FL queues
   when the channel pitch moves (`FUN_00ff86f0`). The engine stretches at
   the voice's start; a pitch clip or a slide then changes a stretched
   voice's speed, where élastique would shift its pitch.
5. An audio clip item's own stretch (`voice+0x39c`, multiplied into `M` in
   `FUN_00d26890`): `Rendered::unsupported_stretch`, 0 clips in the
   corpus's first 10 seconds.
6. The stretch is made in `Graph::new`, so every graph rebuild (an edit in
   the app) stretches again. The cost goes with the stretched length:
   about 0.4 s for a 192 s stereo sample at half its length and 1 s at
   twice (`stretch::tests::wsola_speed`, ignored, release, on a busy
   4-core box). The corpus has stretched samples that long (`USE ME vox
   25 2.wav` in P.Priime - Use Me, an octave down in Stretch). A cache in
   `PreparedSamples` that outlives the graph would make edits snappy.
7. The flag at `voice+0x224` & 0x100000 multiplies the Resample speed by
   the global at `0x15d8970` (1.0 at start); not read what sets either.
