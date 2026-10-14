"""Read bytes from a PE image by virtual address.

Usage:
  python3 -I pe.py <dll> q <va> [count]       # count little-endian u64 at va
  python3 -I pe.py <dll> d <va> [count]       # count u32
  python3 -I pe.py <dll> f <va> [count]       # count f32
  python3 -I pe.py <dll> g <va> [count]       # count f64
  python3 -I pe.py <dll> x <va> [len]         # hex dump
  python3 -I pe.py <dll> s <va>               # Delphi short string (len byte + ansi) and C string
  python3 -I pe.py <dll> refq <value>         # find 8-byte aligned qwords equal to value (VA) in all sections
"""
import sys, struct

class PE:
    def __init__(self, path):
        self.b = open(path, 'rb').read()
        b = self.b
        pe = struct.unpack_from('<I', b, 0x3c)[0]
        nsec = struct.unpack_from('<H', b, pe+6)[0]
        optsz = struct.unpack_from('<H', b, pe+20)[0]
        self.base = struct.unpack_from('<Q', b, pe+24+24)[0]
        self.secs = []
        off = pe+24+optsz
        for i in range(nsec):
            vs, va, rs, ro = struct.unpack_from('<IIII', b, off+8)
            self.secs.append((va, vs, ro, rs)); off += 40
    def off(self, va):
        rva = va - self.base
        for va0, vs, ro, rs in self.secs:
            if va0 <= rva < va0 + max(vs, rs):
                return ro + (rva - va0)
        raise ValueError(f'va {va:#x} not mapped')
    def read(self, va, n):
        o = self.off(va); return self.b[o:o+n]
    def q(self, va): return struct.unpack('<Q', self.read(va, 8))[0]
    def d(self, va): return struct.unpack('<I', self.read(va, 4))[0]
    def i(self, va): return struct.unpack('<i', self.read(va, 4))[0]
    def f(self, va): return struct.unpack('<f', self.read(va, 4))[0]
    def g(self, va): return struct.unpack('<d', self.read(va, 8))[0]
    def shortstr(self, va):
        n = self.read(va, 1)[0]; return self.read(va+1, n).decode('latin1')
    def cstr(self, va):
        o = self.off(va); e = self.b.index(b'\0', o); return self.b[o:e].decode('latin1')
    def find_qword(self, val):
        needle = struct.pack('<Q', val); hits = []
        for va0, vs, ro, rs in self.secs:
            seg = self.b[ro:ro+rs]; i = seg.find(needle)
            while i >= 0:
                if i % 8 == 0: hits.append(self.base + va0 + i)
                i = seg.find(needle, i+1)
        return hits

if __name__ == '__main__':
    pe = PE(sys.argv[1]); cmd = sys.argv[2]; va = int(sys.argv[3], 16)
    n = int(sys.argv[4]) if len(sys.argv) > 4 else 8
    if cmd == 'q':
        for k in range(n): print(f'{va+8*k:08x}: {pe.q(va+8*k):016x}')
    elif cmd == 'd':
        for k in range(n): print(f'{va+4*k:08x}: {pe.d(va+4*k):08x} {pe.i(va+4*k)}')
    elif cmd == 'f':
        for k in range(n): print(f'{va+4*k:08x}: {pe.f(va+4*k)!r}')
    elif cmd == 'g':
        for k in range(n): print(f'{va+8*k:08x}: {pe.g(va+8*k)!r}')
    elif cmd == 'x':
        d = pe.read(va, n)
        for k in range(0, len(d), 16): print(f'{va+k:08x}: {d[k:k+16].hex(" ")}  {"".join(chr(c) if 32<=c<127 else "." for c in d[k:k+16])}')
    elif cmd == 's':
        print('short:', repr(pe.shortstr(va))); print('c:', repr(pe.cstr(va)))
    elif cmd == 'refq':
        print([f'{h:08x}' for h in pe.find_qword(va)])
