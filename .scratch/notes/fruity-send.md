# Fruity Send: what the engine must do

`crates/plugins/src/send.rs`, mapped in `docs/plugins/fruity-send.md`.

**Nothing new in `crates/engine/src/mix.rs`.** FL's Fruity Send adds its
send into the buffer the host hands it for an output number
(`host.GetOutBuffer`, host vtable +0x128), the same call Stereo Shaper makes
for its difference output. The engine already wires that call as
`Effect::aux_output` + `Effect::sidechain_output`, and `send.rs` uses it:

- **Which insert:** `sidechain_output()` is the saved target (state word 3),
  counting the inserts the plugin's own insert sends to, 1 the first,
  exactly as `second_output()` in `mix.rs` reads Stereo Shaper's number.
  0 or less (the corpus has 0 and -1) sends nothing and `aux_output()` is
  `None`.
- **At what level:** the plugin applies the level itself. `aux_output()` is
  the input times the pan/volume matrix (`LL, RR, RL, LR`), already ramped,
  so the host adds it at 1.0, as it does now.
- **Before or after the slot:** the send is made from the slot's input,
  before the dry level scales the slot's output. The host adds it to the
  destination insert's bus after the slot runs, in the same block, which is
  where `mix.rs` already adds an aux output.

So the coordinator only has to check that `mix.rs`'s parallel rewrite keeps
the aux path (`slot.aux`, `second_output`, the destination counted as
reached when a plugin's second output points at it).

Both projects that use Fruity Send (I'll Be Your Madonna, the render test)
have a target of -1 or 0, so in FL's own files it only scales by the dry
level (197 -> 0.497 in Madonna, 256 -> 1 in the render test).

Left open: the mixing routine behind the function pointer at 0x0071c4f8 was
not read; the port takes it to be Balance's ramped matrix.
