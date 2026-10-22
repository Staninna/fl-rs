# Morphine: what is left (2026-10-22)

Morphine is registered (`lib.rs`, `generator.rs`, `port.rs` at `Gaps`),
documented in `docs/plugins/morphine.md` with the DLL addresses, and its
`docs/plugins.md` row is filled in. `cargo test -p plugins morphine` with
`FL_RS_CORPUS` loads and plays all 310 shipped states (3 `.fst`, 7 project
channels, 300 `.mrp`) finite and non-silent, and counts how many switch on
each effect.

## Done in this pass

The effects (`morphine_fx.rs`) were checked against the DLL listing by
listing (`FUN_1800ff630` and everything under it; see the doc's Effects
table). Fixed to match: the tape's slowest step is `1/size`, not 0; the read
head may use the sample that crossed into the next pass (FL keeps it at the
end of the finished pass; the port used to read 0 there); the chorus tapes
are sized from the f32 0.02 widened (881 samples at 44.1 kHz, not 882); the
chorus switch only gates it, without a reset; the equaliser keeps FL's
`6.283185`. The idle path on `+0x584` in `FUN_1800ff630` never runs (the
counter is only ever zeroed) and is not ported.

## Left

- The noise layer (`morphine_noise.rs`, `FUN_180100300`, `FUN_180100880`,
  `FUN_180113b70`) and the jump a generator or envelope makes on a release
  inside its release loop (`FUN_1800ebc50`, `marker_value` in
  `morphine.rs`) are called approximate by the agent that wrote them. They
  were not rechecked; do that against the listings to move toward Exact.
- Unison and the per-note tuning a host sends are left out. Parameter 3
  (`int(12 v)` at `this+0x38dc`, `FUN_180111ec0` case 3) is not read by
  the port; it may be the unison count.
- No null test against FL.

## Tools

`morphine-tools/`: the first agent's scripts (chunk-tree parser `parse.py`,
`tree.py`, VMT walk `vt.py`, `dis.py` disassembly with constant-pool
values, `dat.py` constant dump). They need `pefile` and `capstone`; edit the
DLL path at the top (`/root/work/morphine/` on the machine they were
written on).
