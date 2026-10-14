"""Print the event-213 payload that follows each 201 naming a given plugin.

Usage: python3 -I dump213.py <root> <plugin name> [ext]
Walks every *.flp (or *.fst) under root. Prints file, preset name (event 203 for .fst if present),
and the hex payload plus a few decodings.
"""
import sys, struct, pathlib

def varint(b, i):
    n = shift = 0
    while True:
        c = b[i]; i += 1
        n |= (c & 0x7F) << shift
        shift += 7
        if not c & 0x80:
            return n, i

def events(b):
    assert b[:4] == b"FLhd"
    hlen = struct.unpack_from("<I", b, 4)[0]
    i = 8 + hlen
    assert b[i:i+4] == b"FLdt"
    dlen = struct.unpack_from("<I", b, i+4)[0]
    i += 8; end = i + dlen
    while i < end:
        eid = b[i]; i += 1
        if eid < 64:   v = b[i:i+1]; i += 1
        elif eid < 128: v = b[i:i+2]; i += 2
        elif eid < 192: v = b[i:i+4]; i += 4
        else:
            n, i = varint(b, i); v = b[i:i+n]; i += n
        yield eid, v

def s(v):
    try: return v.decode("utf-16-le").rstrip("\0")
    except Exception: return v.decode("latin1").rstrip("\0")

root = pathlib.Path(sys.argv[1]); name = sys.argv[2]; ext = sys.argv[3] if len(sys.argv) > 3 else "flp"
for p in sorted(root.rglob(f"*.{ext}")):
    b = p.read_bytes()
    try:
        cur = None; preset = None
        for eid, v in events(b):
            if eid == 203: preset = s(v)
            if eid == 201: cur = s(v)
            if eid == 213 and cur == name:
                u32 = struct.unpack_from(f"<{len(v)//4}i", v) if len(v) % 4 == 0 else ()
                print(f"{p.name}\t{preset or ''}\t{len(v)}\t{v.hex(' ')}\t{list(u32)}")
                cur = None
    except Exception as e:
        print(f"FAIL {p}: {e}", file=sys.stderr)
