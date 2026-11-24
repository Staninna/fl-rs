"""Run Sawer's SawerSynth (engine_x64.dll) under unicorn and print its output.

Usage: emu_sawer.py <state.bin (event 213)> "<events>" <frames> [block] [rate]
events: on:<key>:<frame>[:<velocity>] off:<key>:<frame>
Prints one line per frame: left right.
"""
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from emu import Emu, f64b  # noqa: E402
from unicorn import UC_HOOK_CODE  # noqa: E402
from unicorn.x86_const import UC_X86_REG_R8, UC_X86_REG_RAX, UC_X86_REG_RDX  # noqa: E402

DATA = os.environ.get('SAWER_DATA', '/root/fl-install/Plugins/Fruity/Generators/Sawer/data.saw')

state = open(sys.argv[1], 'rb').read()
evs = []
for ev in sys.argv[2].split():
    p = ev.split(':')
    evs.append((int(p[2]), p[0], int(p[1]), int(p[3]) if len(p) > 3 else 100))
evs.sort(key=lambda x: (x[0], x[1] != 'off'))
frames = int(sys.argv[3])
block = int(sys.argv[4]) if len(sys.argv) > 4 else 512
rate = float(sys.argv[5]) if len(sys.argv) > 5 else 44100.0

e = Emu()
data = open(DATA, 'rb').read()
# The data file: "exists" and "read" answered here.
e.wr(0x180019f10, b'\xc3')
e.wr(0x18001a100, b'\xc3')
# The preset list reads files: skip it.
e.wr(0x180031d60, b'\xc3')


def _exists(uc, addr, size, _):
    uc.reg_write(UC_X86_REG_RAX, 1)


def _read(uc, addr, size, _):
    buf, n = uc.reg_read(UC_X86_REG_RDX), uc.reg_read(UC_X86_REG_R8) & 0xffffffff
    uc.mem_write(buf, data[:n])
    uc.reg_write(UC_X86_REG_RAX, 1)


e.mu.hook_add(UC_HOOK_CODE, _exists, begin=0x180019f10, end=0x180019f10)
e.mu.hook_add(UC_HOOK_CODE, _read, begin=0x18001a100, end=0x18001a100)
desc = 0x1800a5fd0
e.wr(0x180089968, struct.pack('<Q', desc))
e.wr(desc + 0x118, struct.pack('<5I', 0x60, 0x49, 8, 0x18, 0x100))
this = e.alloc(0x8000)
e.call(0x180031680, this, 0)
rng = lambda: struct.unpack('<II', e.rd(0x180086e30, 8))
if os.environ.get('SAWER_INFO'):
    print('rng after ctor', rng(), file=sys.stderr)
    tab = struct.unpack('<Q', e.rd(this + 0x1040, 8))[0]
    vals = [struct.unpack('<f', e.rd(tab + 0x94 * i + 4, 4))[0] for i in range(96)]
    print('defaults', vals, file=sys.stderr)
# Sample rate: slot 10 with the rate in xmm1.
vt = struct.unpack('<Q', e.rd(this, 8))[0]
slot = lambda n: struct.unpack('<Q', e.rd(vt + 8 * n, 8))[0]
# Polyphony: slot 53 with 0, the most.
e.call(slot(53), this, 0)
print('voices', struct.unpack('<3i', e.rd(this + 0x38e4, 12)), file=sys.stderr)
e.call(slot(10), this, xmm=(None, f64b(rate)))
if os.environ.get('SAWER_INFO'):
    print('rng after rate', rng(), file=sys.stderr)
# State: the engine's chunk after SSLF and its length.
chunk = state[8:]
buf = e.alloc(len(chunk))
e.wr(buf, chunk)
e.call(slot(36), this, buf, len(chunk))
if os.environ.get('SAWER_INFO'):
    print('rng after load', rng(), file=sys.stderr)
c = struct.pack('<f', 0.7071067811865476)
for n in range(128):
    e.wr(this + 0x20 + 4 * n, c)
    e.wr(this + 0x220 + 4 * n, c)
    e.wr(this + 0x420 + 4 * n, struct.pack('<f', 0.0))
