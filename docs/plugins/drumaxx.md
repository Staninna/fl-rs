# Drumaxx: state, pads, sequencer, mixer

Recovered from `Plugins/Fruity/Generators/Drumaxx/engine_x64.dll` (FL Studio
2026, 577 KB, MSVC C++, image base 0x180000000; its PDB path says
`Drumaxx_x64.dll.pdb`). `Drumaxx_x64.dll` beside it is the Delphi shell.
RTTI names `DMSynth` (the synth, vtable 0x180077f58), `DMPadGenerator` (a
pad's voice, vtable 0x180077f18), `DMInstance`, `DMEditor`, `CKickPad`,
and FL's C++ wrapper for its mobile synths, `CFLWrapper`, `CSynthesizer`
and `CPluginRenderer`, which Drumpad's, BassDrum's and MiniSynth's engines
share (`docs/plugins/drumpad.md`).

The code from 0x180001000 to 0x180040000 was decompiled with headless
Ghidra (`tools/ghidra/DecompileRange.java`), and each function cited below
was read again as asm with its rip-relative constants resolved (Capstone).
Neither output is in the repository.

Corpus: 135 presets (`Data/Patches/.../Drumaxx`, 130 `DMS3` and 5 `DMS4`)
and one state in a project, the render test
`System/Render tests/Drumaxx.flp` (`DMS4`, the "Default Kit"). Every state
is 21644 bytes. 86 presets turn the EQ on, 71 the limiter, 101 use cut
groups, 130 have patterns with steps set and 15 a pad with SOF on; none
routes a pad to another mixer track. The render test plays keys 36, 38 and
42, pads 0, 1 and 3, with no pattern.

## How the plugin is wired

Wrapper (`CFLWrapper`), as Drumpad's:

| function | address | what it does |
|---|---|---|
| ProcessParam | 0x1800034b0 | set: `int / 256`, clamped to 0..1; get: `int(256 v)` |
| slot fill | 0x180003e60 | per-key gains, pitch, cutoff and resonance from `FinalLevels` |
| key cleanup | 0x1800048d0 | lets a key's FL voice go once the synth does not keep it (`+0xa20`) and no note holds it |
| TriggerVoice | 0x180004980 | key `int(InitLevels.Pitch * 0.01)`, 0..127; velocity byte `int(127 log21(10 Vol + 1))` (`0x180005fa0`) |
| Voice_Release | 0x180004aa0 | the synth's note off on the key |

The slot fill stores the pitch of key `k` as
`FinalLevels.Pitch * 0.01 - k + 60` (`0x180078448` is 60), which with
`FinalLevels.Pitch = (key - 60) 100 + fine` is the note's fine pitch over
100: the binary bears out what Drumpad's doc took from `host-voice.md`.
When a key that already has an FL voice gets another, the wrapper sends the
synth a note off on it before the note on.

Synth (`DMSynth`, base `CSynthesizer`):

| function | address |
|---|---|
| constructor | 0x180031f40 |
| MIDI note: velocity table `+0x1068[key] = byte / 127`, then note on or off | 0x180007a20 |
| note on, note off (vtable `+0x130`, `+0x138`) | 0x180031cc0, 0x180031ce0 |
| key to patterns and pads | 0x180034e00 |
| pad strike and cut groups | 0x180035490 |
| render: gains, sequencer | 0x180034280 |
| mix of the pads | 0x180034990 |
| EQ and limiter | 0x180035840 |
| output gain | 0x180024870, gain `0x180025510` |
| parameter set (store `0x180008360`, side effects `0x180033770`) | 0x180033460 |
| parameter get | 0x180008440 |
| sample rate: voices, EQ bands | 0x180033910 |
| chunk load | 0x1800303a0 (`DMS1` `0x18002ec80`, `DMS2` `0x18002f8b0`) |

Voice (`DMPadGenerator`):

| function | address | Drumpad's |
|---|---|---|
| constructor, noise seed | 0x18002c5b0 | 0x18001f1a0 |
| parameters to patch | 0x18002c970 | 0x18001f4d0 |
| velocity blend | 0x180035700 | 0x1800229f0 |
| trigger, lane gains (vtable `+0x10`) | 0x18002d1a0 | 0x18001fca0 |
| reset (vtable `+0x20`) | 0x18002d620 | 0x1800201c0 |
| render at 38 kHz | 0x18002d8b0 | 0x180020460 |
| resampler and output | 0x18002e3b0 | 0x180020f50 |
| LOW highpass, RBJ biquad | 0x18001bd70, 0x18001bed0 | 0x180018bf0, 0x180018d50 |

## 1. State layout (event 213)

```
"SSLF"  u32 len  chunk[len]
chunk:  u32 magic  [char kit[256]]  f32 params[720]
        pattern[16] (0x402 bytes each)  char pad_names[16][128]
        u8 cut_group[16]  varint output[16]
pattern on disk: u8 last_step  u8 key  u8 cell[16][64]
```

The magic `DMS4` sits on disk as `4SMD`. The loader (`0x1800303a0`) reads
`DMS3`, `DMS4`, `DMS5` and the demo's `DMD3` to `DMD5` this way; `DMS1` and
`DMD1` go to `0x18002ec80`, `DMS2` and `DMD2` to `0x18002f8b0` (not read).
In order it:

1. reads a 256-byte kit name when `len - 4 > 0x100`;
2. sets the 720 parameters through the parameter setter, stopping and
   failing when fewer than 3 bytes are left;
3. reads 16 patterns, putting each one's 1024 cells first and its last step
   and key after them (`+0x400`, `+0x401`), which is how the synth reads
   them;
4. copies 0x800 bytes of pad names to synth `+0x673c` when that many are
   left;
5. reads up to 16 cut group bytes to synth `+0x8f4c`;
6. reads 16 output tracks as 7-bit varints, low group first, each through
   `0x180034930`, which moves the pad to another of the plugin's outputs;
7. for `DMS3` and `DMD3` only, divides each pad's parameter 6 (Retrigger)
   by 3 (`0x180078304`), clamped to 0..1.

A cell is a byte: the high bit strikes the pad on that step, the low seven
bits are its velocity. Cell `s` of pad `p` is byte `s + 64 p` of the
pattern.

## 2. Parameters

720 floats: 16 pads of 44 (`0x180031d20` is `44 n`), then 16 of the kit.
Pad `n`'s parameter `p` is `44 n + p`:

| `p` | what |
|---|---|
| 0 | volume: the setter stores `2 v^2` at synth `+0x42c0 + 4n` |
| 1 | pan: stored at `+0x4300 + 4n`; the pad's gains are `(1 - pan) vol` and `pan vol` |
| 2 | mute: not 0 is on |
| 3 | solo: not 0 is on; while any pad is soloed only soloed pads sound |
| 4 | the key that strikes the pad, a key number |
| 5..42 | Drumpad's parameters 0..37 (`docs/plugins/drumpad.md` section 2), `p - 5` there |
| 43 | not read |

`0x18002c970` reads every one of Drumpad's parameters at the pad's offset
plus 5, through the velocity blend `0x180035700`, with the same curves and
constants as Drumpad's `0x18001f4d0` (all compared, rip constants by
value), but two: Noise repeat and Retrigger are read with the literal
indexes 5 and 6 through the getter, not the pad's offset, so every pad uses
pad 0's.

