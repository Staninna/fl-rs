"""Run code of a C++ engine DLL under an emulator, for reference values.

    python3 -I engine_emu.py <engine_x64.dll> <kind> <f> <q> <dB> [...]

calls the `dispatch` export of Frequency Splitter's `engine_x64.dll` (the
matched biquad designer Fruity Parametric EQ 2 uses) with each group of
four arguments, as the plugin does (`f`, `q` and `dB` passed as floats), and
prints `b0 b1 b2 a1 a2`. The designer's own libm runs too, so the values
are FL's to the bit, up to the SSE paths an emulated CPU takes.

Needs `pip install pefile unicorn` in a venv. The image is mapped at its
preferred base, so no relocation is applied; the code it runs must not call
an import.
"""
import struct
import sys

import pefile
from unicorn import UC_ARCH_X86, UC_HOOK_MEM_UNMAPPED, UC_MODE_64, Uc
from unicorn.x86_const import (
    UC_X86_REG_RCX,
    UC_X86_REG_RDX,
    UC_X86_REG_RIP,
    UC_X86_REG_RSP,
    UC_X86_REG_XMM2,
    UC_X86_REG_XMM3,
)

STACK, OUT, RET = 0x10000000, 0x20000000, 0x30000000


def load(path):
    pe = pefile.PE(path)
    base = pe.OPTIONAL_HEADER.ImageBase
    size = (pe.OPTIONAL_HEADER.SizeOfImage + 0xFFF) & ~0xFFF
    mu = Uc(UC_ARCH_X86, UC_MODE_64)
    mu.mem_map(base, size)
    mu.mem_write(base, pe.get_memory_mapped_image()[:size])
    for at in (STACK, OUT, RET):
        mu.mem_map(at, 0x100000)

    def unmapped(uc, access, addr, size, value, _):
        rip = uc.reg_read(UC_X86_REG_RIP)
        print(f"unmapped access {addr:#x} at {rip:#x}", file=sys.stderr)
        return False

    mu.hook_add(UC_HOOK_MEM_UNMAPPED, unmapped)
    exports = {e.name.decode(): base + e.address for e in pe.DIRECTORY_ENTRY_EXPORT.symbols if e.name}
    return mu, exports


def f32_bits(value):
    return struct.unpack("<I", struct.pack("<f", value))[0]


def dispatch(mu, entry, kind, f, q, db):
    """`dispatch(kind, out, f, q, dB)`: out is b0 b1 b2 a1 a2 as doubles."""
    rsp = STACK + 0x80000
    mu.mem_write(rsp, struct.pack("<Q", RET))
    mu.mem_write(rsp + 0x28, struct.pack("<I", f32_bits(db)) + bytes(4))
    mu.mem_write(OUT, bytes(40))
    mu.reg_write(UC_X86_REG_RSP, rsp)
    mu.reg_write(UC_X86_REG_RCX, kind)
    mu.reg_write(UC_X86_REG_RDX, OUT)
    mu.reg_write(UC_X86_REG_XMM2, f32_bits(f))
    mu.reg_write(UC_X86_REG_XMM3, f32_bits(q))
    mu.emu_start(entry, RET)
    return struct.unpack("<5d", mu.mem_read(OUT, 40))


def main():
    mu, exports = load(sys.argv[1])
    args = sys.argv[2:]
    for i in range(0, len(args) - 3, 4):
        kind, f, q, db = int(args[i]), float(args[i + 1]), float(args[i + 2]), float(args[i + 3])
        print(" ".join(repr(v) for v in dispatch(mu, exports["dispatch"], kind, f, q, db)))


if __name__ == "__main__":
    main()
