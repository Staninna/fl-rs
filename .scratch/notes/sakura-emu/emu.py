"""Minimal unicorn harness over the Sakura engine DLL (copied from tools/plugstate/vst_emu.py)."""
import struct
import sys

import pefile
from unicorn import UC_ARCH_X86, UC_HOOK_CODE, UC_HOOK_MEM_UNMAPPED, UC_MODE_64, Uc
from unicorn.x86_const import (
    UC_X86_REG_MXCSR, UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RAX, UC_X86_REG_RCX,
    UC_X86_REG_RDX, UC_X86_REG_RIP, UC_X86_REG_RSP, UC_X86_REG_XMM0, UC_X86_REG_XMM1,
    UC_X86_REG_XMM2, UC_X86_REG_XMM3,
)

STACK, HEAP, STUBS, RET = 0x10000000, 0x40000000, 0x30000000, 0x30100000
PATH = '/root/work/sakura/dll/engine_x64.dll'


class Emu:
    def __init__(self, path=PATH):
        pe = pefile.PE(path)
        base = pe.OPTIONAL_HEADER.ImageBase
        size = (pe.OPTIONAL_HEADER.SizeOfImage + 0xFFF) & ~0xFFF
        mu = self.mu = Uc(UC_ARCH_X86, UC_MODE_64)
        mu.mem_map(base, size)
        mu.mem_write(base, pe.get_memory_mapped_image()[:size])
        for at, n in ((STACK, 0x200000), (HEAP, 0x20000000), (STUBS, 0x200000)):
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
        self.end = STUBS + 16 * n
        mu.mem_write(RET, b'\xf4')
        mu.hook_add(UC_HOOK_CODE, self._stub, begin=STUBS, end=self.end)
        mu.hook_add(UC_HOOK_MEM_UNMAPPED, self._unmapped)

    def _unmapped(self, uc, access, addr, size, value, _):
        print(f'unmapped access {addr:#x} at {uc.reg_read(UC_X86_REG_RIP):#x}', file=sys.stderr)
        return False

    def alloc(self, n):
        p = (self.heap + 15) & ~15
        self.heap = p + max(n, 16)
        self.mu.mem_write(p, bytes(n))
        return p

    def _stub(self, uc, addr, size, _):
        name = self.stubs.get(addr)
        if name == 'HeapAlloc':
            uc.reg_write(UC_X86_REG_RAX, self.alloc(uc.reg_read(UC_X86_REG_R8)))
        elif name == 'HeapReAlloc':
            uc.reg_write(UC_X86_REG_RAX, self.alloc(uc.reg_read(UC_X86_REG_R9)))
        elif name in ('GetProcessHeap', 'HeapFree'):
            uc.reg_write(UC_X86_REG_RAX, 1)
        else:
            uc.reg_write(UC_X86_REG_RAX, 0)

    def call(self, fn, *args, xmm=(), stack=()):
        mu = self.mu
        rsp = STACK + 0x100000
        for reg, a in zip((UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9), args):
            mu.reg_write(reg, a & 0xFFFFFFFFFFFFFFFF)
        for reg, v in zip((UC_X86_REG_XMM0, UC_X86_REG_XMM1, UC_X86_REG_XMM2, UC_X86_REG_XMM3), xmm):
            if v is not None:
                mu.reg_write(reg, v)
        for i, slot in enumerate(stack):
            mu.mem_write(rsp + 0x28 + 8 * i, slot)
        mu.mem_write(rsp, struct.pack('<Q', RET))
        mu.reg_write(UC_X86_REG_RSP, rsp)
        mu.reg_write(UC_X86_REG_MXCSR, 0x9F80)
        mu.emu_start(fn, RET)
        return mu.reg_read(UC_X86_REG_RAX)

    def xmm0(self):
        return self.mu.reg_read(UC_X86_REG_XMM0)

    def rd(self, at, n):
        return bytes(self.mu.mem_read(at, n))

    def wr(self, at, b):
        self.mu.mem_write(at, b)


def f32b(x):
    return struct.unpack('<I', struct.pack('<f', x))[0]


def f64b(x):
    return struct.unpack('<Q', struct.pack('<d', x))[0]


if __name__ == '__main__':
    e = Emu()
    e.call(0x1800579d0, xmm=(f32b(0.1),))
    v = e.xmm0()
    print('579d0(0.1f):', struct.unpack('<4f', v.to_bytes(16, 'little')))
    e.call(0x180057960, xmm=(f64b(0.1),))
    v = e.xmm0()
    print('57960(0.1):', struct.unpack('<2d', v.to_bytes(16, 'little')))
    e.call(0x18005a040, xmm=(f64b(0.1),))
    print('5a040(0.1):', struct.unpack('<2d', e.xmm0().to_bytes(16, 'little')))
    e.call(0x18005aac0, xmm=(f32b(0.1),))
    print('5aac0(0.1f):', struct.unpack('<4f', e.xmm0().to_bytes(16, 'little')))
    e.call(0x18005bbd0, xmm=(f32b(0.1),))
    print('5bbd0(0.1f):', struct.unpack('<4f', e.xmm0().to_bytes(16, 'little')))
    e.call(0x18005a5c0, xmm=(f32b(0.1),))
    print('5a5c0(0.1f):', struct.unpack('<4f', e.xmm0().to_bytes(16, 'little')))
    e.call(0x180044590, xmm=(f32b(10.0), f32b(0.5)))
    print('44590(10,0.5):', struct.unpack('<4f', e.xmm0().to_bytes(16, 'little')))
    e.call(0x180042ef0, xmm=(f64b(2.0), f64b(0.5)))
    print('42ef0(2,0.5):', struct.unpack('<2d', e.xmm0().to_bytes(16, 'little')))
