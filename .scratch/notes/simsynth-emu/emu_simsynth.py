"""Run SimSynth's whole engine (SimSynth_Eng_x64.dll) under unicorn through its
TFruityPlug interface, with a stub host, and print its output.

Usage: emu_simsynth.py <dll> <state.bin> <events> <frames> [block] [rate]
events: space separated on:<key>:<frame>[:<vel gain>[:<pan int>]] and off:<key>:<frame>
Prints one line per frame: left right (repr floats).

The host's ComputeLRVol is the circular law with the integer pan over 64
(docs/plugins/host-voice.md); LockMix/UnlockMix do nothing; Voice_Kill from
the plugin is answered with the plugin's own Voice_Kill after the block.
rand/srand are run here (MSVC's LCG, one state) because the CRT's per-thread
data is not set up.
"""
import math
import struct
import sys

import pefile
from unicorn import UC_ARCH_X86, UC_HOOK_CODE, UC_HOOK_MEM_UNMAPPED, UC_MODE_64, Uc
from unicorn.x86_const import (
    UC_X86_REG_MXCSR, UC_X86_REG_R8, UC_X86_REG_R9, UC_X86_REG_RAX, UC_X86_REG_RCX,
    UC_X86_REG_RDX, UC_X86_REG_RIP, UC_X86_REG_RSP,
)

STACK, HEAP, STUBS, RET, HOST = 0x10000000, 0x40000000, 0x30000000, 0x30100000, 0x30200000
RAND, SRAND = 0x18001dfc8, 0x18001dff4
VT_SAVE, VT_TRIGGER, VT_RELEASE, VT_KILL, VT_RENDER, VT_DISPATCH = (
    0x18000a610, 0x18000b700, 0x18000bc20, 0x18000bc50, 0x18000ae50, 0x18000a4a0)
CREATE = 0x18000be80