The blend (`0x180035700`) reads slot `k`'s pick at pad `+0x23 + k` and its
target at `+0x27 + k`; a pick `m` not 0 moves parameter `m + 6` of the pad,
which is Drumpad's `m + 1`. The last slot that picks a parameter wins.

The kit's sixteen:

| index | what |
|---|---|
| 704 (0x2c0) | not read by the sound |
| 705 (0x2c1) | master volume: `+0x3900 = 10^(1.6 v) * 0.25` (`0x180033770`) |
| 706 (0x2c2) | swing: `clamp(v / 2, 0, 0.5)` |
| 707 (0x2c3) | EQ on; setting it clears the bands |
| 708..715 (0x2c4..0x2cb) | the bands' gains, `16 (v - 0.5)` dB |
| 716 (0x2cc) | limiter on; setting it resets the limiter |
| 717 (0x2cd) | limiter drive, `10^(1.5 v)` |
| 718 (0x2ce) | limiter release, `3 v^2 + 2` a second |
| 719 (0x2cf) | not read by the sound |

## 3. Notes and patterns

The wrapper turns FL's velocity into a byte as Drumpad's does and the synth
stores `byte / 127` (`0x1800782c4`) for the key (`0x180007a20`). A byte of
0 is a note off. That number is what the pads are struck at and what the
velocity slots blend by: Drumaxx has no square root where Drumpad's poly
allocator takes one.

