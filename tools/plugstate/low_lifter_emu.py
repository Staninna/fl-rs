"""Run Low Lifter's processor, `ILRDLowFreqEnhancer` of `Shared/ildsp_x64.dll`,
under the unicorn emulator, for reference output.

    python3 -I low_lifter_emu.py <ildsp_x64.dll> <rate> <zones.json> <in.f32> <out.f32> [block] [--fma]

`zones.json` maps a zone's offset in the Faust object (a hex string, as
`docs/plugins/low-lifter.md` lists them) to its float value. `in.f32` is
interleaved stereo float32; `out.f32` gets the same. The script calls
`instanceConstants(rate)`, `instanceResetUserInterface` and `instanceClear`,
writes the zones, and runs `compute` `block` frames at a time (64 when not
given).

The emulated CPU has no AVX, so the MSVC runtime's `tanf`, `expf` and `powf`
take their SSE2 paths. With `--fma` the script answers those three calls
itself from transcriptions of their FMA3 paths, which FL takes on a CPU with
AVX and FMA, as `crates/plugins/src/low_lifter.rs` does. `--check` instead
compares the SSE2 transcriptions with the emulated runtime over random
arguments.

Needs `pip install pefile unicorn` in a venv. The image is mapped at its
preferred base with no relocations; the code it runs calls no import.
"""
import json
import math
import random
import struct
import sys

import pefile
from unicorn import UC_ARCH_X86, UC_HOOK_CODE, UC_HOOK_MEM_UNMAPPED, UC_MODE_64, Uc
from unicorn.x86_const import (
    UC_X86_REG_R8,
    UC_X86_REG_R9,
    UC_X86_REG_RCX,
    UC_X86_REG_RDX,
    UC_X86_REG_RIP,
    UC_X86_REG_RSP,
    UC_X86_REG_XMM0,
    UC_X86_REG_XMM1,
)

STACK, RET, OBJ, BUF = 0x10000000, 0x30000000, 0x40000000, 0x50000000
CONSTS, RESET_UI, CLEAR, COMPUTE = 0x180177050, 0x1801772D0, 0x180177320, 0x180177F50
TANF, EXPF, POWF = 0x1801C9EF0, 0x1801C5F40, 0x1801C83B0
SIZE = 0x346B8


def f32(v):
    return struct.unpack("<f", struct.pack("<f", v))[0]


def f32_bits(v):
    return struct.unpack("<I", struct.pack("<f", v))[0]


def bits_f32(b):
    return struct.unpack("<f", struct.pack("<I", b & 0xFFFFFFFF))[0]


class Maths:
    """The runtime's tanf, expf and powf(10, y), for positive normal tangent
    arguments under pi/4 and exponents of a few units, as Low Lifter gives
    them."""

    def __init__(self, image, base):
        d = lambda a: struct.unpack("<d", image[a - base : a - base + 8])[0]
        self.table = [d(0x1803E3720 + 8 * i) for i in range(64)]
        self.ln10 = 3.0 * d(0x1803E25AC) + d(0x1803E2670 + 8 * 64)
        self.inv, self.step = d(0x1803E0E10), d(0x1803E0E20)

    def _scale(self, v, k):
        b = struct.unpack("<Q", struct.pack("<d", v))[0] + (k << 52)
        return struct.unpack("<d", struct.pack("<Q", b & (2**64 - 1)))[0]

    def pow10(self, y, fma):
        if y == 0.0:
            return 1.0
        if y == 1.0:
            return 10.0
        t = float(f32(y)) * self.ln10
        n = round(t * self.inv)
        tj = self.table[n & 63]
        if fma:
            r = math.fma(-float(n), self.step, t)
            res = math.fma(r * math.fma(math.fma(1 / 6, r, 0.5), r, 1.0), tj, tj)
        else:
            r = t - float(n) * self.step
            res = ((r * r) * ((1 / 6) * r + 0.5) + r) * tj + tj
        return f32(self._scale(res, n >> 6))

    def exp(self, x, fma):
        if x == float("-inf"):
            return 0.0
        xd = float(f32(x))
        n = round(xd * self.inv)
        tj = self.table[n & 63]
        if fma:
            r = math.fma(-float(n), self.step, xd)
            res = math.fma(math.fma(r * r, math.fma(1 / 6, r, 0.5), r), tj, tj)
        else:
            r = xd - float(n) * self.step
            res = ((r * r) * ((1 / 6) * r + 0.5) + r) * tj + tj
        return f32(res * 2.0 ** ((n - (n & 63)) >> 6))

    def tan(self, x, fma):
        xd = float(f32(x))
        a = abs(xd)
        assert 7.450580596923828e-09 <= a <= 0.7853981633974483, x
        x2 = xd * xd
        if a < 0.0001220703125:
            return f32(math.fma(x2 * xd, 1 / 3, xd) if fma else (x2 * xd) * (1 / 3) + xd)
        if fma:
            num = math.fma(-0.017203248047148168, x2, 0.3852960712639954)
            den = math.fma(math.fma(0.01844239256901656, x2, -0.5139650547885454), x2, 1.1558882143468838)
            return f32(math.fma(x2 * xd, num / den, xd))
        num = -0.017203248047148168 * x2 + 0.3852960712639954
        den = (0.01844239256901656 * x2 + -0.5139650547885454) * x2 + 1.1558882143468838
        return f32(xd + (x2 * xd) * (num / den))


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
    mu.mem_map(OBJ, (SIZE + 0xFFFF) & ~0xFFFF)
    mu.mem_map(BUF, 0x400000)

    def unmapped(uc, access, addr, size, value, _):
        print(f"unmapped access {addr:#x} at {uc.reg_read(UC_X86_REG_RIP):#x}", file=sys.stderr)
        return False

    mu.hook_add(UC_HOOK_MEM_UNMAPPED, unmapped)
    return mu, Maths(image, base)


