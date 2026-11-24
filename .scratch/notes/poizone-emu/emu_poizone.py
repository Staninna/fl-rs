"""Run PoiZone's PoiZoneSynth (engine_x64.dll) under unicorn and print its output.

Usage: emu_poizone.py <state.bin (event 213)> "<events>" <frames> [block] [rate]
events: on:<key>:<frame>[:<velocity>] off:<key>:<frame>
Prints one line per frame: left right.
POIZONE_INFO=1 prints the random generators, the defaults and the voices' phases.
"""
import os
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from emu import Emu, f64b  # noqa: E402
from unicorn import UC_HOOK_CODE  # noqa: E402

state = open(sys.argv[1], 'rb').read()
evs = []
for ev in sys.argv[2].split():
    p = ev.split(':')
    evs.append((int(p[2]), p[0], int(p[1]), int(p[3]) if len(p) > 3 else 100))
evs.sort(key=lambda x: (x[0], x[1] != 'off'))
frames = int(sys.argv[3])
block = int(sys.argv[4]) if len(sys.argv) > 4 else 64
rate = float(sys.argv[5]) if len(sys.argv) > 5 else 44100.0
INFO = os.environ.get('POIZONE_INFO')

e = Emu()
# The preset list reads files: skip it.
e.wr(0x18002e0b0, b'\xc3')
desc = 0x180090f90
e.wr(0x180084928, struct.pack('<Q', desc))
e.wr(desc + 0x118, struct.pack('<5I', 0x9c, 0x4a, 0xc, 0x40, 0x100))
this = e.alloc(0x10000)
rng = lambda: struct.unpack('<III', e.rd(0x180081e20, 12))
if INFO:
    print('rng before ctor', rng(), file=sys.stderr)
e.call(0x18002dca0, this, 0)
q = lambda a: struct.unpack('<Q', e.rd(a, 8))[0]
f = lambda a: struct.unpack('<f', e.rd(a, 4))[0]
i32 = lambda a: struct.unpack('<i', e.rd(a, 4))[0]
if INFO:
    print('rng after ctor', rng(), file=sys.stderr)
    tab = q(this + 0x1040)
    vals = [f(tab + 0x94 * i + 4) for i in range(0x9c)]
    print('defaults', vals, file=sys.stderr)
    v = q(this + 0x1670)
    n = 0
    out = []
    while v and n < 6:
        out.append((hex(v), f(v + 0x48), f(v + 0x5c), i32(v + 0x40)))
        v = q(v + 0x38)
        n += 1
    print('voices', out, file=sys.stderr)
    print('fields 38cc..3910', [f(this + 0x38c8 + 4 * k) for k in range(18)], file=sys.stderr)
vt = q(this)
slot = lambda n: q(vt + 8 * n)
e.call(slot(53), this, 0)
e.call(slot(10), this, xmm=(None, f64b(rate)))
if INFO:
    print('rng after rate', rng(), file=sys.stderr)
chunk = state[8:]
buf = e.alloc(len(chunk))
e.wr(buf, chunk)
e.call(slot(36), this, buf, len(chunk))
if INFO:
    print('rng after load', rng(), file=sys.stderr)
    print('knobs 59e8..5ab8', [f(this + 0x59e8 + 4 * k) for k in range(52)], file=sys.stderr)
    print('ints 38e4..', [i32(this + 0x38e4 + 4 * k) for k in range(6)], file=sys.stderr)
c = struct.pack('<f', 0.7071067811865476)
for n in range(128):
    e.wr(this + 0x20 + 4 * n, c)
    e.wr(this + 0x220 + 4 * n, c)
    e.wr(this + 0x420 + 4 * n, struct.pack('<f', 0.0))
obj = e.alloc(0x20)
out_buf = e.alloc(8 * block + 64)
e.wr(this + 0x1060, struct.pack('<Q', obj))
note = slot(1)
TRACE = os.environ.get('POIZONE_TRACE')
trace = []
if TRACE:
    def _upd(uc, addr, size, _):
        from unicorn.x86_const import UC_X86_REG_RCX
        v = uc.reg_read(UC_X86_REG_RCX)
        if len(trace) < int(TRACE):
            g = lambda o: struct.unpack('<f', bytes(uc.mem_read(v + o, 4)))[0]
            h = lambda o: '%08x' % struct.unpack('<I', bytes(uc.mem_read(v + o, 4)))[0]
            trace.append('upd %d %s %s %s %s %s %s' % (struct.unpack('<i', bytes(uc.mem_read(v + 0x10, 4)))[0], h(0x30), h(0x4c), h(0x60), h(0x90), h(0x94), h(0x98)))
    e.mu.hook_add(UC_HOOK_CODE, _upd, begin=0x18002c580, end=0x18002c580)
process = slot(17)
done = 0
out = []
while done < frames:
    while evs and evs[0][0] <= done:
        _, kind, k, vel = evs.pop(0)
        e.call(note, this, k, vel if kind == 'on' else 0)
        if INFO:
            print('rng after note', rng(), file=sys.stderr)
    n = min(block, frames - done)
    if evs:
        n = min(n, evs[0][0] - done)
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
for t in trace:
    print(t, file=sys.stderr)