class Emu:
    def __init__(self, path):
        pe = pefile.PE(path)
        base = pe.OPTIONAL_HEADER.ImageBase
        size = (pe.OPTIONAL_HEADER.SizeOfImage + 0xFFF) & ~0xFFF
        mu = self.mu = Uc(UC_ARCH_X86, UC_MODE_64)
        mu.mem_map(base, size)
        mu.mem_write(base, pe.get_memory_mapped_image()[:size])
        for at, n in ((STACK, 0x200000), (HEAP, 0x20000000), (STUBS, 0x200000), (HOST, 0x10000)):
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
        # rand / srand: ret at once, the hook sets the result.
        self.holdrand = 1
        for at in (RAND, SRAND):
            mu.mem_write(at, b'\xc3')
        mu.hook_add(UC_HOOK_CODE, self._rand, begin=RAND, end=RAND)
        mu.hook_add(UC_HOOK_CODE, self._srand, begin=SRAND, end=SRAND)
        # malloc, realloc and free of the CRT.
        for at in (0x180043b94, 0x180020890, 0x18004a4a0):
            mu.mem_write(at, b'\xc3')
        mu.hook_add(UC_HOOK_CODE, self._malloc, begin=0x180043b94, end=0x180043b94)
        mu.hook_add(UC_HOOK_CODE, self._realloc, begin=0x180020890, end=0x180020890)
        # The host: an object whose vtable is 32 stubs.
        self.host = HOST
        vt = HOST + 0x100
        mu.write = mu.mem_write
        mu.mem_write(HOST, struct.pack('<Q', vt))
        for i in range(32):
            s = HOST + 0x400 + 16 * i
            mu.mem_write(vt + 8 * i, struct.pack('<Q', s))
            mu.mem_write(s, b'\xc3')
        mu.hook_add(UC_HOOK_CODE, self._host, begin=HOST + 0x400, end=HOST + 0x400 + 16 * 32)
        self.killed = []
        # The stream: a vtable of stubs, Read at +0x18 and Write at +0x20.
        self.stream = HOST + 0x800
        mu.mem_write(self.stream, struct.pack('<Q', HOST + 0x900))
        for i in range(8):
            s = HOST + 0xa00 + 16 * i
            mu.mem_write(HOST + 0x900 + 8 * i, struct.pack('<Q', s))
            mu.mem_write(s, b'\xc3')
        mu.hook_add(UC_HOOK_CODE, self._streamhook, begin=HOST + 0xa00, end=HOST + 0xa00 + 16 * 8)
        self.data = b''
        self.pos = 0
        import os
        if os.environ.get('TRACE'):
            from unicorn import UC_HOOK_BLOCK
            self.hist = []
            def blk(uc, a, s, _):
                self.hist.append(a)
                if len(self.hist) > 400:
                    self.hist.pop(0)
            mu.hook_add(UC_HOOK_BLOCK, blk)

    def _unmapped(self, uc, access, addr, size, value, _):
        print(f'unmapped access {addr:#x} at {uc.reg_read(UC_X86_REG_RIP):#x}', file=sys.stderr)
        return False

    def alloc(self, n):
        p = (self.heap + 15) & ~15
        self.heap = p + max(n, 16)
        self.mu.mem_write(p, bytes(n))
        return p

    def rd(self, at, n):
        return bytes(self.mu.mem_read(at, n))

    def _stub(self, uc, addr, size, _):
        name = self.stubs.get(addr)
        if name == 'HeapAlloc':
            uc.reg_write(UC_X86_REG_RAX, self.alloc(uc.reg_read(UC_X86_REG_R8)))
        elif name == 'HeapReAlloc':
            old = uc.reg_read(UC_X86_REG_R8)
            n = uc.reg_read(UC_X86_REG_R9)
            p = self.alloc(n)
            if old:
                uc.mem_write(p, bytes(uc.mem_read(old, n)))
            uc.reg_write(UC_X86_REG_RAX, p)
        elif name == 'HeapSize':
            uc.reg_write(UC_X86_REG_RAX, 0x10000)
        elif name in ('GetProcessHeap', 'HeapFree'):
            uc.reg_write(UC_X86_REG_RAX, 1)
        else:
            uc.reg_write(UC_X86_REG_RAX, 0)

    def _malloc(self, uc, addr, size, _):
        uc.reg_write(UC_X86_REG_RAX, self.alloc(uc.reg_read(UC_X86_REG_RCX)))

    def _realloc(self, uc, addr, size, _):
        old, n = uc.reg_read(UC_X86_REG_RCX), uc.reg_read(UC_X86_REG_RDX)
        p = self.alloc(n)
        if old:
            uc.mem_write(p, bytes(uc.mem_read(old, n)))
        uc.reg_write(UC_X86_REG_RAX, p)

    def _rand(self, uc, addr, size, _):
        self.holdrand = (self.holdrand * 214013 + 2531011) & 0xFFFFFFFF
        uc.reg_write(UC_X86_REG_RAX, (self.holdrand >> 16) & 0x7FFF)

    def _srand(self, uc, addr, size, _):
        self.holdrand = uc.reg_read(UC_X86_REG_RCX) & 0xFFFFFFFF

    def _host(self, uc, addr, size, _):
        slot = (addr - HOST - 0x400) // 16
        if slot == 3:
            lp, rp = uc.reg_read(UC_X86_REG_RDX), uc.reg_read(UC_X86_REG_R8)
            pan = struct.unpack('<i', struct.pack('<I', uc.reg_read(UC_X86_REG_R9) & 0xFFFFFFFF))[0]
            rsp = uc.reg_read(UC_X86_REG_RSP)
            vol = struct.unpack('<f', bytes(uc.mem_read(rsp + 0x28, 4)))[0]
            p = max(-1.0, min(1.0, pan / 64.0))
            a = (1.0 + p) * 0.7853981633974483
            uc.mem_write(lp, struct.pack('<f', math.cos(a) * vol))
            uc.mem_write(rp, struct.pack('<f', math.sin(a) * vol))
        elif slot == 5:
            self.killed.append(uc.reg_read(UC_X86_REG_RDX))
        elif slot == 0:
            uc.reg_write(UC_X86_REG_RAX, HOST + 0xf000)
            return
        uc.reg_write(UC_X86_REG_RAX, 0)

    def _streamhook(self, uc, addr, size, _):
        slot = (addr - HOST - 0xa00) // 16
        buf, n = uc.reg_read(UC_X86_REG_RDX), uc.reg_read(UC_X86_REG_R8) & 0xFFFFFFFF
        if slot == 3:
            chunk = self.data[self.pos:self.pos + n]
            chunk += bytes(n - len(chunk))
            self.pos += n
            uc.mem_write(buf, chunk)
        uc.reg_write(UC_X86_REG_RAX, 0)

    def call(self, fn, *args, stack=()):
        mu = self.mu
        rsp = STACK + 0x100000
        for reg, a in zip((UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9), args):
            mu.reg_write(reg, a & 0xFFFFFFFFFFFFFFFF)
        for i, slot in enumerate(stack):
            mu.mem_write(rsp + 0x28 + 8 * i, slot)
        mu.mem_write(rsp, struct.pack('<Q', RET))
        mu.reg_write(UC_X86_REG_RSP, rsp)
        mu.reg_write(UC_X86_REG_MXCSR, 0x9F80)
        try:
            mu.emu_start(fn, RET)
        except Exception:
            print([hex(h) for h in [h for h in getattr(self, "hist", []) if h < 0x180020000][-30:]], file=sys.stderr)
            raise
        return mu.reg_read(UC_X86_REG_RAX)


