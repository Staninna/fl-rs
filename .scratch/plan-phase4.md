# fl-rs phase 4: stock plugins

Phases 1 and 2 play every shipped project's sampler, clip, mixer and automation parts. Phase 4 makes the rest audible: the generators and effects FL Studio ships, rebuilt in Rust from their binaries, so that a project sounds like FL without FL.

## Inventory

`docs/plugins.md` lists all 141 stock plugins (51 generators, 90 effects) with their x64 DLL size, whether a render-test project ships for them (78 do), and how many of the 165 shipped projects use them. The top of that table is the order of work: Parametric EQ 2 (54 projects), Limiter (52), Maximus (40), 3x Osc (39), Reeverb 2 (38), Delay 3 (31), WaveShaper (29), Stereo Shaper (27), Peak Controller (26), Soft Clipper (23).

Each plugin stores its state in event 213 bytes. The corpus shows the small effects have a fixed-size state that is just their parameters: Fruity Balance 8 bytes, Soundgoodizer 12, Fast Dist 20, Filter 28, Delay 2 32, Compressor 32/36, Reeverb 2 58/66, Delay 3 108, Limiter 168/169, Parametric EQ 2 309/350/354. 3x Osc is 92 or 460, Sytrus 1.1 to 1.5 KB, FLEX 2.7 KB, Harmor 84 KB.

## Method per plugin

1. **Parameter map.** From the 213 bytes: size variants by version, field offsets, ranges and defaults. Source is the plugin DLL's state loader and the REC control dispatch read through REA/Ghidra, cross-checked against corpus values and the preset files (`Data/Patches/Plugin presets/<plugin>/*.fst`, thousands of them). Written to `docs/plugins/<name>.md` in the same style as `docs/rec-225.md`.
2. **Signal chain.** The DLL's process function, decompiled, read for topology and the maths of each block (filter type, saturation curve, delay line, envelope). Written into the same doc with the function addresses.
3. **Rust implementation** in `crates/plugins/<name>.rs` behind one trait, `Plugin { fn set_state(&mut self, bytes); fn process(&mut self, block) }` for effects and the generator equivalent with note on/off. Allocation-free `process`, built on `crates/dsp`, with new building blocks going into `dsp` when two plugins share them.
4. **Validation.** Three layers, in order of strength. First, unit tests from the maths (a filter at cutoff f passes f/10 and attenuates 10f). Second, the render-test project for that plugin rendered through our engine, inspected for sanity (no NaN, level in range, the expected spectral shape). Third, when a Windows box with FL is available, null tests against FL's own export of the render-test project, with a per-plugin tolerance recorded in `docs/plugins.md`. Layer three is the only real parity proof and is the biggest open item of the whole project.

`docs/plugins.md` gets a status per plugin: mapped, implemented, sanity-checked, nulled.

## Order

- **Batch 1, small effects, one each:** Balance, Soundgoodizer (a Maximus preset, so really a Maximus prerequisite study), Fast Dist, Soft Clipper, Filter, Fast LP, Delay 2, Phase Inverter, Stereo Enhancer, Blood Overdrive, Fruity Send, Mute 2. These settle the plugin trait, the slot mix and enable from the 225 records, and plugin-parameter automation (the 356 unsupported targets from phase 2).
- **Batch 2, the common ones:** Parametric EQ 2, Limiter, Reeverb 2, Delay 3, Compressor, WaveShaper, Stereo Shaper, Peak Controller, PanOMatic, Multiband Compressor, Flanger, Phaser, Chorus, Vocoder, Delay Bank, Love Philter, Convolver, Transient Processor, Spreader, Pitcher, Pitch Shifter.
- **Batch 3, generators by size:** 3x Osc, Fruit Kick, BassDrum, BooBass, Plucked!, Minisynth, DX10, DrumSynth Live, Wasp, Wasp XT, SimSynth, GMS, PoiZone, Sawer, Sakura, Morphine, Toxic Biohazard, Transistor Bass, Kepler, Ogun.
- **Batch 4, sample-based:** FPC, DirectWave, Slicex, Fruity Slicer, Soundfont Player, Drumaxx, Drumpad, Granulizer, FLEX.
- **Batch 5, the big ones:** Maximus, Harmless, Harmor, Sytrus, Gross Beat, Vocodex, Newtone, Newtime, Edison, Patcher, EQUO, Effector, Luxeverb. Each gets its own plan.
- **Deferred:** visual and controller plugins (ZGameEditor Visualizer, Wave Candy, Razer Chroma, Video Player, Dance, the VFX family, X-Y controllers, Keyboard/Envelope/Voltage Controllers, MIDI Out, ReWired, Dashboard). They do not make sound and come after the UI can host plugin windows.

## Sizing

Batch 1 is about two weeks. Batch 2 a month or two. Each big-five synth is one to several months of careful work. The whole phase is the long tail of the project and is where "full compatibility" either happens or does not. The status table keeps it honest.

## Decision for you

Without FL running here, layer-three validation (null tests) cannot start. Options: (a) you run FL on a Windows machine when asked and export the 78 render tests plus a set of demo songs to WAV once, which I then keep outside the repo as the reference set; (b) we try FL under a VM with a Windows guest on this box (needs a Windows licence and GPU passthrough is not needed for offline render); (c) proceed on layers one and two only and accept unproven parity until later. I recommend (a).
