import sys, struct
sys.path.insert(0, '/root/work/morphine/scripts')
from parse import top
off = {0: 0x608, 1: 0x60c, 2: 0x618, 3: 0x628, 4: 0x61c, 5: 0x620, 6: 0x624, 30: 0x62c, 31: 0x630,
       32: 0x634, 33: 0x638, 34: 0x698, 35: 0x69c, 36: 0x6a0, 37: 0x6c0, 38: 0x6a4, 39: 0x6a8,
       40: 0x6b0, 41: 0x6b4, 42: 0x6b8, 0x7c: 0x614, 0x7d: 0x610, 0x7e: 0x6ac, 0x7f: 0x6bc}
for i in range(7, 30):
    off[i] = 0x63c + (i - 7) * 4
names = {int(l.split()[0]): l.split(' ', 2)[2].strip() for l in open('/root/work/morphine/names.txt')}


def s_of(p):
    if 0x608 <= p < 0x6b0:
        return p - 0x604
    return {0x6bc: 0x5b0, 0x6b0: 0x5b4, 0x6b4: 0x5b8, 0x6b8: 0x5bc, 0x6c0: 0x5c0}[p]


ds = []
for f in sys.argv[1:]:
    b = open(f, 'rb').read()
    d = top(b)
    st = d['struct'].o
    ds.append({i: struct.unpack_from('<f', b, st + s_of(p))[0] for i, p in off.items()})
for i in sorted(off):
    print('%3d %-22s' % (i, names.get(i, '?')), ' '.join('%8.4g' % d[i] for d in ds))
