"""Run Emphasizer's limiter, `LimiterLite` of `Shared/ildsp_x64.dll`, under
the unicorn emulator, for reference output.

    python3 -I limiter_lite_emu.py <ildsp_x64.dll> <rate> <params> <in.f32> <out.f32> [block]

`params` is the seven values the shell hands `setParam` (input gain in dB,
emphasis, clipping, level matching, bypass, output gain in dB, the
switch), comma separated. `in.f32` and `out.f32` are interleaved stereo
float32. The script makes the limiter with `LimiterLite_CreateInstance`,
calls `init(rate, 0x800)` and `setParam` for each value, and runs
`process` `block` frames at a time (512 when not given).

`operator new` and `delete` are answered by the script; the code it runs
calls no other import. The emulated CPU has no AVX, so the runtime's maths
take their SSE2 paths. Needs `pip install pefile unicorn`.
"""
import struct
import sys

import pefile
from unicorn import UC_ARCH_X86, UC_HOOK_CODE, UC_HOOK_MEM_UNMAPPED, UC_MODE_64, Uc
from unicorn.x86_const import (
    UC_X86_REG_R8,
    UC_X86_REG_R9,
    UC_X86_REG_RAX,
    UC_X86_REG_RCX,
    UC_X86_REG_RDX,
    UC_X86_REG_RIP,
    UC_X86_REG_RSP,
    UC_X86_REG_XMM1,
    UC_X86_REG_XMM2,
)

STACK, RET, HEAP, BUF = 0x10000000, 0x30000000, 0x31000000, 0x50000000
CREATE, INIT, SET, PROCESS = 0x1801AA540, 0x1801AABA0, 0x1801AABB0, 0x1801AAE60
NEW, FREE = 0x1801AC9C0, 0x1801B34A8


def load(path):
    pe = pefile.PE(path)
    base = pe.OPTIONAL_HEADER.ImageBase
    size = (pe.OPTIONAL_HEADER.SizeOfImage + 0xFFF) & ~0xFFF
    image = pe.get_memory_mapped_image()[:size]
    mu = Uc(UC_ARCH_X86, UC_MODE_64)
    mu.mem_map(base, size)
    mu.mem_write(base, image)
    mu.mem_map(STACK, 0x100000)
    mu.mem_map(RET, 0x1000)
    mu.mem_map(HEAP, 0x1000000)
    mu.mem_map(BUF, 0x400000)

    def unmapped(uc, access, addr, size, value, _):
        print(f"unmapped access {addr:#x} at {uc.reg_read(UC_X86_REG_RIP):#x}", file=sys.stderr)
        return False

    mu.hook_add(UC_HOOK_MEM_UNMAPPED, unmapped)
    heap = [HEAP]

    def ret(uc, value=None):
        if value is not None:
            uc.reg_write(UC_X86_REG_RAX, value)
        rsp = uc.reg_read(UC_X86_REG_RSP)
        uc.reg_write(UC_X86_REG_RIP, struct.unpack("<Q", uc.mem_read(rsp, 8))[0])
        uc.reg_write(UC_X86_REG_RSP, rsp + 8)

    def new(uc, *_):
        at = heap[0]
        heap[0] += (uc.reg_read(UC_X86_REG_RCX) + 15) & ~15
        ret(uc, at)

    mu.hook_add(UC_HOOK_CODE, new, begin=NEW, end=NEW)
    mu.hook_add(UC_HOOK_CODE, lambda uc, *_: ret(uc), begin=FREE, end=FREE)
    return mu


def call(mu, fn, rcx=0, rdx=0, r8=0, r9=0, xmm1=None, xmm2=None):
    rsp = STACK + 0x80000
    mu.mem_write(rsp, struct.pack("<Q", RET))
    mu.reg_write(UC_X86_REG_RSP, rsp)
    for reg, v in ((UC_X86_REG_RCX, rcx), (UC_X86_REG_RDX, rdx), (UC_X86_REG_R8, r8), (UC_X86_REG_R9, r9)):
        mu.reg_write(reg, v)
    for reg, v in ((UC_X86_REG_XMM1, xmm1), (UC_X86_REG_XMM2, xmm2)):
        if v is not None:
            mu.reg_write(reg, struct.unpack("<I", struct.pack("<f", v))[0])
    mu.emu_start(fn, RET)
    return mu.reg_read(UC_X86_REG_RAX)


def main():
    args = sys.argv[1:]
    mu = load(args[0])
    rate, params, inp, outp = float(args[1]), [float(v) for v in args[2].split(",")], args[3], args[4]
    block = int(args[5]) if len(args) > 5 else 512
    assert block <= 0x10000
    obj = call(mu, CREATE)
    call(mu, INIT, obj, 0, 0x800, xmm1=rate)
    for index, value in enumerate(params[:6]):
        call(mu, SET, obj, index, xmm2=value)
    call(mu, 0x1801AAF20, obj, xmm1=params[6])
    data = open(inp, "rb").read()
    n = len(data) // 8
    xs = struct.unpack(f"<{2 * n}f", data)
    in_l, in_r, out_l, out_r, ptrs = BUF, BUF + 0x40000, BUF + 0x80000, BUF + 0xC0000, BUF + 0x100000
    mu.mem_write(ptrs, struct.pack("<4Q", in_l, in_r, out_l, out_r))
    out = []
    for at in range(0, n, block):
        m = min(block, n - at)
        mu.mem_write(in_l, struct.pack(f"<{m}f", *xs[2 * at : 2 * (at + m) : 2]))
        mu.mem_write(in_r, struct.pack(f"<{m}f", *xs[2 * at + 1 : 2 * (at + m) : 2]))
        call(mu, PROCESS, obj, ptrs, ptrs + 16, m)
        left = struct.unpack(f"<{m}f", mu.mem_read(out_l, 4 * m))
        right = struct.unpack(f"<{m}f", mu.mem_read(out_r, 4 * m))
        for a, b in zip(left, right):
            out += [a, b]
    open(outp, "wb").write(struct.pack(f"<{len(out)}f", *out))


if __name__ == "__main__":
    main()
