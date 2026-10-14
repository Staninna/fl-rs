"""Walk every .flp under a root and tally event IDs.

FLP layout: 'FLhd' chunk (len 6: format u16, channels u16, ppq u16), then
'FLdt' chunk holding a stream of events. Event ID byte decides payload size:
  0..63    u8
  64..127  u16
  128..191 u32
  192..255 varint length + bytes
"""
import sys, struct, collections, pathlib

def varint(b, i):
    n = shift = 0
    while True:
        c = b[i]; i += 1
        n |= (c & 0x7F) << shift
        shift += 7
        if not c & 0x80:
            return n, i

def events(b):
    assert b[:4] == b"FLhd", "no FLhd"
    hlen = struct.unpack_from("<I", b, 4)[0]
    fmt, nch, ppq = struct.unpack_from("<hHH", b, 8)
    i = 8 + hlen
    assert b[i:i+4] == b"FLdt", "no FLdt"
    dlen = struct.unpack_from("<I", b, i+4)[0]
    i += 8; end = i + dlen
    while i < end:
        eid = b[i]; i += 1
        if eid == 0xAC:
            eid = struct.unpack_from("<I", b, i)[0]; i += 4; cls = eid >> 28
        else:
            cls = (eid >> 6) << 2
        if cls == 0:   v = b[i:i+1]; i += 1
        elif cls == 4: v = b[i:i+2]; i += 2
        elif cls == 8: v = b[i:i+4]; i += 4
        else:
            n, i = varint(b, i); v = b[i:i+n]; i += n
        yield eid, v
    assert i == end, f"overrun {i} != {end}"
    return

root = pathlib.Path(sys.argv[1])
ids = collections.Counter(); files_with = collections.Counter(); sizes = collections.defaultdict(set)
ok = bad = 0; versions = collections.Counter()
for p in sorted(root.rglob("*.flp")):
    b = p.read_bytes()
    try:
        seen = set()
        for eid, v in events(b):
            ids[eid] += 1; seen.add(eid)
            if eid >= 0xC0000000 or 192 <= eid < 256: sizes[eid].add(len(v))
            if eid == 199: versions[v.rstrip(b"\0").decode("latin1")] += 1
        for e in seen: files_with[e] += 1
        ok += 1
    except Exception as e:
        bad += 1; print(f"FAIL {p.relative_to(root)}: {e}", file=sys.stderr)
print(f"parsed {ok} files, {bad} failed, {len(ids)} distinct event ids")
print("versions:", dict(versions))
print("id  files  count  payload-sizes(var only)")
for eid in sorted(ids):
    s = sorted(sizes[eid]); s = (s[:6] + ["..."]) if len(s) > 6 else s
    var = eid >= 0xC0000000 or 192 <= eid < 256
    print(f"{eid:#x} {files_with[eid]:5d} {ids[eid]:7d}  {s if var else ''}" if eid >= 256 else f"{eid:3d} {files_with[eid]:5d} {ids[eid]:7d}  {s if var else ''}")
