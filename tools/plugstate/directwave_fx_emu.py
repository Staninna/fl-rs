"""Run DirectWave's three program effects under an emulator, for reference values.

    python3 -I directwave_fx_emu.py <DirectWave_x64.dll> <out.bin> [stub]

runs the delay (`FUN_180011f10`), the chorus (`FUN_1800119c0`) and the reverb
(`FUN_18002d0b0`) of FL Studio 2026.1.7's DirectWave on the cases at the end,
256 samples a step at 44.1 kHz and 120 BPM, and writes per case `i32 effect,
flag, blocks, 256, f32 knobs[4]`, then per step the sends left and right and
what the effect added to the dry buffers, left and right. `rand` is MSVC's
from 1, the allocators are stubbed, and the static initialisers' `2 pi` and
`pi` are set. With `stub` the libm calls are double maths rounded to float.
`directwave_fx.rs`'s test hashes the same outputs.

Needs `pip install pefile unicorn` in a venv.
"""
import math, struct, sys
import pefile
from unicorn import *
from unicorn.x86_const import *

pe = pefile.PE(sys.argv[1]); base = pe.OPTIONAL_HEADER.ImageBase
size = (pe.OPTIONAL_HEADER.SizeOfImage + 0xFFF) & ~0xFFF
STUB = len(sys.argv) > 3
STACK, RET, HEAP = 0x10000000, 0x30000000, 0x40000000
f32 = lambda x: struct.unpack('<f', struct.pack('<f', x))[0]

def machine():
    mu = Uc(UC_ARCH_X86, UC_MODE_64)
    mu.mem_map(base, size); mu.mem_write(base, pe.get_memory_mapped_image()[:size])
    mu.mem_map(STACK, 0x200000); mu.mem_map(RET, 0x1000); mu.mem_map(HEAP, 0x10000000)
    heap = [HEAP]; seed = [1]
    # The static initialisers' atanf(1) * 8 and * 4.
    for a, v in ((0x180109cc8, 8.0), (0x18011749c, 8.0), (0x18011c4c0, 8.0), (0x18011c4c4, 4.0)):
        mu.mem_write(a, struct.pack('<f', f32(f32(math.atan(1.0)) * v)))
    def alloc(n):
        p = (heap[0] + 63) & ~63; heap[0] = p + n + 64; mu.mem_write(p, bytes(n)); return p
    def ret(uc, v=None):
        rsp = uc.reg_read(UC_X86_REG_RSP); r = struct.unpack("<Q", uc.mem_read(rsp, 8))[0]
        uc.reg_write(UC_X86_REG_RSP, rsp + 8); uc.reg_write(UC_X86_REG_RIP, r)
        if v is not None: uc.reg_write(UC_X86_REG_RAX, v)
    def rand(uc):
        seed[0] = (seed[0] * 214013 + 2531011) & 0xffffffff
        ret(uc, (seed[0] >> 16) & 0x7fff)
    def xin(uc, k=0):
        return struct.unpack('<f', bytes(uc.reg_read(UC_X86_REG_XMM0 + k).to_bytes(16, 'little'))[:4])[0]
    def fret(uc, v):
        uc.reg_write(UC_X86_REG_XMM0, struct.unpack('<I', struct.pack('<f', v))[0]); ret(uc)
    stubs = {0x180052fd0: lambda uc: ret(uc, alloc(uc.reg_read(UC_X86_REG_RCX))),
             0x1800529e0: lambda uc: ret(uc, 0), 0x180052b00: lambda uc: ret(uc, 0),
             0x18008e000: rand}
    if STUB:
        stubs.update({
            0x1800893e0: lambda uc: fret(uc, f32(math.sin(xin(uc)))),
            0x180095680: lambda uc: fret(uc, f32(math.tan(xin(uc)))),
            0x180088010: lambda uc: fret(uc, f32(math.cos(xin(uc)))),
            0x180095140: lambda uc: fret(uc, f32(math.exp(xin(uc)))),
            0x180087a20: lambda uc: fret(uc, f32(math.log(xin(uc)))),
            0x180088510: lambda uc: fret(uc, f32(math.pow(xin(uc), xin(uc, 1)))),
        })
    def code(uc, addr, sz, _):
        f = stubs.get(addr)
        if f: f(uc)
    for a in stubs: mu.hook_add(UC_HOOK_CODE, code, begin=a, end=a)
    def unmapped(uc, access, addr, sz, value, _):
        print(f"unmapped {addr:#x} at {uc.reg_read(UC_X86_REG_RIP):#x}", file=sys.stderr); return False
    mu.hook_add(UC_HOOK_MEM_UNMAPPED, unmapped)
    return mu, alloc

FUNCS = {0: (0x180011f10, 0x80100, 0x28d, 0x2ee, 0x104c), 1: (0x1800119c0, 0x20100, 0x290, 0x30e, 0x184c),
         2: (0x18002d0b0, 0x200, 0x28f, 0x2fe, 0x204c)}

def lcg_inputs(n, seed):
    out = []
    for _ in range(n):
        seed = (seed * 1103515245 + 12345) & 0x7fffffff
        out.append(f32((seed / 0x7fffffff - 0.5) * 0.8))
    return out, seed

def run_case(out, effect, knobs, flag, blocks, rate=44100.0, tempo=120.0):
    mu, alloc = machine()
    fn, ssize, onoff, koff, sendoff = FUNCS[effect]
    state = alloc(ssize); ctx = alloc(0x3000); prog = alloc(0xd60)
    mu.mem_write(prog + onoff, b'\x01')
    if effect == 0: mu.mem_write(prog + 0x28e, bytes([flag]))
    mu.mem_write(prog + koff, struct.pack('<4f', *knobs))
    mu.mem_write(ctx + 8, struct.pack('<Q', prog)); mu.mem_write(ctx + 0x10, struct.pack('<i', 256))
    mu.mem_write(ctx + 0x14, struct.pack('<f', rate)); mu.mem_write(ctx + 0x24, struct.pack('<f', tempo))
    out.write(struct.pack('<iiii4f', effect, flag, blocks, 256, *knobs))
    seed = 7
    for b in range(blocks):
        l, seed = lcg_inputs(256, seed); r, seed = lcg_inputs(256, seed)
        if b >= blocks // 2: l = [0.0] * 256; r = [0.0] * 256   # let the tail ring
        mu.mem_write(ctx + sendoff, struct.pack('<256f', *l)); mu.mem_write(ctx + sendoff + 0x400, struct.pack('<256f', *r))
        mu.mem_write(ctx + 0x84c, bytes(0x800))
        rsp = STACK + 0x100000; mu.mem_write(rsp, struct.pack("<Q", RET)); mu.reg_write(UC_X86_REG_RSP, rsp)
        mu.reg_write(UC_X86_REG_RCX, state); mu.reg_write(UC_X86_REG_RDX, ctx)
        mu.emu_start(fn, RET)
        out.write(struct.pack('<256f', *l)); out.write(struct.pack('<256f', *r))
        out.write(bytes(mu.mem_read(ctx + 0x84c, 0x800)))

with open(sys.argv[2], 'wb') as out:
    run_case(out, 0, [0.25, 0.6, 0.1, 0.8], 0, 40)
    run_case(out, 0, [0.1, 0.7, 0.3, 0.6], 1, 40)
    run_case(out, 1, [0.4, 0.5, 0.3, 0.4], 0, 40)
    run_case(out, 2, [0.6, 0.4, 0.7, 0.5], 0, 40)
    run_case(out, 2, [1.0, 0.0, 0.0, 1.0], 0, 20)
print("done", file=sys.stderr)
