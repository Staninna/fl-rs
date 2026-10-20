# Stock plugins

Phase 4 checklist: every generator and effect FL Studio 26.1.7 ships, whether a render-test project exists for it in `Data/System/Render tests/`, and how many of the 165 shipped projects use it (by event 201 name). Sizes are the x64 DLL. Status column is ours; empty means not started.

Corpus render at `20f0570`: 112 of 165 shipped projects have sound with the stock plugins built so far, and 33 of 165 play in full. 928 of 1028 effect slots on inserts that sound reaches are hosted, and 180 of 215 generator channels with notes play.

| kind | plugin | DLL KB | render test | used by projects | status |
|---|---|---:|:---:|---:|---|
| Effect | Fruity Parametric EQ 2 | 3942 | yes | 54 | implemented in crates/plugins (cookbook path exact; HQ matched, linear phase and FIR oversampling approximated), docs/plugins/fruity-parametric-eq-2.md |
| Effect | Fruity Limiter | 3777 | yes | 52 | implemented in crates/plugins, docs/plugins/fruity-limiter.md |
| Effect | Maximus | 5766 | yes | 40 | implemented in crates/plugins (IIR and FIR crossovers, curves, detectors, saturators and low cut exact; 2x oversampling and the linear-phase crossover approximated), docs/plugins/maximus.md |
| Generator | 3x Osc | 4247 | yes | 39 | implemented in crates/plugins (HQ tables approximated), docs/plugins/3x-osc.md |
| Effect | Fruity Reeverb 2 | 3742 | yes | 38 | implemented in crates/plugins (network, taps, predelay and modulation follow the engine's listing sample for sample; coefficients follow the Delphi update in f64 and are not checked bit for bit), docs/plugins/fruity-reeverb-2.md |
| Effect | Fruity Delay 3 | 3847 |  | 31 | implemented in crates/plugins, docs/plugins/fruity-delay-3.md |
| Effect | Fruity WaveShaper | 5608 | yes | 29 | implemented in crates/plugins (oversampling approximated), docs/plugins/fruity-waveshaper.md |
| Effect | Fruity Stereo Shaper | 3711 | yes | 27 | implemented in crates/plugins, docs/plugins/fruity-stereo-shaper.md |
| Effect | Fruity Peak Controller | 3949 |  | 26 | implemented in crates/plugins (controller outputs reach 227 links through the 230 formula; `Tension()` formulas unsupported), docs/plugins/fruity-peak-controller.md |
| Effect | Fruity Soft Clipper | 3631 | yes | 23 | implemented in crates/plugins, docs/plugins/fruity-soft-clipper.md |
| Generator | Sytrus | 6122 | yes | 22 | implemented in crates/plugins, approximate (state versions 8..25 exact; operators, FM/RM matrix, articulators and three filters play; no unison sub-voices, plucked mode, waveshaper, effects or automation; versions 5..7 refused), docs/plugins/sytrus.md |
| Effect | Fruity PanOMatic | 3671 | yes | 21 | implemented in crates/plugins (approximated when the LFO runs at a rate other than 44100), docs/plugins/fruity-panomatic.md |
| Generator | Harmless | 4664 | yes | 21 | implemented in crates/plugins, approximate (state versions 4..12, timbre colours and EQ, mask, filter/resonance/phaser curves, pluck, harmonizer, protection, sine bank, envelopes, LFO and unison exact; compression is the Maximus of Data/Compression/N.fst read at runtime; chorus, delay and reverb are FL's TStereoFlanger, TDelay_32FS and TFruityReverb from fl_fx.rs with Harmless's tables, told a 256-frame block; approximated: tremolo smoothing, mono/legato/portamento, slide notes, phaser type 9 and resonance type 8), docs/plugins/harmless.md |
| Generator | FPC | 5925 | yes | 20 | implemented in crates/plugins (state, pad and layer gains, velocity layers, pick modes, cut groups, pan law and the SSE gain ramp exact; a file read between its frames goes through a Hermite curve, pad envelopes run as Sytrus's, outputs above 0 play into the channel, a path that is not a file is silent), docs/plugins/fpc.md |
| Effect | Fruity Flangus | 3600 | yes | 19 | implemented in crates/plugins (exact; the truncating x87 voice renderer, the only one the DLL selects), docs/plugins/fruity-flangus.md |
| Generator | Harmor | 6326 | yes | 19 | implemented in crates/plugins, approximate (state versions 11..19 parse to the last byte; built: sine bank with per-partial ramps, pitch, ratio table, partial count, global EQ, timbres and blends, start phases, voice lifecycle and output, articulators, filters 1 and 2 with masks and mix, resonance including noise, unison, compressor, vibrato, distortion, reverb, chorus, delay, harmonizer, pluck, phaser, prism, blur, protection, clipping, auto-gain, tremolo, legato, portamento and mono; approximated: envelope sustain weights, shared envelope runs, unison type 4, LFO speed and phase articulators, custom filter reach, delay damping, noise numbers; not built: images and resynthesis, strum, arpeggiator, the HQ render; no null test yet), docs/plugins/harmor.md |
| Effect | Fruity Love Philter | 5856 | yes | 18 | implemented in crates/plugins (all twelve filters, oversampling, articulators, follower and waveshaper exact; looped sustain and arpeggiator points approximated, none in the corpus), docs/plugins/fruity-love-philter.md |
| Generator | DirectWave | 1094 | yes | 18 | implemented in crates/plugins (state, zones, groups, the 32.32 linear reader, loop modes, ADSR envelopes, LFOs, the modulation matrix, cookbook filters, limiter, gains and shapers exact; the program's three send effects (one a reverb), program LFOs, zone effects, filter types 6 and 7, stretching and HQ interpolation left out and said; FL's `rand` stream and block sizes not FL's), docs/plugins/directwave.md |
| Generator | Slicex | 6135 | yes | 18 | implemented in crates/plugins, approximate (state, regions from the WAV's cue, ltxt and tlst chunks, key map, slice volume, pan and pitch, knob volume and pitch exact; articulators, declicker, filter, stretching, reverse, start offset and the layer modes that pick one slice left out; FL's resampler stands in as a Hermite curve), docs/plugins/slicex.md |
| Effect | Control Surface | 5556 |  | 17 | |
| Effect | Fruity Fast Dist | 3616 | yes | 17 | implemented in crates/plugins, docs/plugins/fruity-fast-dist.md |
| Effect | Pitcher | 436 | yes | 17 | implemented in crates/plugins (detector, scale correction, delay-line and formant shifters, harmony voices exact up to the last bit of powf, log2f and cos, read from Pitcher_x64.dll itself; MIDI mode and MIDI-driven harmony approximated: no host sends the notes of a MIDI Out channel yet, 2 of 65 corpus states), docs/plugins/pitcher.md |
| Effect | Fruity Delay 2 | 3644 | yes | 16 | implemented in crates/plugins, docs/plugins/fruity-delay-2.md |
| Effect | Gross Beat | 5713 | yes | 16 | implemented in crates/plugins (state versions 1 to 9, time and volume envelopes, slot triggers and position sync from the song's tick, hold, link, one-shot, scratch, the sinc reader with its anti-aliasing, click reduction, DC removal and the 15-frame latency exact; the sinc's reciprocal and the tension's 11-bit IPP exp approximated; MIDI, keyboard and GUI slot triggers have no source; a song loop counts as a move, inferred), docs/plugins/gross-beat.md |
| Effect | Fruity Compressor | 3635 | yes | 15 | implemented in crates/plugins, docs/plugins/fruity-compressor.md |
| Effect | Transient Processor | 3878 | yes | 15 | implemented in crates/plugins (a shell around engine_x64.dll, read in full, exact), docs/plugins/transient-processor.md |
| Generator | FLEX | 7929 | yes | 15 | mapped (shell state high; patches live in obfuscated .flexpack archives and the engine DLLs, not read), docs/plugins/flex.md |
| Effect | Fruity Delay Bank | 3825 | yes | 13 | implemented in crates/plugins (routing, gains, delay lines, feedback modes and plain grain exact; the per-unit filters, 2x oversampler and shaped grain approximated, 30 of 41 corpus states), docs/plugins/fruity-delay-bank.md |
| Effect | Effector | 3639 | yes | 12 | implemented in crates/plugins (a shell around engine_x64.dll; the six effects the corpus picks, distortion, lo-fi, flanger, reverb, trance gate and vowel filter, exact, all 27 project states; the LFO resyncs to a song position the plugin counts itself; effects 3, 4, 5, 7, 9 and 11 unused in the corpus and passed through), docs/plugins/effector.md |
| Effect | Fruity Balance | 3631 | yes | 12 | implemented in crates/plugins, docs/plugins/fruity-balance.md |
| Effect | Fruity Multiband Compressor | 525 | yes | 12 | implemented in crates/plugins (a shell around comp_eng_x64.dll; both crossover types, compressor and limiter exact; FIR type has 52 frames latency), docs/plugins/fruity-multiband-compressor.md |
| Effect | Patcher | 6891 |  | 12 | |
| Generator | Patcher | 6887 |  | 12 | |
| Effect | Fruity Blood Overdrive | 3640 | yes | 11 | implemented in crates/plugins (exact), docs/plugins/fruity-blood-overdrive.md |
| Effect | Soundgoodizer | 6257 | yes | 10 | implemented in crates/plugins (a Maximus preset A to D with the amount on LMH mix), docs/plugins/maximus.md section 5.9 |
| Effect | Vocodex | 5818 | yes | 10 | implemented in crates/plugins (state, the MOD and CAR inputs with two sidechain inputs, L-R encoding, band layout and distribution map, RBJ band filters with 2x oversampling past 11050 Hz, peak and RMS followers, smoothing curves, MIN TIMES, noise gate, unison, pitch shift, pass-throughs, contour, mapping envelopes and the closing Soundgoodizer as read from the DLL; plays FL's HQ render path, draft mode not built; noise generator, curved envelope segments and saturation approximated or left out, none of the last two in the corpus; the carrier synth is this crate's Sytrus), docs/plugins/vocodex.md |
| Effect | Fruity Chorus | 3656 | yes | 9 | implemented in crates/plugins (exact; LFO phases from Delphi's Random from seed 0; its render test sits inside a Fruity Wrapper, so unit tests only), docs/plugins/fruity-chorus.md |
| Effect | Fruity Convolver | 6012 | yes | 9 | implemented in crates/plugins (convolution, levels, separation, normalise, pre-delay and the resampling to the song's rate and the stretch through FL's 256-point interpolator exact; drawn layers, self-convolution and EQ curves left out), docs/plugins/fruity-convolver.md |
| Effect | Fruity Stereo Enhancer | 3641 | yes | 9 | implemented in crates/plugins, docs/plugins/fruity-stereo-enhancer.md |
| Effect | Luxeverb | 4239 |  | 9 | implemented in crates/plugins (a shell around the shimmer reverb in Shared/ildsp_x64.dll, read in full: control curves, 16-line network, damping shelves, LFO, matrix, predelay, diffusers, envelope, limiter, peak filter, feedback and feedforward pitch shifters; all 126 corpus and preset states, versions 7 and 8; FL's round-toward-zero arithmetic and the shifters' seed shared across instances are not reproduced; no render test, unit tests only), docs/plugins/luxeverb.md |
| Effect | Spreader | 3821 | yes | 9 | implemented in crates/plugins (exact; the DSP is a Faust processor in Shared/ildsp_x64.dll, transcribed), docs/plugins/spreader.md |
| Effect | ZGameEditor Visualizer | 807 |  | 9 | |
| Generator | Autogun | 6144 | yes | 9 | implemented in crates/plugins, approximate (state versions 13..19 parse to the last byte; exact as read: seeds and their Jenkins mix, IPP's uniform generator on its generic path, curve baking and combs, spectrum levels, per-partial decay and release, inverse-FFT wavetable frames with their crossfade, 6-point interpolator, unison, articulators, pan law and ramps, declick, 1-3 stage filter at twice the rate, EQ, chorus/delay/reverb on fl_fx.rs, magic dust as the Maximus of Autogun/Data/SG.dat; approximated: IPP's exp, a timbre seed 2 of 0 counts from 0 rather than FL's process-wide counter, qualities other than 1 keep quality 1's interpolator and drop the output oversampling filter, portamento and velocity-envelope mode 2 left out, 16 voices; no null test), docs/plugins/autogun.md |
| Generator | FL Keys | 4114 | yes | 8 | implemented in crates/plugins (state, the four shipped wavetables read at runtime from FL's `Wavtables` (raw files directly, RIFF/Ogg files decoded by the host), key groups, velocity layers, mda's integer reader, envelope, release, overdrive, muffle filter, key pan, treble shelf, LFO, comb and the 32-voice limit exact; approximated: the channel volume is not raised to the velocity sensitivity with the velocity (every corpus state), pitch does not follow slides, Roto Organ's load-time resample), docs/plugins/fl-keys.md |
| Effect | EQUO | 3728 | yes | 7 | implemented in crates/plugins (state versions 3 and 4, the curve with morph, shift, mix and master, 31 band passes with the 2x bands, balance law, gain lines, add mode, per-tick smoothing, silence sleep and the 2x allpass resampler exact; the send to another insert is not routed, so send only passes the input on (no corpus state has a send); a loaded state starts settled where FL glides in from flat; ticks counted from tempo and PPQ; the HQ render flag that forces oversampling is never set), docs/plugins/equo.md |
| Effect | Hyper Chorus | 3846 |  | 7 | implemented in crates/plugins (exact from the C++ engine in Engine_x64.dll: the 4 or 8 modulated lines with Catmull-Rom reads, feedback and movement panning, Q 0.5 filters, high-pass fade, equal-power mix, and the parameter glide that brings a loaded state in from the defaults; differences: f32 rounds to nearest where FL's block rounds toward zero, every instance starts its LFOs where FL's first instance does, a new rate re-applies the parameters; no render test, so unit tests and the 20 corpus states), docs/plugins/hyper-chorus.md |
| Effect | Pitch Shifter | 4161 | yes | 7 | implemented in crates/plugins (both engines from Shared/ildsp_x64.dll; IPP's FFT and the CRT maths swapped for Rust's, the sync button's load order inferred), docs/plugins/pitch-shifter.md |
| Effect | Wave Candy | 4247 |  | 7 | |
| Effect | Fruity NoteBook 2 | 3528 |  | 6 | |
| Generator | Fruity DX10 | 4230 | yes | 6 | implemented in crates/plugins (mda DX10 with a second modulator, all 74 shipped states exact; the instrument tab and the mod X/Y filter not built), docs/plugins/fruity-dx10.md |
| Generator | Ogun | 6043 | yes | 6 | implemented in crates/plugins on Autogun's code (state versions 0 to 19; approximated as Autogun is), docs/plugins/ogun.md |
| Effect | Emphasis | 4045 | yes | 5 | |
| Effect | Vintage Chorus | 3849 | yes | 5 | |
| Generator | Fruity Slicer | 4483 | yes | 5 | |
| Generator | GMS | 1516 | yes | 5 | |
| Effect | Edison | 6077 |  | 4 | |
| Effect | Frequency Shifter | 3786 | yes | 4 | |
| Effect | Fruity Filter | 3692 | yes | 4 | implemented in crates/plugins, docs/plugins/fruity-filter.md |
| Effect | Fruity Flanger | 3629 | yes | 4 | |
| Generator | Morphine | 1704 |  | 4 | |
| Effect | Distructor | 3571 |  | 3 | |
| Effect | Frequency Splitter | 4284 |  | 3 | implemented in crates/plugins (IIR crossovers exact; linear phase window and FFT scaling inferred; bands to other mixer tracks not routed, no shipped state sets one), docs/plugins/frequency-splitter.md |
| Effect | Fruity Parametric EQ | 3727 | yes | 3 | |
| Effect | Fruity Phaser | 3622 | yes | 3 | |
| Effect | Fruity X-Y Controller | 3684 |  | 3 | |
| Effect | Low Lifter | 3704 | yes | 3 | |
| Generator | BassDrum | 3973 | yes | 3 | implemented in crates/plugins (oscillator table and noise sample read from FL's data at runtime), docs/plugins/bassdrum.md |
| Generator | BooBass | 3599 | yes | 3 | implemented in crates/plugins (samples read from FL's BooBass_Eng_x64.dll at runtime), docs/plugins/boobass.md |
| Generator | Fruit Kick | 4168 | yes | 3 | implemented in crates/plugins (articulation block approximated), docs/plugins/fruit-kick.md |
| Generator | Fruity Envelope Controller | 6105 |  | 3 | |
| Generator | Kepler | 3770 |  | 3 | |
| Generator | MIDI Out | 3953 |  | 3 | |
| Generator | Sakura | 4194 | yes | 3 | |
| Effect | Fruity Fast LP | 3664 | yes | 2 | implemented in crates/plugins, docs/plugins/fruity-fast-lp.md |
| Effect | Fruity Free Filter | 3689 | yes | 2 | |
| Effect | Fruity NoteBook | 3526 |  | 2 | |
| Effect | Fruity Wrapper | 5856 |  | 2 | |
| Effect | Hardcore | 626 | yes | 2 | |
| Effect | Transporter | 4072 | yes | 2 | |
| Generator | Drumpad | 4008 | yes | 2 | implemented in crates/plugins (state, the 38 parameters with their velocity slots, the membrane lines, mallet, LOW, MID, HIGH and SOF filters, hold, lo-fi, the 38 kHz render and its resampler exact, and every shipped state plays exact; approximated: 32 voices where FL has 99, membrane room sized per state, each instance's noise starting where FL's first does, DMP1 and DMP2 states turned down), docs/plugins/drumpad.md |
| Generator | Fruity Granulizer | 4371 | yes | 2 | |
| Generator | Fruity Wrapper | 5857 |  | 2 | |
| Generator | Minisynth | 542 | yes | 2 | |
| Generator | Toxic Biohazard | 737 |  | 2 | |
| Generator | Wave Traveller | 4369 | yes | 2 | |
| Effect | Emphasizer | 3688 |  | 1 | |
| Effect | Fruity Bass Boost | 3664 | yes | 1 | |
| Effect | Fruity Delay | 3685 |  | 1 | |
| Effect | Fruity Formula Controller | 3779 |  | 1 | |
| Effect | Fruity HTML NoteBook | 3835 |  | 1 | |
| Effect | Fruity Reeverb | 3627 | yes | 1 | |
| Effect | Fruity Scratcher | 4172 | yes | 1 | |
| Effect | Fruity Send | 3621 | yes | 1 | |
| Effect | Fruity Squeeze | 3736 | yes | 1 | |
| Effect | Fruity Vocoder | 3725 | yes | 1 | |
| Effect | Fruity dB Meter | 3576 |  | 1 | |
| Effect | Razer Chroma | 3952 |  | 1 | |
| Effect | Vintage Phaser | 3880 |  | 1 | |
| Generator | BeepMap | 4186 | yes | 1 | |
| Generator | Drumaxx | 4319 | yes | 1 | |
| Generator | Fruity DrumSynth Live | 1062 |  | 1 | |
| Generator | Fruity Keyboard Controller | 3923 |  | 1 | |
| Generator | Fruity Voltage Controller | 3992 | yes | 1 | |
| Generator | PoiZone | 3867 | yes | 1 | |
| Effect | FL Studio Mobile Rack FX | 8036 |  | 0 | |
| Effect | Fruity 7 Band EQ | 3584 |  | 0 | |
| Effect | Fruity Big Clock | 3514 |  | 0 | |
| Effect | Fruity Center | 3504 | yes | 0 | |
| Effect | Fruity LSD | 4398 |  | 0 | |
| Effect | Fruity Mute 2 | 3605 | yes | 0 | |
| Effect | Fruity Phase Inverter | 3603 | yes | 0 | |
| Effect | Fruity Spectroman | 3668 |  | 0 | |
| Effect | Fruity X-Y-Z Controller | 5661 |  | 0 | |
| Effect | Multiband Delay | 3840 |  | 0 | |
| Effect | Newtime | 4963 |  | 0 | |
| Effect | Newtone | 4851 |  | 0 | |
| Effect | Transmitter | 4111 |  | 0 | |
| Effect | Tuner | 3691 |  | 0 | |
| Effect | VFX Color Mapper | 3827 |  | 0 | |
| Effect | VFX Envelope | 5624 |  | 0 | |
| Effect | VFX Key Mapper | 3907 |  | 0 | |
| Effect | VFX Keyboard Splitter | 5593 |  | 0 | |
| Effect | VFX Level Scaler | 4010 |  | 0 | |
| Effect | VFX Script | 4390 |  | 0 | |
| Effect | VFX Sequencer | 4074 |  | 0 | |
| Generator | Dashboard | 4918 |  | 0 | |
| Generator | FL Slayer | 5608 |  | 0 | |
| Generator | FL Studio Mobile | 11857 |  | 0 | |
| Generator | FL Studio Mobile Rack | 8039 |  | 0 | |
| Generator | Fruity Dance | 4065 |  | 0 | |
| Generator | Fruity Slicer 2 | 5791 |  | 0 | |
| Generator | Fruity Video Player | 4181 |  | 0 | |
| Generator | Kepler Exo | 4314 |  | 0 | |
| Generator | Plucked! | 3610 |  | 0 | |
| Generator | ReWired | 3898 |  | 0 | |
| Generator | Sawer | 4209 | yes | 0 | |
| Generator | SimSynth | 3760 | yes | 0 | |
| Generator | Soundfont Player | 4444 |  | 0 | |
| Generator | Transistor Bass | 3961 |  | 0 | |
| Generator | Wasp | 3513 |  | 0 | |
| Generator | Wasp XT | 3513 |  | 0 | |
