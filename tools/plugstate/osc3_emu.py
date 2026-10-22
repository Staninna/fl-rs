"""Run the HQ table builder of 3x Osc's engine_x64.dll under an emulator.

    python3 -I osc3_emu.py <3x Osc/engine_x64.dll> <out.bin>

calls `FUN_180001310`, which `dispatch` 0 runs once, and writes the 176
tables it leaves at `DAT_180025ce0` (4 shapes of 44 levels) as records of
`i32 shape, i32 points` and `points + 2` little-endian f32. The engine's own
libm and float FFT run, so the tables are FL's to the bit, up to the SSE
paths an emulated CPU takes. The allocators and `__chkstk` are stubbed;
addresses are those of FL Studio 2026.1.7.

Needs `pip install pefile unicorn` in a venv; takes a few seconds.
"""
import struct, sys
import pefile
from unicorn import *
from unicorn.x86_const import *

pe = pefile.PE(sys.argv[1]); base = pe.OPTIONAL_HEADER.ImageBase
size = (pe.OPTIONAL_HEADER.SizeOfImage + 0xFFF) & ~0xFFF
mu = Uc(UC_ARCH_X86, UC_MODE_64)
mu.mem_map(base, size); mu.mem_write(base, pe.get_memory_mapped_image()[:size])
STACK, RET, HEAP = 0x10000000, 0x30000000, 0x40000000
mu.mem_map(STACK, 0x200000); mu.mem_map(RET, 0x1000); mu.mem_map(HEAP, 0x40000000)
heap = [HEAP]
def alloc(n, zero=False, align=16):
    p = (heap[0] + align - 1) & ~(align - 1); heap[0] = p + n + 64
    if zero: mu.mem_write(p, bytes(n))
    return p
def ret(uc, v=None):
    rsp = uc.reg_read(UC_X86_REG_RSP); r = struct.unpack("<Q", uc.mem_read(rsp, 8))[0]
    uc.reg_write(UC_X86_REG_RSP, rsp + 8); uc.reg_write(UC_X86_REG_RIP, r)
    if v is not None: uc.reg_write(UC_X86_REG_RAX, v)
stubs = {
    0x180013030: lambda uc: ret(uc),  # __chkstk
    0x180003580: lambda uc: ret(uc, alloc(uc.reg_read(UC_X86_REG_RCX))),
    0x180009a10: lambda uc: ret(uc, alloc(uc.reg_read(UC_X86_REG_RCX))),
    0x1800099f4: lambda uc: ret(uc, alloc(uc.reg_read(UC_X86_REG_RCX) * uc.reg_read(UC_X86_REG_RDX), True)),
    0x180009974: lambda uc: ret(uc, alloc(uc.reg_read(UC_X86_REG_RCX), False, uc.reg_read(UC_X86_REG_RDX))),
    0x1800099fc: lambda uc: ret(uc, 0), 0x180009958: lambda uc: ret(uc, 0), 0x1800035bc: lambda uc: ret(uc, 0),
}
def code(uc, addr, sz, _):
    f = stubs.get(addr)
    if f: f(uc)
for a in stubs: mu.hook_add(UC_HOOK_CODE, code, begin=a, end=a)
def unmapped(uc, access, addr, sz, value, _):
    print(f"unmapped {addr:#x} at {uc.reg_read(UC_X86_REG_RIP):#x}", file=sys.stderr); return False
mu.hook_add(UC_HOOK_MEM_UNMAPPED, unmapped)
rsp = STACK + 0x100000
mu.mem_write(rsp, struct.pack("<Q", RET)); mu.reg_write(UC_X86_REG_RSP, rsp)
mu.emu_start(0x180001310, RET)
q = lambda a: struct.unpack("<Q", mu.mem_read(a, 8))[0]
i32 = lambda a: struct.unpack("<i", mu.mem_read(a, 4))[0]
root = q(0x180025ce0)
shapes = i32(root); arr = q(root + 8)
out = open(sys.argv[2], "wb")
for s in range(shapes):
    mip = q(arr + 8 * s); levels = i32(mip); tabs = q(mip + 8); sizes = q(mip + 16)
    print("shape", s, "levels", levels, file=sys.stderr)
    for l in range(levels):
        n = i32(sizes + 4 * l); t = q(tabs + 8 * l)
        out.write(struct.pack("<ii", s, n)); out.write(bytes(mu.mem_read(t, 4 * (n + 2))))
