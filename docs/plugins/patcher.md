# Patcher

Patcher is a graph of plugins inside one effect slot or one channel: FL's
own plugins (and VSTs), each with its own state, wired sound to sound,
notes to notes and controller outputs to parameters, between a "From FL"
and a "To FL" side. `crates/plugins/src/patcher.rs` plays it and
`crates/plugins/src/patcher_state.rs` reads its state.

Read from `Plugins/Fruity/Effects/Patcher/Patcher_x64.dll` (FL Studio
26.1.7, image base 0x400000, Delphi), imported into Ghidra with analysis,
and checked against the 35 Patcher states FL ships: 30 in mixer slots and 3
on channels in 12 projects (Astes - Bien Duro has 10), and the two render
tests `Patcher (effect)` and `Patcher (generator)`. The generator build
(`Plugins/Fruity/Generators/Patcher/Patcher_x64.dll`) is the same code with
the same class layout: its `Eff_Render` is empty and its `Gen_Render`
(`0x94e280` there) runs what the effect's `Eff_Render` runs. Confidence
tags: `asm` (read in the decompile or the listing), `corpus` (seen in every
shipped state), `inferred`.

## 1. Classes

| class | VMT | what |
|---|---|---|
| `TTestPlug` | `0x945b48` | the plugin, parent `TDelphiFruityPlug`, 0x1c278 bytes |
| `TModularMap` (unit `ModularIO`) | `0x790440` | the graph, at plugin `+0x4228` |
| `TModularModule` | `0x78fb50` | a node |
| `TModularPlugin` (unit `ModularPlug`) | `0x7f8908` | a node that hosts a plugin, its wrapper at `+0x75c` |
| `TModularConnection` | `0x790010` | a wire, its gain an f32 at `+0x18` |

`TTestPlug`'s slots, in the FL SDK's order: Dispatcher `0x94c940`,
SaveRestoreState `0x956d50`, GetName `0x94f010`, ProcessEvent `0x953ed0`,
ProcessParam `0x954040`, Eff_Render `0x94ebd0`, Gen_Render `0x94eda0`
(empty in the effect), TriggerVoice `0x957900`, Voice_Release `0x957c00`,
Voice_Kill `0x957b90`, NewTick `0x953be0`, MIDITick `0x953bc0`, MIDIIn
`0x953b50`, MsgIn `0x953bd0`. The constructor `0x94b410` makes the map, its
"From FL" (`map+0x18`) and "To FL" (`map+0x20`) modules with an "Audio
input" and an "Audio output" port, and the plugin's 4096 parameter slots at
`+0x4238`, 0x18 bytes each.

## 2. The state (`0x956d50`, loader `0x9513d0`, saver `0x955150`) `asm` `corpus`

An i32 version: 5 and up are refused, 4 is every project's, 3 the render
tests'. From 4 the reader (`0x7c96e0`) takes text as UTF-16, before as one
byte a letter. Then a run of chunks, each an i32 tag, an i64 length and the
bytes; a tag 0 is a group header whose length covers the chunks after it,
and is read past. Groups do not nest: a plugin is the chunks from its
`0x1388` to its `0x1389`.

