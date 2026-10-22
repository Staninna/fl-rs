import sys, struct, pathlib
sys.path.insert(0, sys.argv[2])
def varint(b, i):
    n = shift = 0
    while True:
        c = b[i]; i += 1
        n |= (c & 0x7F) << shift; shift += 7
        if not c & 0x80: return n, i
def events(b):
    hlen = struct.unpack_from("<I", b, 4)[0]; i = 8 + hlen
    dlen = struct.unpack_from("<I", b, i+4)[0]; i += 8; end = i + dlen
    while i < end:
        eid = b[i]; i += 1
        if eid < 64: v = b[i:i+1]; i += 1
        elif eid < 128: v = b[i:i+2]; i += 2
        elif eid < 192: v = b[i:i+4]; i += 4
        else:
            n, i = varint(b, i); v = b[i:i+n]; i += n
        yield eid, v
def s(v):
    for enc in ("utf-16-le","latin1"):
        try:
            t=v.decode(enc).rstrip("\0")
            if t.isprintable(): return t
        except Exception: pass
    return v.decode("latin1")
root = pathlib.Path(sys.argv[1])
for p in sorted(list(root.rglob("*.flp"))+list(root.rglob("*.fst"))):
    b = p.read_bytes()
    if b[:4]!=b"FLhd": continue
    try:
        cur=None
        for eid, v in events(b):
            if eid == 201: cur = s(v)
            if eid == 213 and cur and cur.lower()=="sakura":
                print(p, len(v)); out=pathlib.Path(sys.argv[2])/"states"; out.mkdir(exist_ok=True)
                (out/(p.stem+f"_{len(list(out.glob(p.stem+'*')))}.bin")).write_bytes(v)
                cur=None
    except Exception as e: print("FAIL",p,e,file=sys.stderr)
