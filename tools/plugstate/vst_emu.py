"""Run an MSVC VST 2 engine DLL under the unicorn emulator, for reference output.

    python3 -I vst_emu.py <Vintage Phaser's engine_x64.dll> [rate] [block]

runs the DLL's static initialisers, `VSTPluginMain`, `setSampleRate` and the
36 parameters of Stroll Up's Vintage Phaser ("Lowgrounder") as the shell
hands them on, then `processReplacing` over the saw-and-noise input of
`vintage_phaser::tests::dll_input`, and prints every 16th output sample of
the first 2048, from the 16th, as f32 bits: the `DLL_LEFT` and `DLL_RIGHT`
the test compares with. The DLL's own libm runs too, so the values are FL's
to the bit, up to the paths an emulated CPU takes (its `pow` and `sin` pick
SSE2 code here where a newer CPU takes FMA3).

Imports are stubbed: a bump allocator behind `HeapAlloc`, nothing for the
critical sections. The CRT's start-up is skipped, so the security cookie and
the encoded `_matherr` pointer are cleared by hand, and MXCSR is set to FL's
0x9f80. The host callback answers `audioMasterVersion` and
`audioMasterGetTime` (`set_time`).

Needs `pip install pefile unicorn` in a venv.
"""
import struct
import sys

import pefile
from unicorn import UC_ARCH_X86, UC_HOOK_CODE, UC_HOOK_MEM_UNMAPPED, UC_MODE_64, Uc
from unicorn.x86_const import (
    UC_X86_REG_MXCSR, UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RAX, UC_X86_REG_RCX,
    UC_X86_REG_RDX, UC_X86_REG_RIP, UC_X86_REG_RSP, UC_X86_REG_XMM0, UC_X86_REG_XMM2,
)

STACK, HEAP, STUBS, RET, IO = 0x10000000, 0x40000000, 0x30000000, 0x30100000, 0x50000000

# Vintage Phaser's engine_x64.dll (FL Studio 26.1.7): the static
# initialisers' table, the security cookie and the encoded `_matherr`.
VPHASER_INIT = (0x180028280, 0x1800282F0)
VPHASER_CLEAR = (0x18003A008, 0x18003BCC8)

# The DFM's ranges, and Stroll Up's state.
RANGES = [(125, 2048000), (125, 2048000), (0, 6000), (0, 1), (125, 2048000), (0, 1),
          (1, 245760), (0, 1), (0, 1), (0, 6000), (0, 6000), (0, 6000), (0, 1), (0, 1),
          (0, 6000), (0, 6000), (1, 768), (0, 1), (0, 6000), (3200, 204800), (2000, 204800),
          (0, 100), (0, 1), (0, 6000), (0, 1), (0, 6000), (5000, 650000), (2000, 80000),
          (2000, 80000), (25, 150000), (25, 150000), (0, 6000), (0, 6000), (0, 1),
          (5000, 650000), (0, 1)]
STROLL_UP = [716925, 2048000, 1320, 0, 1182845, 0, 1, 0, 1, 3030, 0, 0, 0, 1, 0, 864, 1, 0,
             0, 31616, 5328, 100, 0, 0, 0, 2145, 77200, 58100, 80000, 66220, 25, 0, 0, 1,
             5000, 0]


def f32(x):
    return struct.unpack('<f', struct.pack('<f', x))[0]


def f32_bits(x):
    return struct.unpack('<I', struct.pack('<f', x))[0]


