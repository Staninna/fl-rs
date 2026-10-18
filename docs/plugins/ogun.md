# Ogun: state, parameters, signal architecture

Recovered from `Ogun_x64.dll` (FL Studio 2026, Delphi native plugin, 6043
KB). Ogun is Autogun's code less the random presets. Every plugin function
was matched against its Autogun twin with a normalised diff of the two
decompiles, plus a check that the constants each one reads hold the same
values. This doc covers what Ogun adds or lacks. For everything shared it
points at the sections of [autogun.md](autogun.md) and does not repeat them.

Ghidra project `/shared/code/fl-studio-target/ghidra/proj`, project name
`ogun`. Decompiles in `ghidra/out/ogun/range_810000.c` and
`range_850000.c`.

Corpus: 256 states. 23 sit in the 6 projects that use Ogun and 233 in
presets. They span state versions 0 to 19, and all of them parse to the
last byte. 17 of the 23 project states run at quality 0, which Autogun's
corpus never does.

Addresses are Ogun DLL addresses unless marked as Autogun's. Confidence
tags as in autogun.md: `code` (read in the decompile), `corpus`, and
`guess` for what the code does not settle.

## How the plugin is wired

Plugin class `TTestPlug`, VMT `0x008621f8`, instance size 0x610 (Autogun's
is 0x620). Constructor `FUN_00866ec0`. `CreatePlugInstance` and the
destructor were not recorded.

| slot | SDK name | address |
|---|---|---|
| 1 | Dispatcher | 0x00862770 |
| 3 | SaveRestoreState | 0x00864d00 |
| 4 | GetName | 0x00869ac0 |
| 5 | ProcessEvent | 0x00867c60 |
| 6 | ProcessParam | 0x008684e0 |
| 8 | Gen_Render | 0x0086d450 |
| 9 | TriggerVoice | 0x0086a8e0 |
| 10 | Voice_Release | 0x0086c2a0 |
| 11 | Voice_Kill | 0x0086c420 |

Other functions: post-load `FUN_008649e0`, quality select `FUN_00866840`,
voice render `FUN_0086c6e0`, master level `FUN_0086a570`.

The timbre engine runs from `0x00842340` to `0x00844bc0`. Each function
sits at its Autogun address less 0x28b0 and is byte-identical to it
(`code`). The articulators are `TOpArticulator`, VMT `0x00861ef0`. The
curves are `TMMap`, VMT `0x00781658`. The chorus, delay and reverb are the
same classes as Autogun's (autogun.md, "How the plugin is wired").

Ogun has no Soundgoodizer host and loads no `SG.dat`.

## 1. State layout (event 213)

`SaveRestoreState` (`FUN_00864d00`) is Autogun's (autogun.md section 1)
with one change: Ogun saves and reads no random-preset seed word. Autogun
reads that i32 after the info text for versions above 18. Autogun keeps
the seed at plugin+0x610, which is where Ogun's 0x610-byte object ends.

For versions 13 to 19 the block, the three bytes, the EQ band types, the
info text, the 13 articulators and the 4 curves are as autogun.md section 1
lays them out. Before version 18 the info text is an i32 length and that
many ANSI bytes. From 18 on it is a wide string.

### Old versions 0 to 12

Every old state in the corpus is an Ogun preset. The loading code is the
same in both DLLs (`code`). `SaveRestoreState` first resets the block to
the constructor's values. It then reads a prefix of the 0x100-byte block
over them, in two parts:

1. The first 12 bytes (REC 0 to 2), or only 8 before version 8. Before
   version 8 there is no fine pitch (REC 2), and the loader sets it to 0.
2. From offset 12, 0xf4 bytes less what each older version lacks. Version
   0 lacks 1 byte more than version 1, and versions before 2, 7, 9, 11, 12
   and 13 lack 3, 1, 16, 1, 1 and 6 bytes more.

Each version added fields to the end of the block, so what a version lacks
is the tail. The missing fields keep the constructor's values. With the
field offsets of autogun.md section 1:

| version | block bytes read | ends before | lacks |
|---|---|---|---|
| 0 | 8 + 0xd7 | 0xe3 | fine pitch; release on volume envelope and everything after it |
| 1 | 8 + 0xd8 | 0xe4 | fine pitch; the two spare switches, portamento mode and everything after them |
| 2 to 6 | 8 + 0xdb | 0xe7 | fine pitch; log curve axis, the combs and everything after them |
| 7 | 8 + 0xdc | 0xe8 | fine pitch; the four curve combs and everything after them |
| 8 | 12 + 0xdc | 0xe8 | the four curve combs and everything after them |
| 9, 10 | 12 + 0xec | 0xf8 | "to target" decay, normalise to curves, 1/sqrt weighting, spare, magic dust |
| 11 | 12 + 0xed | 0xf9 | normalise to curves, 1/sqrt weighting, spare, magic dust |
| 12 | 12 + 0xee | 0xfa | 1/sqrt weighting, spare, magic dust |
| 13 to 19 | 12 + 0xf4 | 0x100 | nothing |

Magic dust has no effect in Ogun at any version (section 3). `autogun.rs`
also ignores the 1/sqrt weighting byte before version 14, so a version 13
state reads it but never weights its curves.

After the block, older versions also leave out articulators and curves.
The loader does not read them and resets them instead:

- Before version 4: articulators 4 and 5, unison pitch and phase
  (`DAT_0087d7a0` = 0x30 in the Autogun DLL).
