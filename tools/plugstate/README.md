# plugstate

Scripts behind `docs/plugins/*.md`. Run with `python3 -I`.

- `pe.py`: read a qword, float or string at a virtual address in a PE, and find Delphi VMT self-pointers; the way from `CreatePlugInstance` to a plugin's method table.
- `dfm.py`: dump a binary Delphi DFM form resource; the wheels' Min/Max/Default/Tag give each parameter's range.
- `ripscan.py`: find RIP-relative writers of a `.bss` address, for constants computed at startup.
- `engine_emu.py`: run a C++ engine DLL's code under the unicorn emulator for reference values; as it stands it calls the matched biquad designer (`dispatch`) of Frequency Splitter's `engine_x64.dll`, which Fruity Parametric EQ 2's tests quote. Needs `pip install pefile unicorn` in a venv; the code it runs must not call an import.
- `dump213.py`: dump event 213 (plugin state) payloads per plugin name from `.flp` and `.fst` files. It guesses UTF-16 first, so an even-length 8-bit name such as "Fruity soft clipper" is missed; match names case-insensitively and try both encodings.

Method, per plugin: import the DLL into Ghidra, decompile `CreatePlugInstance` and its callees, follow the constructor to the class reference at VMT−200, list the VMT slots in FL SDK order, decompile `SaveRestoreState` (names the state block) and `ProcessParam` (the parameter maths), read the DFM for ranges, then the render function and whatever FLEngine host service it calls.
