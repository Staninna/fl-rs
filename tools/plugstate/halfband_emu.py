"""Run FL's 2x resampler kernels under an emulator, for reference values.

    python3 -I halfband_emu.py "<FL Studio>/Plugins/Fruity/Effects/Fruity Delay Bank/Fruity Delay Bank_x64.dll"
    python3 -I halfband_emu.py "<FL Studio>/Plugins/Fruity/Effects/Fruity WaveShaper/Fruity WaveShaper_x64.dll"

Calls the SSE up and down kernels of the DLL's `TOversampler` stage and
the block flush after each, on the blocks `crates/dsp/src/halfband.rs`'s
test `the_dll_s_kernels_give_these_bits` runs, and prints the FNV-1a 64
hash of every output's f32 bits with the first few values. The two DLLs
hold the same code at other addresses (`KERNELS`).

The coefficient table the kernels read is what `FUN_006dba60` /
`FUN_008301d0` build at startup: each of the 13 doubles at `coefs` twice
over. The startup code calls the allocator, an import, so this builds the
table itself and stores its address where the kernels look for it.

Needs `pip install pefile unicorn` in a venv. The image is mapped at its
preferred base; the code run calls no import.
"""
import struct
import sys

import pefile
from unicorn import UC_ARCH_X86, UC_HOOK_MEM_UNMAPPED, UC_MODE_64, Uc
from unicorn.x86_const import UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RIP, UC_X86_REG_RSP

# up, down, flush, the table pointer, the 13 doubles.
KERNELS = {
    "Fruity Delay Bank_x64.dll": (0x006DB400, 0x006DB5D0, 0x005E27A0, 0x00746578, 0x00733FA0),
    "Fruity WaveShaper_x64.dll": (0x0082FB70, 0x0082FD40, 0x0062CA40, 0x008B2020, 0x0089EE70),
}

STACK, TABLE, STATE, BUF_IN, BUF_OUT, RET = 0x10000000, 0x11000000, 0x12000000, 0x13000000, 0x14000000, 0x30000000


def load(path):
    pe = pefile.PE(path)
    base = pe.OPTIONAL_HEADER.ImageBase
    size = (pe.OPTIONAL_HEADER.SizeOfImage + 0xFFF) & ~0xFFF
    mu = Uc(UC_ARCH_X86, UC_MODE_64)
    mu.mem_map(base, size)
    mu.mem_write(base, pe.get_memory_mapped_image()[:size])
    for at in (STACK, TABLE, STATE, BUF_IN, BUF_OUT, RET):
        mu.mem_map(at, 0x100000)

    def unmapped(uc, access, addr, size, value, _):
        rip = uc.reg_read(UC_X86_REG_RIP)
        print(f"unmapped access {addr:#x} at {rip:#x}", file=sys.stderr)
        return False

    mu.hook_add(UC_HOOK_MEM_UNMAPPED, unmapped)
    return mu


def call(mu, entry, *args):
    rsp = STACK + 0x80000
    mu.mem_write(rsp, struct.pack("<Q", RET))
    mu.reg_write(UC_X86_REG_RSP, rsp)
    for reg, value in zip((UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9), args):
        mu.reg_write(reg, value)
    mu.emu_start(entry, RET)


def blocks():
    """The test's blocks of stereo frames: a loud one, one under 2^-24,
    and silence."""
    loud = [(((n * 37) % 101 - 50) / 64.0, ((n * 53) % 97 - 48) / 32.0) for n in range(16)]
    tiny = [(l * 2.0**-28, r * 2.0**-28) for l, r in loud]
    return [loud, tiny, [(0.0, 0.0)] * 8]


def fnv(bits, h=0xCBF29CE484222325):
    for b in bits:
        for k in range(4):
            h ^= (b >> (8 * k)) & 0xFF
            h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def main():
    path = sys.argv[1]
    name = path.replace("\\", "/").rsplit("/", 1)[-1]
    up, down, flush, table_ptr, coefs = KERNELS[name]
    mu = load(path)
    doubles = struct.unpack("<13d", mu.mem_read(coefs, 13 * 8))
    mu.mem_write(TABLE, b"".join(struct.pack("<dd", d, d) for d in doubles))
    mu.mem_write(table_ptr, struct.pack("<Q", TABLE))
    up_state, down_state = STATE, STATE + 0x100
    mu.mem_write(STATE, bytes(0x200))
    wide_bits, narrow_bits = [], []
    for block in blocks():
        n = len(block)
        mu.mem_write(BUF_IN, b"".join(struct.pack("<ff", l, r) for l, r in block))
        call(mu, up, up_state, BUF_IN, BUF_OUT, n)
        call(mu, flush, up_state, 12)
        wide = mu.mem_read(BUF_OUT, 2 * n * 8)
        wide_bits += struct.unpack(f"<{4 * n}I", wide)
        mu.mem_write(BUF_IN, bytes(wide))
        call(mu, down, down_state, BUF_IN, BUF_OUT, n)
        call(mu, flush, down_state, 12)
        narrow_bits += struct.unpack(f"<{2 * n}I", mu.mem_read(BUF_OUT, n * 8))
    print(f"wide   {len(wide_bits)} values, fnv {fnv(wide_bits):#018x}, first {[hex(b) for b in wide_bits[:4]]}")
    print(f"narrow {len(narrow_bits)} values, fnv {fnv(narrow_bits):#018x}, first {[hex(b) for b in narrow_bits[:4]]}")


if __name__ == "__main__":
    main()
