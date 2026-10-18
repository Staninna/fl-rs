# Fruity DX10: state, parameters, voice model

Recovered from `Plugins/Fruity/Generators/Fruity DX10/Fruity DX10_x64.dll`
(Delphi native plugin, image base 0x400000). Fruity DX10 is Paul Kellett's
mda DX10, a two-operator FM synth, which Image-Line rebuilt with a second
modulator. The Ghidra project and decompiles lived in a session scratchpad
and are gone. To read the DLL again, import it and run
`tools/ghidra/DecompileAddrs.java` on the addresses below.

Every formula in this doc was read from the assembly, with its f32 and f64
steps. Section 6 lists the few claims that are guesses.

## How the plugin is wired

`CreatePlugInstance` @ `0x00734510` -> constructor `FUN_00734ae0`,
`TTestPlug` VMT `0x007343f0`, over `TDelphiFruityPlug`.

| slot | SDK name | address |
|---|---|---|
| | Dispatcher | 0x00734570 |
| | SaveRestoreState | 0x00734940 |
| | ProcessEvent | 0x00734ee0 |
| | ProcessParam | 0x00734f30 |
| | Gen_Render | 0x00736a10 |
| | TriggerVoice | 0x00735e30 |
| | Voice_Release | 0x00736320 |
| | Voice_Kill | 0x00736370 |
| 13 | Voice_Render | 0x007364a0 |

The notes give the slot number only for Voice_Render.

Helpers:

| address | what it does |
|---|---|
| `FUN_00735760` | works out every derived value from the parameters (section 2) |
| `FUN_00735660` | coarse ratio |
| `FUN_00735690` | fine ratio |
| `FUN_00736800` | updates a voice's pitch again when the host's pitch moves |
| `FUN_00735d90` | voice steal |
| `FUN_00722c50` | reads and writes the instrument tab |
| `FUN_00722bc0` | the instrument tab's defaults |
| `FUN_00721dc0`, `FUN_00722290` | the articulation unit that mixes the voices |
| `FUN_005fad90` | the plugin's own copy of the circular pan law |
| `FUN_00720cb0` | the plugin's copy of the articulation filter, not read |

The plugin info reports 114 parameters and a default polyphony of 16.

ProcessEvent handles two events. It stores `FPE_MaxPoly` at `+0xe0`, and it
takes the tempo. `+0xe0` stays 0 unless the host sends `FPE_MaxPoly`, and 0
means no voice limit.

Object fields named in the notes:

| offset | meaning |
|---|---|
| 0xe0 | max polyphony, 0 unless the host sends `FPE_MaxPoly` |
| 0x188 | mod wheel, starts at 0, nothing in the DLL moves it |
| 0x18c | pitch bend, starts at 1, nothing in the DLL moves it |
| 0x1b8 | the 22 parameters as i32 |

A voice is a 0x78-byte record.

## 1. State layout (event 213)

`SaveRestoreState` (`0x00734940`) writes an i32 version of 2, then the 22
parameters from `+0x1b8` as i32. On restore it ignores a state whose version
is 3 or more, and also one whose version is 0x7fffffff. FL does not clamp the
values to their wheel's range.

| bytes | content |
|---|---|
| 0..3 | version, written as 2 |
| 4..91 | the 22 parameters, i32, in index order (section 2) |
| 92..95 | four bytes FL does not read |
| 96..459 | the instrument tab, 364 bytes |

Version 1 ends after the parameters, at 92 bytes. Version 2 is 460 bytes.

The instrument tab is the same articulation block that 3x Osc and Fruit
Kick save (`FUN_00722c50`), with its defaults in `FUN_00722bc0`. It holds
five envelope and LFO blocks of 17 i32 each, for pan, volume, mod X, mod Y
and pitch, then the filter's cutoff, resonance and type. `docs/plugins/3x-osc.md`
section 1 lays out a block. In the default tab every block is
`0, 0, 100, 20000, 20000, 30000, 50, 20000, 0, 100, 20000, 0, 32950, 0, 0, 0, 0`,
except the volume block, whose word 0 is 4 and word 16 is -101. The cutoff is
256, the resonance 0 and the filter type 0 (Fast LP).