- Before version 10: articulators 11 and 12, rotation and EQ shifting
  (`DAT_0087d7a1` = 0x18, counted from articulator 8, Autogun DLL).
- Before version 5: the forced-harmonics curve, which is reset flat at 0.

A reset articulator is taken to be the one Autogun's constructor makes,
every map flat at the value that changes nothing (`guess`, from the shape
of Autogun's `FUN_0087ed10`).

Before version 6 a curve's x counted partials at the richness it was
saved with. The loader converts it:

1. It scales every x by `(2^(order-1) - 1) / 32767`.
2. It adds a point at x = 1 at the curve's last level, so the curve holds
   that level to the end.
3. It resamples the curve at 16 points. Point `i` sits at `x = i/15` and
   takes the value at `2^i / 32767`.
4. It turns the log curve axis (+0x213) on.

After the resample FL calls `FUN_00784a40`, which drops redundant flat
points. `autogun.rs` leaves it out on the assumption that it does not
change the curve's shape (`guess`).

## 2. Parameters (`ProcessParam` @ 0x008684e0)

The 49 parameters, their ranges, defaults and derived values are
Autogun's (autogun.md section 2), with one exception. REC 0, the master
level, goes through `FUN_0086a570`, which computes `level3(v/128)` with
`level3(x) = (3^x - 1)/2`. Autogun's version multiplies that by 0.25, so
Ogun plays 4 times louder at the same wheel value (`code`).

The quality select (`FUN_00866840`) and its five settings are Autogun's.
Quality 0 differs from quality 1 only in its interpolator (section 3).

## 3. What differs from Autogun

The DLLs differ in four places (`code`):

1. The state has no random-preset seed word (section 1).
2. The master level has no `* 0.25` (section 2).
3. `Gen_Render` has no magic dust. The effect chain is EQ, chorus, delay,
   reverb and the oversampling filter (quality 2 and up), as in
   autogun.md section 5, with nothing after it.
4. The Dispatcher handles more editor opcodes. None of them touches the
   sound. Which opcodes they are was not recorded.

Two shared paths matter more for Ogun because only its corpus reaches
them: the old-version loader (section 1) and quality 0.

Quality 0 picks FL's linear interpolator, `FUN_00610ef0` in the Autogun
DLL, through the resampler table at `0x008d5d60`. The notes do not say
which DLL that table address belongs to. Quality 1 picks the 6-point
interpolator, `FUN_00611420` in the Autogun DLL. The linear one reads a
table at a fractional position as `a + (b - a) * frac`, from the sample at
the position and the one after it. The engine reads the newer and the
older table with it, then crossfades them over the frame as at quality 1
(autogun.md section 3). A quality above 4 changes nothing, so the
constructor's settings, which are quality 1's, stay.

The resampler's table of CPU-specific paths (`PTR_FUN_008d5dc0`) is taken
to give the same output as the generic path (`guess`).

## 4. What `ogun.rs` builds

`ogun.rs` defines `Ogun`, a newtype over `Autogun::new(Kind::Ogun)` that
forwards every `Generator` method. All the work is in the shared files:

- `autogun.rs` has `pub enum Kind { Autogun, Ogun }`. `State::parse_as(bytes,
  kind)` reads the seed for Autogun only. `State::parse` reads as Autogun.
  The parser reads versions 0 to 19 through `default_block`, `block_rest`,
  `blank_art`, `blank_shape`, `old_curve` and `add_point` (FL's
  `FUN_00786c60`). The master level takes the quarter for Autogun only.
  `build_sg` returns early for Ogun and `set_resource_dir` loads no
  `SG.dat`, so Ogun has no magic dust and needs no resource file.
- `autogun_engine.rs` has `Setup::linear` and `Timbre::linear`. The voice
  sets it when the quality's interpolator is the linear one (quality 0),
  and `Timbre::render` then reads with the linear interpolator.
- `fidelity()` reports Ogun's own messages. Of the qualities it flags
  only 2 to 4; quality 0 is not flagged.

The `ogun.rs` tests check that Ogun's output is 4 times Autogun's at the
same settings, that Ogun reports no latency and needs no `SG.dat`, that
every corpus state parses to the last byte with seed 0 and versions
spanning 0 to 19, that every project state plays a finite note at a sane
level, and that every preset plays with no NaN.

At the time of the handover, the engine corpus run went from 108 to 110
projects with sound. Silent Ogun channels went from 7 to 0, and hosted
generators from 163 to 170. Peaks: `Ogun.flp` 0.900, Electric Loop 1.000,
Astes 0.980.

Exact as read: everything autogun.md section 6 lists as exact, less the
magic dust; the master level; quality 0's linear interpolator; and the
loading of versions 0 to 12, every corpus state to the last byte.

Approximated, as for Autogun (autogun.md section 6): nothing is
null-tested against FL. IPP's `exp` is plain `exp`. A timbre seed 2 of 0
counts from 0 here and not from FL's process-wide counter. Qualities 2 to
4 keep the 6-point interpolator, so quality 4 lacks its longer one, and
they leave out the output oversampling filter. Portamento and
velocity-to-envelope mode 2 are left out. 16 voices.

Guessed: a reset articulator equals Autogun's `blank_state` one. Leaving
out `FUN_00784a40` keeps the curve's shape. The resampler's CPU-specific
paths match the generic one.

Unknown: the extra Dispatcher opcodes, `CreatePlugInstance` and the
destructor, and which DLL the resampler table address `0x008d5d60` is in.