Note on (`0x180034e00`, velocity above 0):

1. Every pattern whose key byte equals the key starts: synth `+0x6358` =
   key, `+0x6354` = pattern, `+0x6340` = 1 (start over), the key kept
   (`+0xa20[key] = 1`). A later pattern on the same key wins.
2. Every pad whose key (`int` of parameter 4) is the key, not muted and
   soloed when any pad is, is struck. Then, for each pad in order,
   `+0xa20[pad key]` is set to whether it was struck.

Note off: on the pattern's key only, the pattern stops (`+0x6354 = -1`,
start over), and `+0x6359` is set when it was running, so the key stays
kept while the pads ring on. Any other note off does nothing.

`+0x39b8` can confine a note to one pad and `+0x94c4` turns the next note
into a key assignment; both are the editor's and are off in playback.

Strike (`0x180035490`): the pad's voice gets the velocity and its pad
number, every other pad whose cut group is not 0 and the same as this
pad's gets its cut flag (`voice + 0x4c`), then the voice triggers
(`0x18002d1a0`, which clears its own cut flag). There is one voice a pad:
striking a pad that sounds starts it again where it is.

### The render (`0x180034280`)

At the start of each block:

1. For each pad whose voice sounds, `+0xa20[pad key] = 1`, and the pad's
   slot is the pattern key's when that key is kept, else its own key's.
   Its gain targets are `(1 - pan) vol slotL` and `pan vol slotR`, its
   bend `powf(2, clamp(semis + 12 * synth[+0x1468], -12, 12) / 12)`.
   `+0x1468` is 0 here. A pad that is not sounding keeps the targets it
   last had (0 from new), so a pad the sequencer strikes in the middle of
   a block takes those for the rest of it.
2. The pattern key is kept while the pattern runs, and after it stops
   while `+0x6359` and any pad sounds.
3. A start over sets the frame count to the next step and the step to 0.

Without a pattern the block is mixed. With one, the block is cut at the
steps: when the count reaches 0 the step's length is added to it,
`rate * 15 / tempo * (1 + swing)` on even steps and `* (1 - swing)` on odd
ones, truncated to whole frames, and each pad whose cell (step clamped to
64) has its high bit is struck at `(cell & 0x7f) / 127`, when not muted and
soloed if any pad is. The step then goes on by one, back to 0 past the
pattern's last step.

### The mix (`0x180034990`)

In passes of at most 512 frames: each pad's voice is read out
(`0x18002e3b0`) into the plugin's bus or, for a pad with an output track,
into that output. While any pad on the bus was sounding at the start of
the pass, the bus goes through `0x180035840` and onto the main output;
otherwise the EQ bands are cleared and the limiter reset. The demo timer
at `+0x6360` is negative in a registered plugin and does nothing.

`0x180035840`, per side:

- EQ, while 707 is on: eight RBJ biquads (`0x18001bed0`) at the host's
  rate, run from band 7 down to band 0. Band `b` is at `80 * 2^b` Hz
  (`0x180033910`), 2 octaves wide; band 0 a low shelf (type 0), bands 1 to
  6 peaks, band 7 a high shelf. As in the voices, `A = 10^(dB / 20)`, so a
  band's gain comes out at twice its dB. The run (`0x18001c270`) flushes
  its input and output under 1e-9 to 0. The low shelf, read from the type
  0 branch, is the cookbook's: `b0 = A ((A+1) - (A-1) cos + S)`,
  `b1 = 2A ((A-1) - (A+1) cos)`, `b2 = A ((A+1) - (A-1) cos - S)`,
  `a0 = (A+1) + (A-1) cos + S`, `a1 = -2 (A-1) - 2 (A+1) cos`,
  `a2 = (A+1) + (A-1) cos - S`, with `S = sin w sqrt(A) 2 sinh(...)`.
