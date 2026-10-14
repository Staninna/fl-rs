"""Like dump213.py but matches the 201 name case-insensitively in both encodings.
Usage: python3 -I dump213b.py <root> <plugin name> [ext]"""
import sys, struct, pathlib
def varint(b, i):
    n = shift = 0
    while True:
        c = b[i]; i += 1; n |= (c & 0x7F) << shift; shift += 7
        if not c & 0x80: return n, i
def events(b):
    assert b[:4] == b"FLhd"
    hlen = struct.unpack_from("<I", b, 4)[0]; i = 8 + hlen
    assert b[i:i+4] == b"FLdt"
    dlen = struct.unpack_from("<I", b, i+4)[0]; i += 8; end = i + dlen
    while i < end:
        eid = b[i]; i += 1
        if eid < 64: v = b[i:i+1]; i += 1
        elif eid < 128: v = b[i:i+2]; i += 2
        elif eid < 192: v = b[i:i+4]; i += 4
        else: n, i = varint(b, i); v = b[i:i+n]; i += n
        yield eid, v
def names(v):
    out = []
    if len(v) % 2 == 0:
        try: out.append(v.decode("utf-16-le").rstrip("\0"))
        except Exception: pass
    out.append(v.decode("latin1").rstrip("\0"))
    return [s.lower() for s in out]
root = pathlib.Path(sys.argv[1]); name = sys.argv[2].lower(); ext = sys.argv[3] if len(sys.argv) > 3 else "flp"
for p in sorted(root.rglob(f"*.{ext}")):
    b = p.read_bytes()
    try:
        cur = False; preset = None
        for eid, v in events(b):
            if eid == 203: preset = names(v)[0]
            if eid == 201: cur = name in names(v)
            if eid == 213 and cur:
                print(f"{p.name}\t{preset or ''}\t{len(v)}\t{v.hex(' ')}")
                cur = False
    except Exception as e:
        print(f"FAIL {p}: {e}", file=sys.stderr)