Loading a state kills every voice.

## 2. Parameter semantics (`ProcessParam` @ 0x00734f30)

The wheels sit on the DFM form `TSynthEditorForm`. Each wheel's `Tag` is its
parameter index.

| index | parameter | range | default |
|---|---|---|---|
| 0 | attack | 0..65536 | 0 |
| 1 | decay | 0..65536 | 42598 |
| 2 | release | 0..65536 | 28901 |
| 3 | modulator 1 coarse ratio | 0..65536 | 55181 |
| 4 | modulator 1 fine ratio | 0..65536 | 21561 |
| 5 | modulator 1 initial depth | 0..65536 | 15073 |
| 6 | modulator 1 decay | 0..65536 | 52429 |
| 7 | modulator 1 sustain depth | 0..65536 | 3277 |
| 8 | modulator 1 release | 0..65536 | 52429 |
| 9 | modulator 1 velocity sensitivity | 0..65536 | 58982 |
| 10 | vibrato | 0..65536 | 0 |
| 11 | waveform | 0..65536 | 29295 |
| 12 | modulator thru | 0..65536 | 0 |
| 13 | LFO rate | 0..65536 | 27132 |
| 14..20 | modulator 2: coarse, fine, initial depth, decay, sustain, release, velocity | 0..65536 | 0 each |
| 21 | octave | -3..3 | 0 |

The defaults come from the table at `0x00734e40`. They are the program
"Bright E.Piano", which is mda DX10's program 1. Modulator 2 starts silent.

On `REC_UpdateValue` the plugin stores the integer and keeps
`p = (value - min) / (max - min)` as an f32. Then `FUN_00735760` works out
every derived value again. `rate` is the host's sample rate, and
`inv = 1 / rate` as an f32. `fall(k)` stands for `exp(-inv * exp(k))`.

```
octave  = floor(6.9 * p21)                          // 0..6, 3 is the wheel at 0
tune    = inv * 8.175798915644 * 2^(octave - 2)    // carrier step at key 0; 8.1758 Hz is MIDI note 0

attack  = 1 - fall(8 - 8 * p0)
decay   = p1 > 0.98 ? 1 : fall(5 - 8 * p1)          // at the top, no decay
release = fall(5 - 5 * p2)

for each modulator (params at 3..9 and 14..20):
    ratio   = (coarse + fine) * 2^30
    coarse  = floor(40.1 * pc^2)                    // FUN_00735660, 0..40
    fine    = pf < 0.5 ? 0.2 * pf^2                 // FUN_00735690
            : Round(8.9 * pf) -> 4: 1/4, 5: 1/3, 6: 1/2, 7: 2/3, else 3/4
    depth   = 0.0002 * (44100 / rate) * pinit^2
    sustain = 0.0002 * (44100 / rate) * psus^2
    decay   = 1 - fall(6 - 7 * pdec)
    release = 1 - fall(5 - 8 * prel)
    velsens = pvel

vibrato = 0.001 * p10^2
rich    = 0.5 - 3 * p11^2                          // the waveform's polynomial shape
modmix  = 0.25 * p12^2                             // modulator 1 heard as it is
lfo     = inv * 15707.5 * p13^2                    // LFO rotation per step
```

`Round` is the Delphi RTL's, to the nearest with ties to even.

## 3. The voice model

### Starting a voice (`TriggerVoice` @ 0x00735e30)

The host gives the pitch as cents from key 60, fine pitch included. The voice
splits it into a whole key and a fraction:

```
fraction = (Round(cents) mod 100) * 0.01
key      = clamp(trunc(cents * 0.01 - fraction + 60), 0, 127) + fraction
l        = exp(key * 0.05776226505)                 // 0.05776 = ln 2 / 12
step     = tune * l                                  // carrier step a sample
```