| tag | length | meaning |
|---|---:|---|
| `0x3e8` | 0x40 | i64 the next free id, i32 a global flag, i32 bits (bit 1 off: run the graph's layers on worker threads, `0x79e680`) |
| `0x3ea`, `0x3eb`, `0x3ec`, `0x5dc`, `0x640` | | the editor: window, page sizes, surface, zoom, a node's place (`0x640`: i64 node id, x, y) |
| `0x44c` | 0x20 | i64 a port id, i32 the Patcher parameter it is (`0x9529d0`, `0x956ce0`) |
| `0x514` / `0x515` | 0x20 / 0 | a wire: i64 the port it goes to, i64 the port it comes from, f32 gain, i32 2 (0 in older states), i32 0 sound, 1 notes, 2 a control (`0x952440`, `0x79b790`) |
| `0x578` / `0x57a` / `0x579` | 0x1c / n / 0 | a port of the Patcher's own (`0x952560`): i64 id, i32 kind (0 sound, 1 notes), i32 flags (bit 0 "From FL", bit 2 shown), a byte, i32 at byte 17: FL's buffer number, 0 the main input or output, `n` sidechain input `n` (`port+0x78`) or output `n` (`+0x68`), -1 FL's own notes; then its name |
| `0x1388` | 0x28 | a plugin (`0x952a70`): i32 kind (2 effect, 3 generator), i64 node id, i32 flags, i32 inputs, i32 outputs, window place |
| `0x138c` | n | the plugin: its event 201 name (version 4) or its DLL's path under FL (version 3), made a name by `0x94e9b0` |
| `0x1390`, `0x1392` | n | the name it was made under, and the name the patch shows |
| `0x138d` | n | the plugin's own state, what its event 213 would be |
| `0x138f` | 0x28 | f32 at `+0x8c` (1 in every state), i32 a program number (then `0x138e` names it), i32 flags (bit 0 at `+0x88`: the module renders nothing), i32 lowest and highest key it takes notes of (`+0x738`, `+0x73c`), i32 root key (`+0x740`), the window's place |
| `0x1391` | 0x1c | a port of the plugin: i32 kind (0 sound, 1 notes, 2 control), i32 index, i64 id, i32 flags |
| `0x3e9` | 0 | the end |

A Fruity Wrapper node takes its name from the wrapper's state. A node whose
kind is neither 2 nor 3 is refused with "The plugin you want to open is of
an unknown type", and one that does not load is left out with "Something
went wrong loading the %s plugin".

The node header's flags (`0x952a70`): bit 0 puts the plugin on the
Patcher's surface, bit 1 is handed to the wrapper when it is made, bit 2
keeps it off the worker threads (vfunc `+0x38`), bit 3 sets `+0x750` and
the wrapper's `+0xc49`. None of them changes what is played; bit 3, set on
the four plugins of Mehran Abbasi - With You's first Patcher, is taken to
be the wrapper's smart disable, which sleeps a silent plugin `inferred`.

A port's index is the plugin's input or output number for sound (0 its
main one) and notes, and for a control the parameter, with `0x1000_0000`
added for a controller output. Its flags (saver `0x955150`): bit 0 an
input, bit 1 shown (`+0x32`), bit 2 `+0x50`, bit 3 a control port of a
parameter that is not a float. Every effect node has its main input, its
main output and outputs 1 to 4, the ones it does not use hidden; a plugin
with a sidechain input has it as input 1 to 4 (Limiter's, flags 7). Hidden
sound inputs are not read (`0x79bf20` walks the shown ones, `+0x68`).

## 3. A block (`0x94ebd0` → `0x94a9b0`, `0x79cd80`, `0x94a5c0`) `asm`

1. Every "From FL" sound port (`+0x420c`) takes its buffer from FL:
   the host's `GetInBuf` (vtable `+0x120`) for its number, and the block
   when the number is 0 or FL has no buffer for it.
2. The graph (`0x79cd80`) sorts itself when it changed (`0x79afb0`), then
   runs its modules in order (`0x79d830`): each sound input is the sum of
   its wires, the first wire's source times its gain and each next one
   added times its gain, and silence with no wire (`0x79bf20`); then the
   module runs (vfunc `+0x78`).
3. Every "To FL" sound port (`+0x4214`) is added to FL's buffer for its
   number (`GetOutBuf`, vtable `+0x128`), the block for 0, which the
   effect clears first.

A plugin module (`0x7fdcd0`) first takes the events wired into it: notes
(`2` a note on, `3` off, `4` a voice event, with the key between `+0x738`
and `+0x73c`), `5` a parameter value (`0x7ffc30`, the wrapper's
`+0x1f0`), `6` MIDI. It clears its sound outputs, and unless `+0x88` is
set it renders (wrapper `+0xc0`, handed the input and output buffers):
an effect on its main input, its sidechain inputs and its second outputs
being the buffers the wrapper hands it for those numbers; a generator adds
its voices to silence.

### Order (`0x79daa0`, `0x79bd40`)

A module's layer is one past the highest layer of the modules wired into
any of its input ports, sound, notes or controls; a module with no wire in
is in layer 0 and one fed only from FL in layer 1. The modules run layer
by layer, and in the order the state lists them within a layer. A wire
that closes a loop would recurse without end; FL's editor does not make
one, and `patcher.rs` leaves such a wire out of the layers.

### Lateness (`0x79bc00`, `0x79bf20`, `0x79f0b0`)

A module's lateness is its own (`+0x38`, what its plugin reports) and the
most of the modules wired into any of its inputs (`+0x3c`). A sound wire
is held back by `(late(to) - own(to)) - late(from)` frames in a delay line
of its own (`0x79f350`), so every path into a module arrives together.
The Patcher reports the "To FL" module's lateness to FL (`0x94a5c0`,
host message `0x1e`).

## 4. Parameters and controls (`0x954040`, `0x7ffc30`) `asm`

The Patcher has 4096 parameters (`0x954040` takes 0 to 0xfff); chunk
`0x44c` makes parameter `n` a control port inside. `ProcessParam` with
`REC_FromMIDI` scales the value by 2^-30 and, for a parameter that is not
a float, onto the inner parameter's range: `min + Round(v * (max - min))`.
Without it the value is the inner parameter's own. Either way the inner
plugin is set to it, and `ProcessParam` hands back the value in that range.

A wire from a controller output to a parameter carries the output, 0 to 1,
as an event of kind 5 to the target module, which sets the parameter
through its wrapper as FL's link does: `REC_FromMIDI` in 2^30ths, the same
rounding as a Fruity Wrapper's VST parameters (`wrapper::from_vst`).
`NewTick` (`0x953be0` → `0x79be90`) ticks the modules that tick, in the
graph's order.

## 5. What fl-rs plays

`Patcher` (effect) and `PatcherGenerator` (channel) share one graph:

- Every node is made with `plugins::by_name` or `generator_by_name`, told
  the rate, tempo, PPQ and FL's plugin directory as the engine tells a
  slot or a channel, and handed its own state. One that is not built, or
  turns its state down, passes its main input to its main output and is
  named in `unhosted()`; the engine counts it among the passed slots
  (`crates/engine/src/mix.rs`) or the unhosted generators.
- Sound wires with their gains, the layers' order and the delay lines are
  as FL's. The buffers are 512 frames, sized at load; a longer block is
  played in parts.
- Second outputs: a plugin hands them with `Effect::outputs`, which is its
  `aux_output` at its `sidechain_output` number unless it says more:
  Stereo Shaper's difference, and Frequency Splitter's bands at their send
  levels to the outputs its targets name.
- Sidechain inputs into an inner effect are the ports numbered from 1, and
  the Patcher's own "From FL" sidechain ports are its
  `sidechain_inputs`, the block when the engine hands none.
- Controller wires are carried on every tick, in the graph's order;
  exposed parameters set the inner parameter in its own range.
- FL's notes go to the generators the "Notes" port is wired to, within
  their key range.

Left out, each said in `fidelity()`: "To FL" ports other than the main
output and MIDI ports. Also left out, and not said: a node's root key and
`+0x8c` (1 everywhere).

In FL's states, the plugins inside that are not built are Fruity X-Y-Z
Controller (32 nodes) and Distructor (2): their targets keep the values
the project saved, and Distructor's sound passes through. Fruity Formula
Controller (52 nodes) is built and runs inside.