- Limiter, while 716 is on: the sample times the drive; a gain
  `r^4 (1 - h) + h` from a ramp `r` and a floor `h`; when the gain times
  the sample is over 1, `r = 0` and `h` and the gain become `1 / |x|`; the
  sample times the gain; then `r` moves on by the release over the rate,
  up to 1. Reset is `r = 0`, `h = 1`.

### The output (`0x180024870`)

The main output is multiplied by a gain per side that moves 0.005 of the
way to its target each frame, from 0 when the plugin is made. The target
(`0x180025510`) is `master * CC7 * 0.5`, panned by CC 10 and `+0x3904`,
which are 1, 0.5 and 0.5 unless MIDI moves them: `10^(1.6 v) / 8` on both
sides.

## 4. The voice

`0x18002c970`, `0x18002d1a0`, `0x18002d620`, `0x18002d8b0` and
`0x18002e3b0` were read against Drumpad's (`docs/plugins/drumpad.md`
section 4) operation by operation, and the rip constants of the first,
second and fourth compared by value with Drumpad's. The
arithmetic is the same: mallet, membrane lines and their shift scheme, SOF,
LOW, HIGH, MID with its envelope and retrigger, the clip, hold, lo-fi, the
silence rule, the 256-frame resampler and its 0.001 gain glide. What
differs:

- The cut flag: while it is set the hold envelope falls as it does after
  the hold time (`hold -= 0.001 hold`), from the next sample.
- The trigger works out the lane gains and the lines a note runs only when
  Phase, Tension, Material, Size or Shape changed (`+0x38a0`); the values
  are those Drumpad's work out each time.
- Each pad keeps one voice, and its 128 lines are never cleared by a
  trigger, only by the reset when the voice falls silent. A pad struck
  again while it rings runs on with what its lines hold, whichever lines
  the new note runs.
- The noise seeds come from the same global generator, 1000 steps a voice
  (`0x18002c5b0`), so pad `n`'s seed is Drumpad's voice `n`'s in the first
  plugin FL makes.

## 5. What `drumaxx.rs` builds

`crates/plugins/src/drumaxx.rs` is the synth; the pads are
`drumpad_voice::Voice`s made by `Voice::whole`, which keeps all 128 lines
apart as FL does, with the cut flag the shared voice now has (Drumpad
never sets it). Each pad's parameters go to `Patch::of` as Drumpad's 38
with Noise repeat and Retrigger taken from parameters 5 and 6.

Exact as read: the state loader with the `DMS3` Retrigger change, the
720 parameters and ProcessParam's `int / 256`, the patterns as FL holds
them, cut groups, the velocity as `byte / 127`, the key to pattern and pad
map with mute and solo, the key keeping (`+0xa20`) and the wrapper's key
cleanup, the per-key slot fill, the pad gains, pans and bends, the
sequencer with its swing and truncated steps, the 512-frame mix passes,
the EQ with its flushes, the limiter, the master gain and its glide, and
the voices.

Approximated:

- A pad with an output track other than 0 plays on the plugin's own bus,
  through the EQ, limiter and master gain, where FL sends it to another
  mixer track's output. No shipped state does; fidelity says so.
- A new Drumaxx starts from Drumpad's defaults on every pad, keys 36 to
  51, master 0.72, where FL loads its `dmkit` resource. Every state sets
  all of it.
- Each instance's noise starts where the first instance FL makes does.
- `DMS1` and `DMS2` states are turned down, where FL converts them.
- `tail()` is Drumpad's bound for the pad that rings longest.
- No render has been compared against FL's output.

Unknown:

- What `+0x1468` holds (taken as 0: a pitch bend, by its place after the
  CC table at `+0x1268`).
- What 704 and 719 do; nothing in the sound path reads them.
- How `0x18002ec80` and `0x18002f8b0` convert `DMS1` and `DMS2`.