The host's velocity gain `v` (1.0 is MIDI velocity 100) becomes a 0..127
velocity:

```
velocity = Round(127 * ln(10 v + 1) / ln(21.551182))    // 10 v capped at 20.551182
```

Velocity 100 comes out as 99, and `v` from 2.0551 up gives 127.

Each modulator's depths scale by velocity and by key, with the key factor
capped at 50:

```
k        = min(l, 50) * (velsens * (velocity - 64) + 64)
depth    = depth * k,  target = sustain * k,  rate = decay
phase    = 0,  phase step = Round(ratio * step)     // u32
```

A carrier step of `s` covers `s / 2` of a cycle, and the modulator's phase
covers 2^32 a cycle, so a modulator turns at its ratio times half the carrier
frequency.

The carrier's envelope starts at

```
env0  = (1.5 - p11) * 0.0035f * (velocity + 10)
scale = 1.2 / env0
```

and the output level follows it from 0 at the attack rate. Because the
output is multiplied by `scale`, every voice's envelope starts at 1.2,
whatever the velocity.

### A sample (`Voice_Render` @ 0x007364a0)

```
level += attack * (env - level)
env   *= decay

s1 = sine[phase1 >> 18]                             // host WaveTables[0], 16384 points
s2 = sine[phase2 >> 18]
carrier += step + vibrato + depth1 * s1 + depth2 * s2
depth1  += rate1 * (target1 - depth1)
depth2  += rate2 * (target2 - depth2)
phase1  += step1;  phase2 += step2                  // u32, wraps
while |carrier| > 1: carrier -= 2 * sign(carrier)

c     = carrier
shape = c + c^3 * (rich * (c^2 - 1) - 1)            // mda's fifth-order polynomial
out   = scale * level * (modmix * s1 + shape)
```

The carrier is mda's: a phase from -1 to 1 read through a polynomial whose
shape the waveform wheel sets. Modulator 1 can also be heard directly through
the modulator thru (`modmix`). Modulator 2 only moves the carrier.

### Release, end and kill

`Voice_Release` (`0x00736320`) sets the carrier envelope to the current
level, its rate to `release` and the attack to 1, so the level follows the
envelope at once. Each modulator's depth target goes to 0 at its release
rate. Voice_Release does not call the articulation unit's release, so FL
applies no 10 ms declick fade.

At the end of each block:

- A voice whose envelope is under 0.0002 ends. The test compares the f32 bit
  patterns as signed integers, not the float values.
- A modulator depth under 5.96e-8 (bits 0x33800000, 2^-24) goes to 0, and so
  does its target.

`Voice_Kill` is at `0x00736370`. The notes record nothing more about it.

### The LFO

The plugin has one LFO, a sine and cosine pair that all voices share. It
starts at sine 0, cosine 1. Each step rotates it:

```
sin += lfo * cos
cos -= lfo * sin
vibrato = cos * (modwheel + vibrato_depth)          // modwheel at +0x188 stays 0
```

Each voice has its own counter and steps the shared LFO once every 101 of
its own samples. With n voices sounding, the LFO runs n times as fast. The
vibrato a voice reads is the value its last step left.

### Voice stealing (`FUN_00735d90`)

When the voices reach the limit at `+0xe0`, the plugin takes the quietest
voice that is not already dying: it sets the voice's envelope to 0 and marks
it dying. The stolen voice plays one more block, then ends. With `+0xe0` at
0, the default, FL never steals.

### Output scaling and pan law

The plugin mixes the voices through the articulation unit (`FUN_00721dc0`,
`FUN_00722290`). The unit applies a circular pan law from the plugin's own
copy at `FUN_005fad90`. Only the unit's init writes the law's selector. The
pan law takes the note's final pan and `Final.Vol` times the volume
envelope:

```
angle = (1 + pan) * pi / 4
left  = cos(angle) * vol,  right = sin(angle) * vol
```

Velocity reaches the output level only here. The voice itself is normalised
to 1.2.

