"""Run Transporter's engine, `TransientLooperFaust` of `Shared/ildsp_x64.dll`,
under the unicorn emulator, for reference output.

    python3 -I transient_looper_emu.py <ildsp_x64.dll> <rate> <zones.json> <in.f32> <out.f32> [block] [dump]

`zones.json` maps a zone's offset in the Faust object (a hex string, as
`docs/plugins/transporter.md` lists them) to its float value. `in.f32` is
five interleaved float32 channels: the input's left and right, the
transient source's left and right, and the beat (the place in a 4-beat bar,
0 to 1). `out.f32` gets the four outputs interleaved: the main pair and the
second output's pair (loop A, loop B). The script calls `init(rate)`
(`classInit`, which fills the sine table, then `instanceConstants`,
`instanceResetUserInterface` and `instanceClear`), writes the zones, and
runs `compute` `block` frames at a time (64 when not given).

`operator new` and `delete` are answered by the script; the code it runs
calls no other import. The emulated CPU has no AVX, so the runtime's maths
take their SSE2 paths. Needs `pip install pefile unicorn`.
"""
import json
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
)

STACK, RET, HEAP, BUF, OBJ = 0x10000000, 0x30000000, 0x31000000, 0x50000000, 0x60000000
INIT, COMPUTE, VTABLE = 0x18018BAB0, 0x18018C4E0, 0x1803DF118
NEW, FREE = 0x1801AC9C0, 0x1801B34A8
SIZE = 0x1D507A8


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
    mu.mem_map(HEAP, 0x100000)
    mu.mem_map(BUF, 0x400000)
    mu.mem_map(OBJ, (SIZE + 0xFFFF) & ~0xFFFF)

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


def call(mu, fn, rcx, rdx=0, r8=0, r9=0):
    rsp = STACK + 0x80000
    mu.mem_write(rsp, struct.pack("<Q", RET))
    mu.reg_write(UC_X86_REG_RSP, rsp)
    for reg, v in ((UC_X86_REG_RCX, rcx), (UC_X86_REG_RDX, rdx), (UC_X86_REG_R8, r8), (UC_X86_REG_R9, r9)):
        mu.reg_write(reg, v)
    mu.emu_start(fn, RET)


def main():
    args = sys.argv[1:]
    mu = load(args[0])
    rate, zones, inp, outp = int(args[1]), args[2], args[3], args[4]
    block = int(args[5]) if len(args) > 5 else 64
    assert block <= 0x4000
    mu.mem_write(OBJ, struct.pack("<Q", VTABLE))
    call(mu, INIT, OBJ, rate)
    for offset, value in json.load(open(zones)).items():
        mu.mem_write(OBJ + int(offset, 16), struct.pack("<f", value))
    data = open(inp, "rb").read()
    n = len(data) // 20
    xs = struct.unpack(f"<{5 * n}f", data)
    ins = [BUF + 0x10000 * k for k in range(5)]
    outs = [BUF + 0x50000 + 0x10000 * k for k in range(4)]
    ptrs = BUF + 0x100000
    mu.mem_write(ptrs, struct.pack("<9Q", *ins, *outs))
    out = []
    dump = int(args[6]) if len(args) > 6 else -1
    for at in range(0, n, block):
        if at == dump:
            head = bytes(mu.mem_read(OBJ, 0x1610))
            mid = bytes(mu.mem_read(OBJ + 0xEA7610, 0xEA96B8 - 0xEA7610))
            open(outp + ".state", "wb").write(head + mid)
        m = min(block, n - at)
        for k in range(5):
            mu.mem_write(ins[k], struct.pack(f"<{m}f", *xs[5 * at + k : 5 * (at + m) : 5]))
        call(mu, COMPUTE, OBJ, m, ptrs, ptrs + 40)
        chans = [struct.unpack(f"<{m}f", mu.mem_read(outs[k], 4 * m)) for k in range(4)]
        for frame in zip(*chans):
            out += frame
    open(outp, "wb").write(struct.pack(f"<{len(out)}f", *out))


if __name__ == "__main__":
    main()