def call(mu, fn, rcx, rdx=0, r8=0, r9=0):
    rsp = STACK + 0x80000
    mu.mem_write(rsp, struct.pack("<Q", RET))
    mu.reg_write(UC_X86_REG_RSP, rsp)
    for reg, v in ((UC_X86_REG_RCX, rcx), (UC_X86_REG_RDX, rdx), (UC_X86_REG_R8, r8), (UC_X86_REG_R9, r9)):
        mu.reg_write(reg, v)
    mu.emu_start(fn, RET)


def xmm(mu, reg):
    return bits_f32(mu.reg_read(reg))


def hook_maths(mu, maths):
    """Answer the three calls from the FMA3 transcriptions."""

    def ret(uc, v):
        uc.reg_write(UC_X86_REG_XMM0, f32_bits(v))
        rsp = uc.reg_read(UC_X86_REG_RSP)
        uc.reg_write(UC_X86_REG_RIP, struct.unpack("<Q", uc.mem_read(rsp, 8))[0])
        uc.reg_write(UC_X86_REG_RSP, rsp + 8)

    def powf(uc, *_):
        assert xmm(uc, UC_X86_REG_XMM0) == 10.0
        ret(uc, maths.pow10(xmm(uc, UC_X86_REG_XMM1), True))

    mu.hook_add(UC_HOOK_CODE, lambda uc, *_: ret(uc, maths.tan(xmm(uc, UC_X86_REG_XMM0), True)), begin=TANF, end=TANF)
    mu.hook_add(UC_HOOK_CODE, lambda uc, *_: ret(uc, maths.exp(xmm(uc, UC_X86_REG_XMM0), True)), begin=EXPF, end=EXPF)
    mu.hook_add(UC_HOOK_CODE, powf, begin=POWF, end=POWF)


def check(mu, maths, count=20000):
    def run(fn, a, b=None):
        mu.reg_write(UC_X86_REG_XMM0, f32_bits(a))
        if b is not None:
            mu.reg_write(UC_X86_REG_XMM1, f32_bits(b))
        call(mu, fn, 0)
        return f32_bits(xmm(mu, UC_X86_REG_XMM0))

    random.seed(3)
    apart = {"powf": 0, "expf": 0, "tanf": 0}
    for _ in range(count):
        y = f32(random.uniform(-6.5, 1.0))
        apart["powf"] += run(POWF, 10.0, y) != f32_bits(maths.pow10(y, False))
        x = f32(random.uniform(-0.05, 0.0))
        apart["expf"] += run(EXPF, x) != f32_bits(maths.exp(x, False))
        x = f32(random.uniform(1e-6, 0.06))
        apart["tanf"] += run(TANF, x) != f32_bits(maths.tan(x, False))
    print(f"{count} arguments each, SSE2 transcription apart from the runtime: {apart}")


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    mu, maths = load(args[0])
    if "--check" in sys.argv:
        check(mu, maths)
        return
    rate, zones, inp, outp = int(args[1]), args[2], args[3], args[4]
    block = int(args[5]) if len(args) > 5 else 64
    if "--fma" in sys.argv:
        hook_maths(mu, maths)
    call(mu, CONSTS, OBJ, rate)
    call(mu, RESET_UI, OBJ)
    call(mu, CLEAR, OBJ)
    for offset, value in json.load(open(zones)).items():
        mu.mem_write(OBJ + int(offset, 16), struct.pack("<f", value))
    data = open(inp, "rb").read()
    n = len(data) // 8
    xs = struct.unpack(f"<{2 * n}f", data)
    in_l, in_r, out_l, out_r, ptrs = BUF, BUF + 0x10000, BUF + 0x20000, BUF + 0x30000, BUF + 0x40000
    assert block <= 0x4000
    mu.mem_write(ptrs, struct.pack("<4Q", in_l, in_r, out_l, out_r))
    out = []
    for at in range(0, n, block):
        m = min(block, n - at)
        mu.mem_write(in_l, struct.pack(f"<{m}f", *xs[2 * at : 2 * (at + m) : 2]))
        mu.mem_write(in_r, struct.pack(f"<{m}f", *xs[2 * at + 1 : 2 * (at + m) : 2]))
        call(mu, COMPUTE, OBJ, m, ptrs, ptrs + 16)
        left = struct.unpack(f"<{m}f", mu.mem_read(out_l, 4 * m))
        right = struct.unpack(f"<{m}f", mu.mem_read(out_r, 4 * m))
        for a, b in zip(left, right):
            out += [a, b]
    open(outp, "wb").write(struct.pack(f"<{len(out)}f", *out))


if __name__ == "__main__":
    main()