obj = e.alloc(0x20)
out_buf = e.alloc(8 * block + 64)
e.wr(this + 0x1060, struct.pack('<Q', obj))
note = slot(1)
trace = []
cur = [0]
if os.environ.get('SAWER_TRACE'):
    from unicorn.x86_const import UC_X86_REG_XMM1, UC_X86_REG_XMM0
    def _in(uc, addr, size, _):
        trace.append(('in', struct.unpack('<f', struct.pack('<I', uc.reg_read(UC_X86_REG_XMM1) & 0xffffffff))[0]))
    e.mu.hook_add(UC_HOOK_CODE, _in, begin=0x18001a9e0, end=0x18001a9e0)
    def _stage(name):
        def h(uc, addr, size, _):
            vals = struct.unpack('<4f', bytes(uc.mem_read(out_buf, 16)))
            vals2 = struct.unpack('<4f', bytes(uc.mem_read(out_buf + 4 * cur[0], 16)))
            trace.append((name, vals, vals2))
        return h
    from unicorn.x86_const import UC_X86_REG_R8 as R8
    def _voice(uc, addr, size, _):
        from unicorn.x86_const import UC_X86_REG_RSP as RSP
        v = struct.unpack('<Q', bytes(uc.mem_read(uc.reg_read(RSP) + 0x28, 8)))[0]
        if len([x for x in trace if x[0] == 'voice']) < 3:
            f = lambda o: struct.unpack('<f', bytes(uc.mem_read(v + o, 4)))[0]
            i = lambda o: struct.unpack('<I', bytes(uc.mem_read(v + o, 4)))[0]
            d = lambda o: struct.unpack('<d', bytes(uc.mem_read(v + o, 8)))[0]
            trace.append(('voice', [i(0x58), i(0x5c), i(0x60), i(0x64), i(0x68), i(0x6c), i(0x70), f(0x74), f(0x7c), f(0x9c), f(0xa0), f(0xa4), f(0xb8), f(0xbc), f(0xc0), f(0xd4), f(0xd8), f(0xdc), f(0xe0), f(0xe4), f(0xe8), f(0xf0), f(0x20), f(0x30), d(0x50)]))
    e.mu.hook_add(UC_HOOK_CODE, _voice, begin=0x180036710, end=0x180036710)
    from unicorn.x86_const import UC_X86_REG_RDX as RDX, UC_X86_REG_R8 as R8_
    def _down(uc, addr, size, _):
        if any(x[0] == 'over' for x in trace):
            return
        l, r = uc.reg_read(RDX), uc.reg_read(R8_)
        trace.append(('over', struct.unpack('<12f', bytes(uc.mem_read(l, 48))), struct.unpack('<12f', bytes(uc.mem_read(r, 48)))))
        open('emu_over.bin', 'wb').write(bytes(uc.mem_read(l, 4 * 384)) + bytes(uc.mem_read(r, 4 * 384)))
    e.mu.hook_add(UC_HOOK_CODE, _down, begin=0x180036f50, end=0x180036f50)
    cur = [0]
    for name, at in (('fx_in', 0x180030300), ('master_in', 0x180023220)):
        e.mu.hook_add(UC_HOOK_CODE, _stage(name), begin=at, end=at)
process = slot(17)
done = 0
out = []
while done < frames:
    while evs and evs[0][0] <= done:
        _, kind, k, vel = evs.pop(0)
        e.call(note, this, k, vel if kind == 'on' else 0)
        if os.environ.get('SAWER_INFO'):
            print('rng after note', rng(), file=sys.stderr)
    n = min(block, frames - done)
    if evs:
        n = min(n, evs[0][0] - done)
    cur[0] = n
    e.wr(obj, struct.pack('<QIII', out_buf, 2, n, n))
    e.wr(out_buf, bytes(8 * n))
    e.call(process, this, 0, 0)
    raw = e.rd(out_buf, 8 * n)
    vals = struct.unpack('<%df' % (2 * n), raw)
    for i in range(n):
        out.append((vals[i], vals[n + i]))
    done += n
for l, r in out:
    print(repr(l), repr(r))
if trace:
    print(trace[:20], file=sys.stderr)
