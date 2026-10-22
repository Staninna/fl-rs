"""Run Sakura's CSakuraSynth under unicorn and print its output for a state.

Usage: emu_sakura.py <state.bin (event 213 bytes)> <key> <frames> [block] [rate] [release_at]
Prints one line per output frame: left right (as floats, repr).
"""
import struct
import sys

sys.path.insert(0, '/root/work/sakura/scripts')
from emu import Emu, f32b, f64b  # noqa: E402

from unicorn.x86_const import UC_X86_REG_XMM1  # noqa: E402

state = open(sys.argv[1], 'rb').read()
key = int(sys.argv[2])
frames = int(sys.argv[3])
block = int(sys.argv[4]) if len(sys.argv) > 4 else 512
rate = float(sys.argv[5]) if len(sys.argv) > 5 else 44100.0
release_at = int(sys.argv[6]) if len(sys.argv) > 6 else -1
velocity = int(sys.argv[7]) if len(sys.argv) > 7 else 100

e = Emu()
# The preset list the constructor makes reads files: skip it.
e.wr(0x18002e9c0, b'\xc3')
desc = 0x180092fc0
e.wr(0x180086958, struct.pack('<Q', desc))
e.wr(desc + 0x118, struct.pack('<5I', 0x72, 0x4e, 0x10, 99, 0x100))
this = e.alloc(0x6000)
e.call(0x18002e670, this, 0)
# Polyphony: slot 53 with 0, the most.
e.call(0x1800222b0, this, 0)
# Sample rate: slot 10 with the rate.
e.call(0x180030660, this, xmm=(None, f64b(rate)))
# State: the engine's chunk after SSLF and its length.
chunk = state[8:]
buf = e.alloc(len(chunk))
e.wr(buf, chunk)
e.call(0x1800313d0, this, buf, len(chunk))
# Per-note gains (circular pan law in the centre) and pitch offset 0.
c = struct.pack('<f', 0.7071067811865476)
for n in range(128):
    e.wr(this + 0x20 + 4 * n, c)
    e.wr(this + 0x220 + 4 * n, c)
    e.wr(this + 0x420 + 4 * n, struct.pack('<f', 0.0))
# Output buffers.
obj = e.alloc(0x20)
data = e.alloc(8 * block + 64)
e.wr(this + 0x1060, struct.pack('<Q', obj))
import os
events = []
for ev in os.environ.get('SAKURA_EVENTS', '').split():
    kind, k, at = ev.split(':')
    events.append((int(at), kind, int(k)))
if not events:
    events = [(0, 'on', key)]
    if release_at >= 0:
        events.append((release_at, 'off', key))
    release_at = -1
events.sort()
done = 0
out = []
while done < frames:
    n = min(block, frames - done)
    while events and events[0][0] <= done:
        _, kind, k = events.pop(0)
        e.call(0x180007a20, this, k, velocity if kind == 'on' else 0)
    if release_at >= 0 and done <= release_at < done + n:
        pass
    e.wr(obj, struct.pack('<QIII', data, 2, n, n))
    e.call(0x180030720, this, 0, 0)
    raw = e.rd(data, 8 * n)
    vals = struct.unpack('<%df' % (2 * n), raw)
    for i in range(n):
        out.append((vals[i], vals[n + i]))
    done += n
    if done <= 3 * n and len(sys.argv) > 8:
        v = struct.unpack('<Q', e.rd(this + 0x3c90, 8))[0]
        f = lambda o, k=1: struct.unpack('<%df' % k, e.rd(v + o, 4 * k))
        d = lambda o: struct.unpack('<d', e.rd(v + o, 8))[0]
        print('step', d(0x118), 'scale', d(0x130), 'freq', d(0x128), file=sys.stderr)
        print('damp', f(0x290, 4), 'sharp', f(0x2a0, 4), file=sys.stderr)
        print('gain', f(0x2b0, 4), 'mix', f(0x2d0, 4), file=sys.stderr)
        print('b', f(0x19c, 3), 'hc', f(0x170, 5), 'amp', f(0x100, 3), file=sys.stderr)
        print('sat', f(0x25c), 'follow', f(0x2f4, 2), 'comb', f(0x254, 2), 'eg', f(0x50, 2), file=sys.stderr)
        fx = struct.unpack('<Q', e.rd(this + 0x3c88, 8))[0]
        rb = struct.unpack('<Q', e.rd(fx + 8, 8))[0]
        print('res', struct.unpack('<8f', e.rd(rb, 32)), struct.unpack('<8f', e.rd(rb + 0x20, 32)), file=sys.stderr)
        print('hpf', struct.unpack('<6f', e.rd(fx + 0xac, 24)), file=sys.stderr)
        print('lens', [(struct.unpack('<Q', e.rd(v + o + 0x10, 8))[0] - struct.unpack('<Q', e.rd(v + o, 8))[0]) // 4 for o in (0x1b8, 0x1d0, 0x208, 0x220, 0x1e8, 0x238)], file=sys.stderr)
    if release_at >= 0 and done >= release_at and release_at > -2:
        e.call(0x180007a20, this, key, 0)
        release_at = -2
for l, r in out:
    print(repr(l), repr(r))
