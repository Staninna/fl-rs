# Fruity NoteBook, NoteBook 2 and HTML NoteBook: no sound path

The three notebooks keep text (or HTML) in a mixer slot. None of them has a
sound path: each sets `FPF_NoProcess` in its plugin info, which tells FL
never to call `Eff_Render`, and the `Eff_Render` slot of each one's method
table is Delphi's abstract-method error. FL's mixer passes the slot's block
on as it came, and so does fl-rs, with a `PassThrough`
(`crates/plugins/src/pass_through.rs`). Their pages and states are left out:
none of them reaches the sound.

Read from the three `_x64.dll` (FL Studio 26.1.7, Delphi, image base
0x400000) with capstone listings; tag `asm`.

| plugin | `CreatePlugInstance` | class ref, VMT | slot 7 `Eff_Render` | `TFruityPlugInfo` | Flags | params |
|---|---|---|---|---|---|---:|
| Fruity NoteBook | `0x6a5b30` | `0x6a5950`, `0x6a5a18` | `0x409ea0` | `0x6f4290` | `0x10400040` | 1 |
| Fruity NoteBook 2 | `0x6a7000` | `0x6a6df8`, `0x6a6ec0` | `0x409ea0` | `0x6f5f58` | `0x10400040` | 1 |
| Fruity HTML NoteBook | `0x6e0b80` | `0x6e0968`, `0x6e0a30` | `0x409ec0` | `0x73a6a8` | `0x10400040` | 0 |

Flags bit 6 (0x40) is `FPF_NoProcess`; bit 22 is `FPF_Reserved1` and bit 28
the bit every Delphi build of FL's plugins sets. The `Eff_Render` slots are
the RTL's abstract stub: `mov ecx, 0xd2` (runtime error 210, "abstract
method") and a call to the error handler, so FL cannot be calling them.
