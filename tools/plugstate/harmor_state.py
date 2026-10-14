"""Parse Harmor event-213 states (and .fst presets) with the layout in docs/plugins/harmor.md.

Usage: python3 -I harmor_state.py <dump.tsv> [summary|params|arts|check]
<dump.tsv> is the output of dump213b.py (<file> <preset> <len> <hex>).

summary: one line per state (version, flags, part sizes, image kinds, info text)
params:  per state, the 386 part params of A and B and the 53 + 6 global ints
arts:    per state, which of the 43 articulators have an active envelope / LFO / non-flat map
check:   only report states that do not parse to the last byte
"""
import sys, struct

ARTS = ["Panning", "Volume", "FX / dry mix", "Timbre 1 & 2 mix", "Filter 1 frequency", "Filter 2 frequency",
        "Filter 1 width", "Filter 2 width", "Filter 1 resonance amount", "Filter 2 resonance amount",
        "Filter 1 resonance width", "Filter 2 resonance width", "Filter 1 resonance offset",
        "Filter 2 resonance offset", "Filter 1 & 2 mix", "Pluck amount", "Phaser mix", "Phaser width",
        "Phaser offset", "Harmonizer mix", "Harmonizer width", "Harmonic clipping threshold",
        "Harmonic blur amount", "Prism amount", "Pitch", "Pitch vibrato depth", "Unison pitch thickness",
        "Image time offset", "Image fine speed", "Image formant shift", "Image freq pixel scale",
        "Filter 1 envelope amount", "Filter 2 envelope amount", "Global envelope attack time",
        "Global envelope attack scale", "Global envelope decay scale", "Global envelope sustain offset",
        "Global envelope release scale", "Global LFO amount", "Global LFO speed", "Global LFO phase",
        "Unison phase", "Predelay"]
SHAPES = ["Local EQ", "Harmonic phase", "Timbre 1 harmonic level", "Timbre 2 harmonic level",
          "Harmonic randomness", "Harmonic protection", "Harmonic clipping", "Harmonic prism",
          "Harmonic unison pitch", "Image formant mutation", "Filter shape 1", "Filter shape 2",
          "Resonance shape 1", "Resonance shape 2", "Pluck shape", "Phaser shape", "Harmonizer distribution",
          "Global EQ", "Filter 1 mask", "Filter 2 mask", "Filter 1 resonance mask", "Filter 2 resonance mask",
          "Phaser mask", "Harmonizer local mask", "Blur local mask", "Image freq local mask"]
SUBS = ["env", "lfo", "key", "vel", "modx", "mody", "modz", "rand", "unison", "held", "modxspeed"]


class R:
    def __init__(self, b, i=0): self.b, self.i = b, i
    def take(self, n):
        if self.i + n > len(self.b): raise ValueError(f"short read {n} at {self.i}")
        v = self.b[self.i:self.i + n]; self.i += n; return v
    def i32(self): return struct.unpack('<i', self.take(4))[0]
    def u8(self): return self.take(1)[0]
    def ints(self, n): return list(struct.unpack(f'<{n}i', self.take(4 * n)))


def menv(r, cls):
    """TCustomMEnvelope stream (FUN_0079a810) plus the class tail. Returns a dict."""
    kind = r.i32(); ver = r.i32(); n = r.i32()
    pts = []
    for _ in range(n):
        if ver >= 2:
            x, y = struct.unpack('<dd', r.take(16)); t = struct.unpack('<f', r.take(4))[0]; flags = r.take(4)
        elif ver == 1:
            x, y, t = struct.unpack('<fff', r.take(12)); flags = r.take(4)
        else:
            x, y, t = struct.unpack('<fff', r.take(12)); flags = b'\0' * 4
        pts.append((x, y, t, flags))
    tail = struct.unpack('<Iiiii', r.take(20))
    e = {'kind': kind, 'ver': ver, 'pts': pts, 'flags': tail[0], 'active': tail[1], 'decay': tail[2],
         'loop': (tail[3], tail[4])}
    if cls == 'env':
        e['knobs'] = r.ints(4)
    elif cls == 'lfo':
        e['knobs'] = r.ints(4)
        if ver >= 3: e['phase'] = r.i32()
    return e


def articulator(r):
    fmt = r.i32(); f3 = struct.unpack('<3f', r.take(12))
    subs = [menv(r, 'env'), menv(r, 'lfo')] + [menv(r, 'map') for _ in range(9)]
    return {'fmt': fmt, 'f3': f3, 'subs': subs}


def flat(m):
    ys = {round(p[1], 6) for p in m['pts']}
    return len(ys) <= 1


def image(r):
    ver = r.i32()
    h = r.take(0x2c)
    typ, a58, a100 = struct.unpack_from('<iii', h, 0); d = struct.unpack_from('<3d', h, 12)
    cols, rows = struct.unpack_from('<ii', h, 36)
    planes = []
    if typ == 0:
        for p in range(3):
            n = r.i32()
            if n > 0:
                r.take(4 * cols * rows)
            planes.append(n)
    return {'ver': ver, 'type': typ, 'a58': a58, 'a100': a100, 'd': d, 'cols': cols, 'rows': rows, 'planes': planes}


