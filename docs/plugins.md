# Stock plugins

Phase 4 checklist: every generator and effect FL Studio 26.1.7 ships, whether a render-test project exists for it in `Data/System/Render tests/`, and how many of the 165 shipped projects use it (by event 201 name). Sizes are the x64 DLL. Status column is ours; empty means not started.

Corpus render at `ff3ce57`: 114 of 165 shipped projects have sound with the stock plugins built so far, and 57 of 165 play in full. 1081 of 1101 effect slots on inserts that sound reaches are hosted, and 186 of 215 generator channels with notes play.

| kind | plugin | DLL KB | render test | used by projects | status |
|---|---|---:|:---:|---:|---|
| Effect | Fruity Parametric EQ 2 | 3942 | yes | 54 | implemented in crates/plugins (cookbook and matched HQ filters and both 2x resamplers exact; linear phase approximated), docs/plugins/fruity-parametric-eq-2.md |
| Effect | Fruity Limiter | 3777 | yes | 52 | implemented in crates/plugins, docs/plugins/fruity-limiter.md |
| Effect | Maximus | 5766 | yes | 40 | implemented in crates/plugins (IIR, FIR and linear-phase crossovers (the engine's FFT convolver, its rounding aside), curves, detectors, saturators, low cut and the polyphase half-band oversampling exact, stage times and the flush on a slope change as FL's), docs/plugins/maximus.md |
| Generator | 3x Osc | 4247 | yes | 39 | implemented in crates/plugins (HQ tables approximated), docs/plugins/3x-osc.md |
| Effect | Fruity Reeverb 2 | 3742 | yes | 38 | implemented in crates/plugins (network, taps, predelay and modulation follow the engine's listing sample for sample; coefficients follow the Delphi update in f64 and are not checked bit for bit), docs/plugins/fruity-reeverb-2.md |
| Effect | Fruity Delay 3 | 3847 |  | 31 | implemented in crates/plugins, docs/plugins/fruity-delay-3.md |
| Effect | Fruity WaveShaper | 5608 | yes | 29 | implemented in crates/plugins (oversampling approximated), docs/plugins/fruity-waveshaper.md |
| Effect | Fruity Stereo Shaper | 3711 | yes | 27 | implemented in crates/plugins, docs/plugins/fruity-stereo-shaper.md |
| Effect | Fruity Peak Controller | 3949 |  | 26 | implemented in crates/plugins (controller outputs reach 227 links through the 230 formula; `Tension()` formulas unsupported), docs/plugins/fruity-peak-controller.md |
| Effect | Fruity Soft Clipper | 3631 | yes | 23 | implemented in crates/plugins, docs/plugins/fruity-soft-clipper.md |
| Generator | Sytrus | 6122 | yes | 22 | implemented in crates/plugins, approximate (state versions 8..25 exact; operators, FM/RM matrix, articulators and three filters play; no unison sub-voices, plucked mode, waveshaper or effects; the 597 REC parameters read back and store inside their ranges, and those of the parts that are built are heard; versions 5..7 refused), docs/plugins/sytrus.md |
| Effect | Fruity PanOMatic | 3671 | yes | 21 | implemented in crates/plugins (approximated when the LFO runs at a rate other than 44100), docs/plugins/fruity-panomatic.md |
| Generator | Harmless | 4664 | yes | 21 | implemented in crates/plugins, exact with gaps (state versions 4..12, timbre colours and EQ, mask, filter/resonance/phaser curves with FL's point-below lookup under quality 2, noise resonance, frequency-shift phaser, pluck, harmonizer, protection, sine bank, envelopes, LFO, unison, legato and portamento glides, the host gains and their capped ramp, and the per-voice stereo tremolo exact; compression is the Maximus of Data/Compression/N.fst read at runtime; chorus, delay and reverb are FL's TStereoFlanger, TDelay_32FS and TFruityReverb from fl_fx.rs with Harmless's tables, told a 256-frame block; note colours (the colour groups of legato and portamento, the 14th and 15th turning MonoBox and PortaBox round, the filter notes of the 16th) and the pitch the host moves under a note exact; FL's random draws come from a generator of its own), docs/plugins/harmless.md |
| Generator | FPC | 5925 | yes | 20 | implemented in crates/plugins (state, pad and layer gains, velocity layers, pick modes, cut groups, pan law and the SSE gain ramp exact; a file read between its frames goes through a Hermite curve, pad envelopes run as Sytrus's, outputs above 0 play into the channel, a path that is not a file is silent), docs/plugins/fpc.md |
| Effect | Fruity Flangus | 3600 | yes | 19 | implemented in crates/plugins (exact; the truncating x87 voice renderer, the only one the DLL selects), docs/plugins/fruity-flangus.md |
| Generator | Harmor | 6326 | yes | 19 | implemented in crates/plugins, approximate (state versions 11..19 parse to the last byte; built: sine bank with per-partial ramps, pitch, ratio table, partial count, global EQ, timbres and blends, start phases, voice lifecycle and output, articulators, filters 1 and 2 with masks and mix, resonance including noise, unison, compressor, vibrato, distortion, reverb, chorus, delay, harmonizer, pluck, phaser, prism, blur, protection, clipping, auto-gain, tremolo, legato, portamento and mono; approximated: envelope sustain weights, shared envelope runs, unison type 4, LFO speed and phase articulators, custom filter reach, delay damping, noise numbers; not built: images and resynthesis, strum, arpeggiator, the HQ render; no null test yet), docs/plugins/harmor.md |
| Effect | Fruity Love Philter | 5856 | yes | 18 | implemented in crates/plugins (all twelve filters, oversampling, articulators, follower and waveshaper exact; looped sustain and arpeggiator points approximated, none in the corpus), docs/plugins/fruity-love-philter.md |
| Generator | DirectWave | 1094 | yes | 18 | implemented in crates/plugins (state, zones, groups, the 32.32 linear reader, loop modes, ADSR envelopes, LFOs, the modulation matrix, cookbook filters, limiter, gains and shapers exact; the program's three send effects (one a reverb), program LFOs, zone effects, filter types 6 and 7, stretching and HQ interpolation left out and said; FL's `rand` stream and block sizes not FL's), docs/plugins/directwave.md |
| Generator | Slicex | 6135 | yes | 18 | implemented in crates/plugins, approximate (state, regions from the WAV's cue, ltxt and tlst chunks, key map, slice volume, pan and pitch, knob volume and pitch exact; articulators, declicker, filter, stretching, reverse, start offset and the layer modes that pick one slice left out; FL's resampler stands in as a Hermite curve), docs/plugins/slicex.md |
| Effect | Control Surface | 5556 |  | 17 | implemented in crates/plugins (the sound passes untouched, `FPF_NoProcess`; state versions 1 and -1, every control type's values, parameters in 2^30ths and output n as parameter n's value, exact; a loaded state sends its values on the first tick, where FL leaves the targets at what the project saved until a value is set, which in every corpus link agree; MIDI out and the keyboard control left out), docs/plugins/control-surface.md |
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
| Effect | Fruity Chorus | 3656 | yes | 9 | implemented in crates/plugins (exact; LFO phases from Delphi's Random from seed 0; its render test sits inside a Fruity Wrapper and plays through its swap), docs/plugins/fruity-chorus.md |
| Effect | Fruity Convolver | 6012 | yes | 9 | implemented in crates/plugins (convolution, levels, separation, normalise and pre-delay exact at the impulse's rate; resampled impulses, drawn layers, self-convolution and EQ curves approximated), docs/plugins/fruity-convolver.md |
| Effect | Fruity Stereo Enhancer | 3641 | yes | 9 | implemented in crates/plugins, docs/plugins/fruity-stereo-enhancer.md |
| Effect | Luxeverb | 4239 |  | 9 | implemented in crates/plugins (a shell around the shimmer reverb in Shared/ildsp_x64.dll, read in full: control curves, 16-line network, damping shelves, LFO, matrix, predelay, diffusers, envelope, limiter, peak filter, feedback and feedforward pitch shifters; all 126 corpus and preset states, versions 7 and 8; FL's round-toward-zero arithmetic and the shifters' seed shared across instances are not reproduced; no render test, unit tests only), docs/plugins/luxeverb.md |
| Effect | Spreader | 3821 | yes | 9 | implemented in crates/plugins (exact; the DSP is a Faust processor in Shared/ildsp_x64.dll, transcribed), docs/plugins/spreader.md |
| Effect | ZGameEditor Visualizer | 807 |  | 9 | implemented in crates/plugins (the sound passes untouched, exact, for a state whose layers run effects read to make no sound and that loads no video: all 9 corpus states; an effect that renders audio, video clips' audio and the sound components ZGameEditor's mixer plays left out, passed through; the display and the 1024 controller outputs left out), docs/plugins/zgameeditor-visualizer.md |
| Generator | Autogun | 6144 | yes | 9 | implemented in crates/plugins, approximate (state versions 13..19 parse to the last byte; exact as read: seeds and their Jenkins mix, IPP's uniform generator on its generic path, curve baking and combs, spectrum levels, per-partial decay and release, inverse-FFT wavetable frames with their crossfade, 6-point interpolator, unison, articulators, pan law and ramps, declick, 1-3 stage filter at twice the rate, EQ, chorus/delay/reverb on fl_fx.rs, magic dust as the Maximus of Autogun/Data/SG.dat; approximated: IPP's exp, a timbre seed 2 of 0 counts from 0 rather than FL's process-wide counter, qualities other than 1 keep quality 1's interpolator and drop the output oversampling filter, portamento and velocity-envelope mode 2 left out, 16 voices; no null test), docs/plugins/autogun.md |
| Generator | FL Keys | 4114 | yes | 8 | implemented in crates/plugins (state, the four shipped wavetables read at runtime from FL's `Wavtables` (raw files directly, RIFF/Ogg files decoded by the host), key groups, velocity layers, mda's integer reader, envelope, release, overdrive, muffle filter, key pan, treble shelf, LFO, comb and the 32-voice limit exact; approximated: the channel volume is not raised to the velocity sensitivity with the velocity (every corpus state), pitch does not follow slides, Roto Organ's load-time resample), docs/plugins/fl-keys.md |
| Effect | EQUO | 3728 | yes | 7 | implemented in crates/plugins (state versions 3 and 4, the curve with morph, shift, mix and master, 31 band passes with the 2x bands, balance law, gain lines, add mode, per-tick smoothing, silence sleep and the 2x allpass resampler exact; the send to another insert is not routed, so send only passes the input on (no corpus state has a send); a loaded state starts settled where FL glides in from flat; ticks counted from tempo and PPQ; the HQ render flag that forces oversampling is never set), docs/plugins/equo.md |
| Effect | Hyper Chorus | 3846 |  | 7 | implemented in crates/plugins (exact from the C++ engine in Engine_x64.dll: the 4 or 8 modulated lines with Catmull-Rom reads, feedback and movement panning, Q 0.5 filters, high-pass fade, equal-power mix, and the parameter glide that brings a loaded state in from the defaults; differences: f32 rounds to nearest where FL's block rounds toward zero, every instance starts its LFOs where FL's first instance does, a new rate re-applies the parameters; no render test, so unit tests and the 20 corpus states), docs/plugins/hyper-chorus.md |
| Effect | Pitch Shifter | 4161 | yes | 7 | implemented in crates/plugins (both engines from Shared/ildsp_x64.dll; IPP's FFT and the CRT maths swapped for Rust's, the sync button's load order inferred), docs/plugins/pitch-shifter.md |
| Effect | Wave Candy | 4247 |  | 7 | implemented in crates/plugins (the sound passes untouched, exact: `Eff_Render` feeds its display and copies the block with `ippsCopy_64s`; the display is left out), docs/plugins/wave-candy.md |
| Effect | Fruity NoteBook 2 | 3528 |  | 6 | implemented in crates/plugins (the sound passes untouched, exact: `FPF_NoProcess`, so FL never calls its abstract `Eff_Render`), docs/plugins/fruity-notebook.md |
| Generator | Fruity DX10 | 4230 | yes | 6 | implemented in crates/plugins (mda DX10 with a second modulator, all 74 shipped states exact; the instrument tab and the mod X/Y filter not built), docs/plugins/fruity-dx10.md |
| Generator | Ogun | 6043 | yes | 6 | implemented in crates/plugins on Autogun's code (state versions 0 to 19; approximated as Autogun is), docs/plugins/ogun.md |
| Effect | Emphasis | 4045 | yes | 5 | implemented in crates/plugins (the limiter of Shared/ildsp_x64.dll: slow adaptive stage, oversampled brickwall, M/S and true-peak stages, halfbands, lookaheads, delta, level matching, bypass and rebuild fades exact; Versatile and Loud follow the input once a 512-sample block where FL uses its own blocks; meters left out), docs/plugins/emphasis.md |
| Effect | Vintage Chorus | 3849 | yes | 5 | implemented in crates/plugins (the engine_x64.dll chorus exact; approximated: the hiss seeds, which are those of the first engine FL loads, and tempo sync, which follows the host's ticks), docs/plugins/vintage-chorus.md |
| Generator | Fruity Slicer | 4483 | yes | 5 | implemented in crates/plugins (state, key map, slice lengths, play to end, reverse, slice declicker, attack and decay fades, pan law and the note-off declick exact; Elastique's time stretch left out, a stretched slice plays unstretched in its new place; the volume release runs per frame, not per tick; the other envelopes and LFOs left out; FL's resampler stands in as a Hermite curve), docs/plugins/fruity-slicer.md |
| Generator | GMS | 1516 | yes | 5 | |
| Effect | Edison | 6077 |  | 4 | implemented in crates/plugins (the sound passes untouched, exact, for a state that does not mute the input and holds no audio: 5 of 6 corpus states; a muted input and the playback of a held sample left out, passed through: the second Edison of Asher Postman - Future House, which holds a sample MIDI notes can start), docs/plugins/edison.md |
| Effect | Frequency Shifter | 3786 | yes | 4 | implemented in crates/plugins (the Engine_x64.dll shifters, HQ crossovers, Hilbert pair, ring modulator and tempo sync read in full and summed in its order; the CRT's sinf, sin, tanhf, powf and pow are Rust's; glides from the engine's defaults land when a state loads; tempo sync counts four steps a beat and the song's frames from the host's ticks), docs/plugins/frequency-shifter.md |
| Effect | Fruity Filter | 3692 | yes | 4 | implemented in crates/plugins, docs/plugins/fruity-filter.md |
| Effect | Fruity Flanger | 3629 | yes | 4 | implemented in crates/plugins (exact; a new sample rate empties the lines where FL keeps them), docs/plugins/fruity-flanger.md |
| Generator | Morphine | 1704 |  | 4 | |
| Effect | Distructor | 3571 |  | 3 | |
| Effect | Frequency Splitter | 4284 |  | 3 | implemented in crates/plugins (IIR crossovers exact; linear phase window and FFT scaling inferred; bands to other mixer tracks not routed, no shipped state sets one), docs/plugins/frequency-splitter.md |
| Effect | Fruity Parametric EQ | 3727 | yes | 3 | implemented in crates/plugins, docs/plugins/fruity-parametric-eq.md |
| Effect | Fruity Phaser | 3622 | yes | 3 | implemented in crates/plugins (exact), docs/plugins/fruity-phaser.md |
| Effect | Fruity X-Y Controller | 3684 |  | 3 | implemented in crates/plugins (the sound passes untouched, `FPF_NoProcess`; state, the ten parameters, speed and acceleration at the song's PPQ, the point's motion each tick, tension, min and max and the two outputs exact, with Rust's `ln`, `exp`, `atan` and `sin_cos` for Delphi's; a loaded pad starts at its levels, inferred from FL ticking while stopped; the joystick input left out, none in the corpus), docs/plugins/fruity-x-y-controller.md |
| Effect | Low Lifter | 3704 | yes | 3 | implemented in crates/plugins (exact: the Faust processor in Shared/ildsp_x64.dll transcribed and matched bit for bit against its own code run under an emulator, over the corpus and preset states at five rates; the MSVC runtime's FMA3 tanf, expf and powf and Delphi's Ln and Exp for the wheel curves transcribed too; the render's one-second quiet gate and the 10 ms lookahead latency as FL's; not yet nulled against FL), docs/plugins/low-lifter.md |
| Generator | BassDrum | 3973 | yes | 3 | implemented in crates/plugins (oscillator table and noise sample read from FL's data at runtime), docs/plugins/bassdrum.md |
| Generator | BooBass | 3599 | yes | 3 | implemented in crates/plugins (samples read from FL's BooBass_Eng_x64.dll at runtime), docs/plugins/boobass.md |
| Generator | Fruit Kick | 4168 | yes | 3 | implemented in crates/plugins (articulation block approximated), docs/plugins/fruit-kick.md |
| Generator | Fruity Envelope Controller | 6105 |  | 3 | |
| Generator | Kepler | 3770 |  | 3 | implemented in crates/plugins (Kepler-1 state versions 1..4 onto the engine's 258 parameters and their defaults, 8 voices and their allocation, BLIT DCO with saw, pulse, PWM and sub, the engine's recorded noise and LFO wave read from its DLL, LFO 1 with delay, sync and trigger, the ADSR, the ladder with its tanh stages, 2x/4x oversampling and the halfband decimator, highpass, chorus I/II/I+II, main volume, the arpeggiator up/up-down/down over 1/2/4 octaves free or synced; approximated: the step table normalised by an exact reciprocal, no crossfade on a chorus mode change, FL's MIDI volume and pan not read (output gain 6.309), the sync position counted from the first frame, hold, the chorus noise and automation smoothing not built), docs/plugins/kepler.md |
| Generator | MIDI Out | 3953 |  | 3 | |
| Generator | Sakura | 4194 | yes | 3 | |
| Effect | Fruity Fast LP | 3664 | yes | 2 | implemented in crates/plugins, docs/plugins/fruity-fast-lp.md |
| Effect | Fruity Free Filter | 3689 | yes | 2 | implemented in crates/plugins (exact; the corner is worked out at 44.1 kHz at every rate, as in FL), docs/plugins/fruity-free-filter.md |
| Effect | Fruity NoteBook | 3526 |  | 2 | implemented in crates/plugins (the sound passes untouched, exact: `FPF_NoProcess`, so FL never calls its abstract `Eff_Render`), docs/plugins/fruity-notebook.md |
| Effect | Fruity Wrapper | 5856 |  | 2 | implemented in crates/plugins (only what FL's project loader does with it: every corpus wrapper holds the FL 11 to 13 VST build of one of FL's own effects, which `FLEngine_x64.dll` swaps for the effect itself at its defaults and moves to the VST chunk's parameters; that swap is exact, and hosts the wrapped Balance, Chorus, Compressor, Fast LP and Filter, 23 of the 36 corpus states; the wrapped Bass Boost, Center, Flanger, Free Filter, Mute 2, Phase Inverter, Phaser and Reeverb are not built and passed through, and a third-party VST, none in the corpus, is never hosted), docs/plugins/fruity-wrapper.md |
| Effect | Hardcore | 626 | yes | 2 | |
| Effect | Transporter | 4072 | yes | 2 | |
| Generator | Drumpad | 4008 | yes | 2 | implemented in crates/plugins (state, the 38 parameters with their velocity slots, the membrane lines, mallet, LOW, MID, HIGH and SOF filters, hold, lo-fi, the 38 kHz render and its resampler exact, and every shipped state plays exact; approximated: 32 voices where FL has 99, membrane room sized per state, each instance's noise starting where FL's first does, DMP1 and DMP2 states turned down), docs/plugins/drumpad.md |
| Generator | Fruity Granulizer | 4371 | yes | 2 | |
| Generator | Fruity Wrapper | 5857 |  | 2 | |
| Generator | Minisynth | 542 | yes | 2 | |
| Generator | Toxic Biohazard | 737 |  | 2 | |
| Generator | Wave Traveller | 4369 | yes | 2 | |
| Effect | Emphasizer | 3688 |  | 1 | |
| Effect | Fruity Bass Boost | 3664 | yes | 1 | implemented in crates/plugins (exact; a new sample rate works the corner out again where FL keeps it), docs/plugins/fruity-bass-boost.md |
| Effect | Fruity Delay | 3685 |  | 1 | implemented in crates/plugins (exact: tempo-synced ring, the three modes, the feedback low pass in f64 as FL's SSE loops keep it, both ramps with the renderer's cap; the input-add routine is taken to be Balance's; no render test, unit tests only), docs/plugins/fruity-delay.md |
| Effect | Fruity Formula Controller | 3779 |  | 1 | |
| Effect | Fruity HTML NoteBook | 3835 |  | 1 | implemented in crates/plugins (the sound passes untouched, exact: `FPF_NoProcess`, so FL never calls its abstract `Eff_Render`), docs/plugins/fruity-notebook.md |
| Effect | Fruity Reeverb | 3627 | yes | 1 | implemented in crates/plugins (exact: the plate's twelve lines, taps, cuts, colour shelf and damping as the render reads them, in f32 with FL's 1e-30 fill), docs/plugins/fruity-reeverb.md |
| Effect | Fruity Scratcher | 4172 | yes | 1 | |
| Effect | Fruity Send | 3621 | yes | 1 | implemented in crates/plugins (pan matrix, knob curves and the send to the chosen insert through the host's output buffer, as Stereo Shaper's difference goes; the send's mixing routine is not read and is taken to be Balance's), docs/plugins/fruity-send.md |
| Effect | Fruity Squeeze | 3736 | yes | 1 | implemented in crates/plugins (exact: crusher, gate with its crossfaded edges, the twice-run state-variable filter before or after, mix and gain), docs/plugins/fruity-squeeze.md |
| Effect | Fruity Vocoder | 3725 | yes | 1 | |
| Effect | Fruity dB Meter | 3576 |  | 1 | implemented in crates/plugins (the sound passes untouched, exact: `Eff_Render` keeps the peaks and copies the block with `ippsCopy_64s`; the meter is left out), docs/plugins/fruity-db-meter.md |
| Effect | Razer Chroma | 3952 |  | 1 | implemented in crates/plugins (the sound passes untouched, exact: `Eff_Render` reads the block for the keyboard lights and copies it with `ippsCopy_64s`; the lights are left out), docs/plugins/razer-chroma.md |
| Effect | Vintage Phaser | 3880 |  | 1 | implemented in crates/plugins (the engine_x64.dll phaser, both engines, the upsampler, halfbands, hiss, gate and delay, the same to the bit as the DLL run under an emulator but for an ULP of the sine LFO's sinf; approximated: the hiss seeds, which are those of the first engine FL loads, and tempo sync, which follows the host's ticks), docs/plugins/vintage-phaser.md |
| Generator | BeepMap | 4186 | yes | 1 | |
| Generator | Drumaxx | 4319 | yes | 1 | |
| Generator | Fruity DrumSynth Live | 1062 | yes | 1 | implemented in crates/plugins (state versions 2..4, the 120 patches by key, morph, voice modes, both oscillators, waves read from the engine DLL's resource, harmonics, re-trigger, tone filter, release and the noise generator with its seed exact; approximated: the gain ramp of a cut voice, FL's channel volume counted once where FL counts it twice, FL's block order of the one noise generator, sends to a mixer track not built (none in the corpus), 256 voices at most), docs/plugins/fruity-drumsynth-live.md |
| Generator | Fruity Keyboard Controller | 3923 |  | 1 | |
| Generator | Fruity Voltage Controller | 3992 | yes | 1 | |
| Generator | PoiZone | 3867 | yes | 1 | |
| Effect | FL Studio Mobile Rack FX | 8036 |  | 0 | |
| Effect | Fruity 7 Band EQ | 3584 |  | 0 | |
| Effect | Fruity Big Clock | 3514 |  | 0 | |
| Effect | Fruity Center | 3504 | yes | 0 | implemented in crates/plugins (exact: the follower `v += (x - y) a; y += v; v *= 0.96` per side in doubles, its pace `4.567e-6 * 44100 / rate` as a single, the state under 2^-24 cleared after each block; its render test sits inside a Fruity Wrapper), docs/plugins/fruity-center.md |
| Effect | Fruity LSD | 4398 |  | 0 | |
| Effect | Fruity Mute 2 | 3605 | yes | 0 | implemented in crates/plugins (exact: mutes below half way, L, L + R or R by thirds of the Channel wheel, a muted side multiplied by 0 or zeroed as FL does; its render test sits inside a Fruity Wrapper), docs/plugins/fruity-mute-2.md |
| Effect | Fruity Phase Inverter | 3603 | yes | 0 | implemented in crates/plugins (exact: Off, Left or Right by thirds of the wheel, half to even, the side's sign bits flipped; its render test sits inside a Fruity Wrapper), docs/plugins/fruity-phase-inverter.md |
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
