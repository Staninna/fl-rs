"""Print the f32, f64 and i32 at virtual addresses of a PE image.

usage: python3 -I peconst.py <dll> <addr>[:<count>[:<kind>]] ...
kind: f (f32 and f64, default), i (i32), p (pointer), b (bytes)
"""

import struct
import sys

IMAGE_BASE = 0x400000


def sections(data):
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    count = struct.unpack_from("<H", data, pe + 6)[0]
    opt_size = struct.unpack_from("<H", data, pe + 20)[0]
    at = pe + 24 + opt_size
    out = []
    for _ in range(count):
        va, raw_size, raw = struct.unpack_from("<III", data, at + 12)
        out.append((va, raw_size, raw))
        at += 40
    return out


def file_offset(secs, va):
    rva = va - IMAGE_BASE
    for start, size, raw in secs:
        if start <= rva < start + size:
            return raw + rva - start
    raise SystemExit(f"{va:#x} is not in a section")


def main():
    data = open(sys.argv[1], "rb").read()
    secs = sections(data)
    for spec in sys.argv[2:]:
        parts = spec.split(":")
        addr = int(parts[0], 16)
        count = int(parts[1]) if len(parts) > 1 else 1
        kind = parts[2] if len(parts) > 2 else "f"
        for k in range(count):
            if kind == "f":
                at = file_offset(secs, addr + 4 * k)
                f32 = struct.unpack_from("<f", data, at)[0]
                f64 = struct.unpack_from("<d", data, at)[0]
                i32 = struct.unpack_from("<i", data, at)[0]
                print(f"{addr + 4 * k:#010x}  f32 {f32!r:<16} f64 {f64!r:<24} i32 {i32}")
            elif kind == "d":
                at = file_offset(secs, addr + 8 * k)
                print(f"{addr + 8 * k:#010x}  f64 {struct.unpack_from('<d', data, at)[0]!r}")
            elif kind == "i":
                at = file_offset(secs, addr + 4 * k)
                print(f"{addr + 4 * k:#010x}  i32 {struct.unpack_from('<i', data, at)[0]}")
            elif kind == "p":
                at = file_offset(secs, addr + 8 * k)
                print(f"{addr + 8 * k:#010x}  ptr {struct.unpack_from('<Q', data, at)[0]:#x}")
            elif kind == "b":
                at = file_offset(secs, addr)
                print(f"{addr:#010x}  {data[at:at + count].hex()}")
                break


main()
