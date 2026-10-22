"""Run one CStringDelay of the engine over a noise input; print outputs as f32 reprs.
Usage: emu_string.py <points> <position> <feedback> <frames>"""
import struct
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from emu import Emu, f32b, f64b  # noqa: E402

points, position, feedback, frames = int(sys.argv[1]), float(sys.argv[2]), float(sys.argv[3]), int(sys.argv[4])
e = Emu()
obj = e.alloc(0x58)
e.call(0x180025330, obj, points)
seed = 1
out = []
for i in range(frames):
    seed = (seed * 1664525 + 1013904223) & 0xFFFFFFFF
    x = struct.unpack('<f', struct.pack('<f', (seed >> 8) / 16777216.0 - 0.5))[0] if i < 2000 else 0.0
    e.call(0x1800257e0, obj, xmm=(None, f64b(position)))
    e.wr(obj + 0x14, struct.pack('<f', feedback))
    e.wr(obj + 8, struct.pack('<f', 0.5))
    e.call(0x180025570, obj, xmm=(None, f32b(x)))
    y = struct.unpack('<f', (e.xmm0() & 0xFFFFFFFF).to_bytes(4, 'little'))[0]
    out.append(y)
for y in out:
    print(repr(y), repr(y))
