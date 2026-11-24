#!/usr/bin/env python3
"""Unpack FL Studio's Windows installer without running it.

    python3 -I tools/fl-extract.py flstudio_win64_26.1.7.5654.exe <out dir>

The installer is NSIS 3 (Unicode, LZMA, not solid), built with two changes
that stop 7-Zip: eight zero bytes sit between the first header and the
compressed header, and each entry of the script holds 8 parameters (36
bytes) where stock NSIS has 6 (28 bytes). With those two known, every file
is an `ExtractFile` (opcode 20) under the last `SetOutPath` (opcode 11),
and its data is one LZMA stream with its own properties.

The install directory (`$INSTDIR`, which this installer keeps in a user
variable) becomes `<out dir>`. Files under other roots (the VST folder,
Common Files, the 32-bit plugins) go to `<out dir>/_unresolved/` and are
not needed. Check the result with `tools/check-corpus.sh "<out dir>/Data"`.
Standard library only; it reads the whole installer (1.1 GB) into memory.
"""

import lzma
import os
import struct
import sys

EXTRACT_FILE = 20
CREATE_DIR = 11
INSTDIR_VARS = (21, 31)  # $INSTDIR, and the user variable this installer copies it to


def lzma_raw(blob):
    """Decode an NSIS LZMA stream: 5 bytes of properties, then raw LZMA."""
    filt = lzma._decode_filter_properties(lzma.FILTER_LZMA1, blob[:5])
    return lzma.LZMADecompressor(lzma.FORMAT_RAW, filters=[filt]).decompress(blob[5:])


def main(installer, out):
    with open(installer, "rb") as f:
        exe = f.read()
    first = exe.find(b"\xef\xbe\xad\xdeNullsoftInst")
    if first < 4:
        sys.exit("no NSIS first header found")
    first -= 4  # the flags word before the signature
    # The compressed header follows the 28-byte first header, after any
    # padding: find its length word, then LZMA properties 0x5d.
    p = first + 28
    while exe[p + 4] != 0x5D:
        p += 4
        if p > first + 64:
            sys.exit("no LZMA header after the first header")
    n = struct.unpack_from("<I", exe, p)[0]
    if not n & 0x80000000:
        sys.exit("the header is not compressed; this installer is laid out differently")
    n &= 0x7FFFFFFF
    header = lzma_raw(exe[p + 4 : p + 4 + n])
    data0 = p + 4 + n

    blocks = [struct.unpack_from("<II", header, 4 + 8 * k) for k in range(8)]
    entries_at, entries = blocks[2]
    strings_at = blocks[3][0]
    size = (strings_at - entries_at) // entries
    words = size // 4
    if size * entries != strings_at - entries_at or words < 7:
        sys.exit("entries do not tile their block")

    def string(off):
        if off < 0:
            return None
        q = strings_at + 2 * off
        out = []
        while True:
            ch = struct.unpack_from("<H", header, q)[0]
            q += 2
            if ch == 0:
                return "".join(out)
            if ch in (1, 2, 3, 4):  # language, shell, variable, skip
                w = struct.unpack_from("<H", header, q)[0]
                q += 2
                if ch == 4:
                    out.append(chr(w))
                    continue
                v = (w & 0x7F) | (((w >> 8) & 0x7F) << 7)
                if ch == 3 and v in INSTDIR_VARS:
                    out.append("\x00I")
                else:
                    out.append("\x00%s%d" % ("LSV"[ch - 1], v))
            else:
                out.append(chr(ch))

    def local(path):
        path = path.replace("\\", "/")
        if path.startswith("\x00I"):
            return path[2:].lstrip("/")
        return "_unresolved/" + path.replace("\x00", "_").lstrip("/")

    outdir = ""
    seen = set()
    written = 0
    for k in range(entries):
        e = struct.unpack_from("<%di" % words, header, entries_at + size * k)
        if e[0] == CREATE_DIR and e[2] == 1:  # SetOutPath
            outdir = string(e[1]) or ""
        elif e[0] == EXTRACT_FILE:
            name = string(e[2]) or ""
            full = name if name.startswith("\x00") else outdir + "\\" + name
            rel = local(full)
            key = (rel, e[3])
            if key in seen:
                continue
            seen.add(key)
            q = data0 + e[3]
            m = struct.unpack_from("<I", exe, q)[0]
            blob = exe[q + 4 : q + 4 + (m & 0x7FFFFFFF)]
            if m & 0x80000000:
                blob = lzma_raw(blob)
            path = os.path.join(out, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "wb") as f:
                f.write(blob)
            written += 1
    print("wrote %d files to %s" % (written, out))


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
