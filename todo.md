# Todo

The next big steps, biggest payoff first. None of them needs FL Studio running: each is checked against FL's DLLs or against this project's own output.

1. **Run the heavy instruments' voices in parallel.** The mixer runs inserts side by side on its worker pool (`crates/engine/src/pool.rs`), but the projects that drop out are mostly one instrument doing all the work: JuiceBass - SeamlessR - Menagerie is 78% one Harmor, and Olbaid - Harmless Essentials is nearly all one Harmless. Each voice renders into its own buffer before the voices are mixed, so the voices of one plugin could go to the pool. This is what would take the worst projects from just about realtime to comfortably realtime.

2. **Take the work out of the first play.** The profile harness's cold round shows where pressing play drops out: Bombs Away ft KARRA - I'm Awake's slowest cold call is 1339% of its budget against 485% once warm, and Right Night feeling's is 751% against 147%. Warm each plugin up with a silent block at load and move its first-call setup there. The harness's cold columns show whether it worked.

3. **Close the biggest sound gaps the corpus render lists.** 123 of 165 projects have sound and 92 play in full.
   - FLEX: 13 silent channels. Decide first whether to read `.flexpack` or leave FLEX out.
   - Eight generators ignore `set_note_pitch`, so slides and pitch clips do not bend them: GMS, MiniSynth, Kepler, Kepler Exo, Autogun, Ogun, Morphine, FL Slayer.
   - The channel features under "Left out" in `docs/rec-channel.md`: instrument envelopes, time stretch, tracking, echo delay, TS404. Count how many corpus channels turn each on, as was done for the arpeggiator, and build the most used first.
   - Emphasizer's LimiterLite and Transporter's TransientLooper, both exported by `Shared/ildsp_x64.dll`. Their notes in `.scratch/notes/` have the steps.

4. **A per-plugin benchmark CI can run.** Time every built effect and generator on fixed generated input, with no FL files, so CI catches a plugin that got slower on any push. The README's bench block cannot: it changes only when someone runs the corpus locally.

5. **One harness that checks a port against its DLL.** A plugin is checked bit for bit against FL's own code only where a one-off emulator script exists (`tools/plugstate/halfband_emu.py`, `engine_emu.py`, the Sakura harness in `.scratch/notes/sakura-emu/`). One reusable harness that runs a DLL's process function under unicorn and compares it with the port would let any Gaps or approximate port be checked the same way.
