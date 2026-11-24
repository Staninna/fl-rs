"""Dump plugin states (event 213 after 201 name) from flp/fst under roots. usage: states.py name outdir roots..."""
import sys, struct, pathlib
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
    if len(v) >= 2 and len(v) % 2 == 0 and all(c == 0 for c in v[1::2]):
        return v.decode("utf-16-le").rstrip("\0")
    return v.decode("latin1").rstrip("\0")
name=sys.argv[1].lower(); out=pathlib.Path(sys.argv[2]); out.mkdir(exist_ok=True,parents=True)
for root in sys.argv[3:]:
  for p in sorted(list(pathlib.Path(root).rglob("*.flp"))+list(pathlib.Path(root).rglob("*.fst"))):
    b = p.read_bytes()
    if b[:4]!=b"FLhd": continue
    try:
        cur=None; k=0
        for eid, v in events(b):
            if eid == 201: cur = s(v)
            if eid == 213 and cur and cur.lower()==name:
                fn=out/((p.parent.name+"__" if p.suffix==".flp" else "")+p.stem+f"_{k}.bin"); k+=1
                fn.write_bytes(v); print(fn.name, len(v), p.parent.name)
                cur=None
    except Exception as e: print("FAIL",p,e,file=sys.stderr)