def ansistr(r):
    n = r.u8()
    if n == 0xff: n = r.i32()
    return r.take(n).decode('latin1')


def widestr(r):
    n = r.u8()
    if n == 0xff: n = r.i32()
    return r.take(2 * n).decode('utf-16-le', 'replace')


def part(r, ver):
    size = r.i32(); start = r.i
    np = 386 if ver >= 16 else 385
    params = r.ints(np)
    if ver < 16: params.insert(376, 0)
    extra = r.ints(12)
    sn = 0x4b if ver >= 11 else 0x47
    s = r.take(sn)
    st = {'ints13': list(struct.unpack_from('<13i', s, 0)), 'pairs': list(struct.unpack_from('<4i', s, 52)),
          'bytes': list(s[68:71]), 'last': struct.unpack_from('<i', s, 71)[0] if sn == 0x4b else 0}
    arts = []
    for k in range(43):
        if ver < 16 and k == 33:
            arts.append(None); continue
        arts.append(articulator(r))
    shapes = [menv(r, 'map') for _ in range(26)]
    img = image(r)
    fname = ansistr(r)
    end = r.i
    return {'size': size, 'used': end - start, 'params': params, 'extra': extra, 'struct': st, 'arts': arts,
            'shapes': shapes, 'image': img, 'file': fname}


def parse(b):
    r = R(b)
    v = r.i32()
    ver = v >> 16 if v > 0xffff else v
    flags = r.i32()
    out = {'ver': ver, 'raw': v, 'flags': flags, 'parts': []}
    nparts = 2 if flags & 3 == 3 else (1 if flags & 3 else 0)
    for _ in range(nparts):
        p = part(r, ver)
        if p['used'] != p['size']:
            # trust the size field, as the loader does when it skips a part
            r.i = r.i - p['used'] + p['size']
        out['parts'].append(p)
    if flags & 4:
        out['global'] = r.ints(53)
        out['global2'] = r.ints(6)
        g = r.take(0x3d)
        out['g3d'] = list(struct.unpack_from('<14i', g, 0)) + list(g[56:61])
        if ver < 18:
            n = r.i32(); out['info'] = r.take(n).decode('latin1', 'replace')
        else:
            out['info'] = widestr(r)
            if ver > 18:
                out['gflags'] = r.u8()
    out['rest'] = len(b) - r.i
    return out


def rows(path):
    for l in open(path):
        f, p, n, h = l.rstrip('\n').split('\t')
        yield f, p, bytes.fromhex(h.replace(' ', ''))


if __name__ == '__main__':
    mode = sys.argv[2] if len(sys.argv) > 2 else 'summary'
    for f, p, b in rows(sys.argv[1]):
        try:
            s = parse(b)
        except Exception as e:
            print(f"FAIL\t{f}\t{p}\t{len(b)}\t{e}"); continue
        if mode == 'check':
            bad = [q for q in s['parts'] if q['used'] != q['size']]
            if s['rest'] or bad:
                print(f"{f}\t{p}\t{len(b)}\tv{s['ver']}\trest {s['rest']}\t" + ' '.join(f"{q['used']}/{q['size']}" for q in s['parts']))
            continue
        if mode == 'summary':
            imgs = ' '.join(f"img{q['image']['type']}:{q['image']['cols']}x{q['image']['rows']}:{q['image']['planes']}" for q in s['parts'])
            print(f"{f}\t{p}\t{len(b)}\tv{s['ver']}\tflags {s['flags']}\t{imgs}\trest {s['rest']}\t{s.get('info','')[:40]!r}")
        elif mode == 'params':
            for k, q in enumerate(s['parts']):
                print(f"{f}\t{p}\tpart{k}\t" + ' '.join(map(str, q['params'])) + '\t' + ' '.join(map(str, q['extra'])) + '\t' + repr(q['struct']))
            if 'global' in s:
                print(f"{f}\t{p}\tglobal\t" + ' '.join(map(str, s['global'])) + '\t' + ' '.join(map(str, s['global2'])) + '\t' + ' '.join(map(str, s['g3d'])) + f"\t{s.get('gflags')}")
        elif mode == 'arts':
            for k, q in enumerate(s['parts']):
                on = []
                for a, art in enumerate(q['arts']):
                    if art is None: continue
                    subs = [SUBS[j] for j, m in enumerate(art['subs']) if (j < 2 and m['active']) or (j >= 2 and m['active'] and not flat(m))]
                    if subs: on.append(f"{a}:{'+'.join(subs)}")
                sh = [str(j) for j, m in enumerate(q['shapes']) if m['active'] and not flat(m)]
                print(f"{f}\t{p}\tpart{k}\t{' '.join(on)}\tshapes {' '.join(sh)}")
