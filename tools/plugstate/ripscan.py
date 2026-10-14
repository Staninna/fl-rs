"""Find RIP-relative references to a VA inside a PE's .text, and print the bytes around each.

Usage: python3 -I ripscan.py <dll> <va>
Also: python3 -I ripscan.py <dll> vmts   -> scan for Delphi x64 VMTs via the self pointer at -200 and print class names
"""
import sys, struct
sys.path.insert(0, '/shared/code/fl-rs/tools/plugstate')
from pe import PE

pe = PE(sys.argv[1])
if sys.argv[2] == 'vmts':
    for va0, vs, ro, rs in pe.secs:
        seg = pe.b[ro:ro+rs]
        for k in range(0, len(seg)-8, 8):
            q = struct.unpack_from('<Q', seg, k)[0]
            a = pe.base + va0 + k
            if q == a + 200:
                try:
                    cn = pe.q(a + 200 - 136)
                    name = pe.shortstr(cn)
                except Exception:
                    name = '?'
                flt = sys.argv[3].lower() if len(sys.argv) > 3 else None
                if flt is None or flt in name.lower():
                    print(f'vmt {q:08x} class {name}')
    sys.exit()

target = int(sys.argv[2], 16)
va0, vs, ro, rs = pe.secs[0]
seg = pe.b[ro:ro+rs]
for k in range(len(seg)-4):
    disp = struct.unpack_from('<i', seg, k)[0]
    ip = pe.base + va0 + k + 4
    if ip + disp == target:
        ctx = seg[max(0, k-12):k+6]
        print(f'{pe.base+va0+k:08x}: {ctx.hex(" ")}')
