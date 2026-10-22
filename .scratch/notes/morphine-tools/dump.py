import sys, struct, pathlib
def varint(b, i):
    n = shift = 0
    while True:
        c = b[i]; i += 1
        n |= (c & 0x7F) << shift; shift += 7
        if not c & 0x80: return n, i
def events(b):
    hlen = struct.unpack_from("<I", b, 4)[0]; i = 8 + hlen + 8
    while i < len(b):
        eid = b[i]; i += 1
        if eid < 64: n=1
        elif eid < 128: n=2
        elif eid < 192: n=4
        else: n, i = varint(b, i)
        yield eid, b[i:i+n]; i += n
def s(v):
    if len(v)>=2 and v[1]==0: return v.decode("utf-16-le","replace").rstrip("\0")
    return v.decode("latin1").rstrip("\0")
root = pathlib.Path(sys.argv[1]); out = pathlib.Path(sys.argv[2]); out.mkdir(exist_ok=True)
k=0
for ext in ("flp","fst"):
  for p in sorted(root.rglob(f"*.{ext}")):
    b = p.read_bytes()
    if b[:4]!=b"FLhd": continue
    cur=None
    for eid, v in events(b):
        if eid == 201: cur = s(v)
        if eid == 213 and cur and cur.lower()=="morphine":
            name = f"{ext}_{k:03d}_{p.stem}.bin".replace(" ","_")
            (out/name).write_bytes(v); k+=1; cur=None
print(k)