class Vst:
    def __init__(self, path, init=VPHASER_INIT, clear=VPHASER_CLEAR):
        pe = pefile.PE(path)
        base = pe.OPTIONAL_HEADER.ImageBase
        size = (pe.OPTIONAL_HEADER.SizeOfImage + 0xFFF) & ~0xFFF
        mu = self.mu = Uc(UC_ARCH_X86, UC_MODE_64)
        mu.mem_map(base, size)
        mu.mem_write(base, pe.get_memory_mapped_image()[:size])
        for at, n in ((STACK, 0x200000), (HEAP, 0x10000000), (STUBS, 0x200000), (IO, 0x100000)):
            mu.mem_map(at, n)
        self.heap = HEAP
        self.stubs = {}
        n = 0
        for dll in pe.DIRECTORY_ENTRY_IMPORT:
            for imp in dll.imports:
                stub = STUBS + 16 * n
                n += 1
                mu.mem_write(stub, b'\xc3')
                mu.mem_write(imp.address, struct.pack('<Q', stub))
                self.stubs[stub] = imp.name.decode() if imp.name else str(imp.ordinal)
        self.host = STUBS + 16 * n
        mu.mem_write(self.host, b'\xc3')
        mu.mem_write(RET, b'\xf4')
        mu.hook_add(UC_HOOK_CODE, self._stub, begin=STUBS, end=self.host)
        mu.hook_add(UC_HOOK_MEM_UNMAPPED, self._unmapped)
        for at in clear:
            mu.mem_write(at, bytes(8))
        self.exports = {e.name.decode(): base + e.address
                        for e in pe.DIRECTORY_ENTRY_EXPORT.symbols if e.name}
        self.set_time(-1.0, 120.0, False)
        for at in range(init[0], init[1], 8):
            entry = struct.unpack('<Q', mu.mem_read(at, 8))[0]
            if entry:
                self.call(entry)

    def _unmapped(self, uc, access, addr, size, value, _):
        print(f'unmapped access {addr:#x} at {uc.reg_read(UC_X86_REG_RIP):#x}', file=sys.stderr)
        return False

    def alloc(self, n):
        p = (self.heap + 15) & ~15
        self.heap = p + max(n, 16)
        self.mu.mem_write(p, bytes(n))
        return p

    def _stub(self, uc, addr, size, _):
        if addr == self.host:
            op = uc.reg_read(UC_X86_REG_RDX)
            uc.reg_write(UC_X86_REG_RAX, {1: 2400, 7: IO}.get(op, 0))
            return
        name = self.stubs.get(addr)
        if name in ('HeapAlloc',):
            uc.reg_write(UC_X86_REG_RAX, self.alloc(uc.reg_read(UC_X86_REG_R8)))
        elif name in ('HeapReAlloc',):
            uc.reg_write(UC_X86_REG_RAX, self.alloc(uc.reg_read(UC_X86_REG_R9)))
        elif name in ('GetProcessHeap', 'HeapFree'):
            uc.reg_write(UC_X86_REG_RAX, 1)
        else:
            uc.reg_write(UC_X86_REG_RAX, 0)

    def call(self, fn, *args, stack=()):
        """Call `fn` with integer `args` in RCX, RDX, R8, R9 and `stack`
        (raw 8-byte slots) from the fifth argument on."""
        mu = self.mu
        rsp = STACK + 0x100000
        for reg, a in zip((UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9), args):
            mu.reg_write(reg, a & 0xFFFFFFFFFFFFFFFF)
        for i, slot in enumerate(stack):
            mu.mem_write(rsp + 0x28 + 8 * i, slot)
        mu.mem_write(rsp, struct.pack('<Q', RET))
        mu.reg_write(UC_X86_REG_RSP, rsp)
        mu.reg_write(UC_X86_REG_MXCSR, 0x9F80)
        mu.emu_start(fn, RET)
        return mu.reg_read(UC_X86_REG_RAX)

    def set_time(self, ppq, tempo, playing):
        """The `VstTimeInfo` the host gives: ppqPos and tempo, and whether
        the song plays (flags 0x3e02, plus 2 when it does)."""
        flags = 0x3E00 | (2 if playing else 0)
        self.mu.mem_write(IO, struct.pack('<8d6i', 0, 44100, 0, ppq, tempo, 0, 0, 0, 4, 4, 0, 0,
                                          0, flags))

    def open(self, rate):
        self.effect = self.call(self.exports['VSTPluginMain'], self.host)
        q = lambda o: struct.unpack('<Q', self.mu.mem_read(self.effect + o, 8))[0]
        self.dispatcher, self.set_parameter, self.process_replacing = q(8), q(0x18), q(0x78)
        self.dispatch(0)
        self.dispatch(10, opt=rate)
        self.dispatch(11, value=512)
        self.dispatch(12, value=1)

    def dispatch(self, op, index=0, value=0, ptr=0, opt=0.0):
        return self.call(self.dispatcher, self.effect, op, index, value,
                         stack=(struct.pack('<Q', ptr), struct.pack('<f', opt) + bytes(4)))

    def set_param(self, index, v):
        self.mu.reg_write(UC_X86_REG_XMM2, f32_bits(v))
        self.call(self.set_parameter, self.effect, index)

    def process(self, left, right):
        n = len(left)
        mark = self.heap
        buf = self.alloc(16 * n)
        ptrs = self.alloc(32)
        ins, outs = (buf, buf + 4 * n), (buf + 8 * n, buf + 12 * n)
        self.mu.mem_write(ins[0], struct.pack(f'<{n}f', *left))
        self.mu.mem_write(ins[1], struct.pack(f'<{n}f', *right))
        self.mu.mem_write(ptrs, struct.pack('<4Q', *ins, *outs))
        self.call(self.process_replacing, self.effect, ptrs, ptrs + 16, n)
        self.heap = mark
        return [list(struct.unpack(f'<{n}f', self.mu.mem_read(o, 4 * n))) for o in outs]


def shell_value(index, value):
    """What the shell hands the engine: `(value - min) / (max - min)`, kept
    between 0 and 1, from f64 to f32."""
    lo, hi = RANGES[index]
    return f32(min(max((value - lo) * (1.0 / (hi - lo)), 0.0), 1.0))


def dll_input(n=2048):
    """`vintage_phaser::tests::dll_input`, in f32 arithmetic."""
    seed, left, right = 1, [], []
    for i in range(n):
        seed = (seed * 1664525 + 1013904223) & 0xFFFFFFFF
        noise = f32(f32((seed >> 8) / 16777216.0) - 0.5)
        saw = f32(f32(f32((i % 200) / 100.0) - 1.0) * f32(0.4))
        left.append(f32(saw + f32(noise * 0.25)))
        right.append(f32(f32(saw * -0.5) + f32(noise * 0.5)))
    return left, right


def main():
    rate = float(sys.argv[2]) if len(sys.argv) > 2 else 44100.0
    block = int(sys.argv[3]) if len(sys.argv) > 3 else 256
    vst = Vst(sys.argv[1])
    vst.open(rate)
    for index, value in enumerate(STROLL_UP):
        # The VST swaps 22 and 23; the shell sends by its own index.
        vst.set_param(index, shell_value(index, value))
    left, right = dll_input()
    out = [[], []]
    for at in range(0, len(left), block):
        l, r = vst.process(left[at:at + block], right[at:at + block])
        out[0] += l
        out[1] += r
    for side in out:
        print(', '.join(f'0x{f32_bits(v):08x}' for v in side[15::16]))


if __name__ == '__main__':
    main()