The unit's Fast LP runs on `Final.FCut + tab cutoff / 256 + mod X`, and only
when the cutoff factor `f` is not 1 or the feedback `fb` is not 0. With a
default tab the unit does the plain pan law and no filter.

## 4. Differences from mda DX10

- Two modulators, where mda has one. Modulator 2 has its own ratio, depths,
  envelope and velocity sensitivity, and no thru.
- The modulators read the host's sine table (`WaveTables[0]`, index
  `phase >> 18`) with an integer phase. mda uses a recursive sine.
- The fine ratio rounds `8.9 p` with `Round`. mda truncates. At `p = 0.55`,
  `8.9 p = 4.895` gives mda 1/4 and FL 1/3.
- The depths scale with `44100 / rate`.
- The output is normalised so the envelope starts at 1.2, and velocity
  reaches the level only through the pan law.
- The LFO steps every 101 samples per voice, and each voice's counter steps
  the one shared LFO.
- The mod wheel (`+0x188`) stays 0 and the pitch bend (`+0x18c`) stays 1.
  Nothing in the DLL moves them.

## 5. Corpus

74 states parse: 11 in projects, the render test, 59 presets and three in
the plugin databases. All of them are exact under section 6's rules. The
corpus notes' mod X and mod Y values on DX10 channels were not checked.

## 6. What `dx10.rs` builds

`crates/plugins/src/dx10.rs` builds the state, the parameters, the full
voice, LFO and steal model, and reports `Fidelity::Approximate` when the
instrument tab or the filter would change a note.

Exact as read:

- The state, version and length rules. `set_state` rejects version 3 and up
  (0x7fffffff included) with an error and keeps the values unclamped. A state
  from 460 bytes on loads its tab. A shorter one gets the default tab.
  Loading clears every voice.
- The parameter defaults, ranges, `p` and every derived value of section 2,
  in the same f32 and f64 steps.
- The voice of section 3: key and fraction, velocity curve, depth scaling,
  envelopes, modulator phases on the host sine table, carrier wrap and
  polynomial, output scale, end threshold and the depth flush, both as bit
  compares.
- The shared LFO with its per-voice counter. `LFO_STEP` is 100, and the
  counter counts down from 100 to 0 before the next step, which is 101
  samples a step.
- The pan law, with the host's velocity gain as the volume.
- The steal rule of `FUN_00735d90`.

Approximated:

- The instrument tab is read but not played. The plugin reports approximate
  when the tab would be heard: the volume envelope on, any LFO or mod
  envelope with an amount, the filter not at cutoff 256, resonance 0, type 0,
  a pitch envelope with an amount, or a volume envelope that is off and has
  no start-at-full flag.
- A note with mod X under 0 or mod Y over 0 goes through the tab's Fast LP
  in FL. Here it plays unfiltered and marks the plugin approximate.
- FL sets no voice limit unless the host sends `FPE_MaxPoly`. Here the limit
  is 64 voices, and a note past it steals by FL's rule.
- A slide note sounds nothing. A voice's pitch stays where it started, where
  FL moves it with the host's pitch (`FUN_00736800`).
- Each voice adds straight into the output block. FL sums the voices in a
  buffer of its own first.
- A sample-rate change leaves the sounding voices as they started. A
  parameter change does the same, except the LFO rate, the vibrato depth,
  the waveform and the modulator thru, which voices read every sample.
- `tail` estimates the release from the loudest start envelope down to
  0.0002.

Guessed or unknown:

- That `Final.Vol` is the velocity times the channel volume, as FL Keys
  assumes. The Rust passes the velocity gain as the pan law's volume.
- The articulation's Fast LP for mod X below 0 or mod Y above 0. The DX10
  copy of the filter, `FUN_00720cb0`, was not read.
- Whether any corpus DX10 channel sets mod X or mod Y.
- The slot numbers of every VMT entry except Voice_Render.
- What ProcessEvent does with the tempo.
- What FL does with a version below 1.