def main():
    e = Emu(sys.argv[1])
    state = open(sys.argv[2], 'rb').read()
    events = []
    for ev in sys.argv[3].split():
        parts = ev.split(':')
        events.append((int(parts[2]), parts[0], int(parts[1]),
                       float(parts[3]) if len(parts) > 3 else 1.0, int(parts[4]) if len(parts) > 4 else 0))
    events.sort(key=lambda x: (x[0], x[1] != 'off'))
    frames = int(sys.argv[4])
    block = int(sys.argv[5]) if len(sys.argv) > 5 else 512
    rate = int(sys.argv[6]) if len(sys.argv) > 6 else 44100
    idx = lambda: struct.unpack('<H', e.rd(0x1800dc4a0, 2))[0]
    plug = e.call(CREATE, e.host, 0)
    print('noise idx after create', idx(), file=sys.stderr)
    e.call(VT_DISPATCH, plug, 4, 0, rate)
    print('noise idx after rate', idx(), file=sys.stderr)
    e.data, e.pos = state, 0
    e.call(VT_SAVE, plug, e.stream, 0)
    print('noise idx after load', idx(), 'holdrand', e.holdrand, file=sys.stderr)
    import os
    if os.environ.get('DUMPTAB'):
        open(os.environ['DUMPTAB'], 'wb').write(e.rd(0x1800fc4f0, 4 * 30020) + e.rd(0x18009c4a0, 4 * 65536) + e.rd(0x1800dc4b0, 2 * 65536) + e.rd(plug + 0x258 + 0, 8))
    voices = {}
    handles = {}
    out = []
    buf = e.alloc(8 * block + 64)
    lenp = e.alloc(16)
    done = 0
    while done < frames:
        while events and events[0][0] <= done:
            _, kind, key, vol, pan = events.pop(0)
            if kind == 'on':
                params = e.alloc(0x28)
                lv = struct.pack('<ififf', pan, vol, (key - 60) * 100, 0.0, 0.0)
                e.mu.mem_write(params, lv + lv)
                voices[key] = e.call(VT_TRIGGER, plug, params, 1000 + key)
                handles[1000 + key] = voices[key]
            elif key in voices:
                h = voices.pop(key)
                if h in handles.values():
                    e.call(VT_RELEASE, plug, h)
        n = min(block, frames - done)
        if events:
            n = min(n, events[0][0] - done)
        e.mu.mem_write(buf, bytes(8 * n))
        e.mu.mem_write(lenp, struct.pack('<i', n))
        e.call(VT_RENDER, plug, buf, lenp)
        vals = struct.unpack('<%df' % (2 * n), e.rd(buf, 8 * n))
        for i in range(n):
            out.append((vals[2 * i], vals[2 * i + 1]))
        for h in e.killed:
            if h in handles:
                e.call(VT_KILL, plug, handles.pop(h))
        e.killed = []
        done += n
    for l, r in out:
        print(repr(l), repr(r))


if __name__ == '__main__':
    main()
