"""Dump Delphi binary DFM resources ("TPF0") found in a PE file as indented text.

Usage: python3 -I dfm.py <dll> [class-name-filter]
Only prints objects whose class name or object name contains the filter (case-insensitive),
plus their children. Properties printed as name = value.
"""
import sys, struct

def rd_short(b, i):
    n = b[i]; return b[i+1:i+1+n].decode('latin1'), i+1+n

def rd_value(b, i):
    t = b[i]; i += 1
    if t == 0: return None, i
    if t == 1: # vaList
        items = []
        while b[i] != 0:
            v, i = rd_value(b, i); items.append(v)
        return items, i+1
    if t == 2: return struct.unpack_from('<b', b, i)[0], i+1
    if t == 3: return struct.unpack_from('<h', b, i)[0], i+2
    if t == 4: return struct.unpack_from('<i', b, i)[0], i+4
    if t == 5: # extended (10 bytes)
        raw = b[i:i+10]; i += 10
        mant, se = struct.unpack('<QH', raw); sign = -1 if se & 0x8000 else 1; e = (se & 0x7fff)
        v = 0.0 if mant == 0 else sign * mant * 2.0 ** (e - 16383 - 63)
        return v, i
    if t == 6: s, i = rd_short(b, i); return repr(s), i
    if t == 7: s, i = rd_short(b, i); return s, i
    if t == 8: return False, i
    if t == 9: return True, i
    if t == 10: n = struct.unpack_from('<I', b, i)[0]; return f'<binary {n}>', i+4+n
    if t == 11:
        items = []
        while True:
            s, i = rd_short(b, i)
            if s == '': break
            items.append(s)
        return '[' + ','.join(items) + ']', i
    if t == 12: n = struct.unpack_from('<I', b, i)[0]; return repr(b[i+4:i+4+n].decode('latin1')), i+4+n
    if t == 13: return 'nil', i
    if t == 14: # collection
        items = []
        while b[i] != 0:
            if b[i] in (2, 3, 4): _, i = rd_value(b, i)
            i += 1  # vaList
            props = {}
            while b[i] != 0:
                name, i = rd_short(b, i); v, i = rd_value(b, i); props[name] = v
            i += 1
            items.append(props)
        return items, i+1
    if t == 15: return struct.unpack_from('<f', b, i)[0], i+4
    if t == 16: return struct.unpack_from('<q', b, i)[0] / 10000, i+8
    if t == 17: return struct.unpack_from('<d', b, i)[0], i+8
    if t == 18 or t == 20:
        n = struct.unpack_from('<I', b, i)[0]; i += 4
        if t == 18: return repr(b[i:i+2*n].decode('utf-16-le')), i+2*n
        return repr(b[i:i+n].decode('utf-8', 'replace')), i+n
    if t == 19: return struct.unpack_from('<q', b, i)[0], i+8
    if t == 21: return struct.unpack_from('<d', b, i)[0], i+8
    raise ValueError(f'unknown value type {t} at {i:#x}')

def rd_object(b, i, depth, out):
    flags = 0
    if b[i] & 0xf0 == 0xf0:
        flags = b[i] & 0x0f; i += 1
        if flags & 2: _, i = rd_value(b, i)
    cls, i = rd_short(b, i); name, i = rd_short(b, i)
    props = []
    while b[i] != 0:
        pname, i = rd_short(b, i); v, i = rd_value(b, i); props.append((pname, v))
    i += 1
    children = []
    while b[i] != 0:
        child, i = rd_object(b, i, depth+1, out)
        children.append(child)
    i += 1
    return (cls, name, props, children), i

def show(obj, depth, flt):
    cls, name, props, children = obj
    hit = flt is None or flt in cls.lower() or flt in name.lower()
    if hit:
        print('  '*depth + f'object {name}: {cls}')
        for p, v in props:
            print('  '*depth + f'  {p} = {v}')
    for c in children:
        show(c, depth+1 if hit else depth, None if hit else flt)

b = open(sys.argv[1], 'rb').read()
flt = sys.argv[2].lower() if len(sys.argv) > 2 else None
i = 0
while True:
    i = b.find(b'TPF0', i)
    if i < 0: break
    try:
        obj, _ = rd_object(b, i+4, 0, None)
        show(obj, 0, flt)
    except Exception as e:
        print(f'# parse error at {i:#x}: {e}', file=sys.stderr)
    i += 4
