"""Print a Delphi x64 class from its class reference (the address `lea` loads): name, size, parent, VMT slots.
Usage: vmt.py <dll> <classref-hex> [nslots]"""
import sys; sys.path.insert(0, '/shared/code/fl-rs/tools/plugstate')
from pe import PE
pe = PE(sys.argv[1]); ref = int(sys.argv[2], 16); n = int(sys.argv[3]) if len(sys.argv) > 3 else 22
vmt = ref; assert pe.q(vmt-200) == vmt, hex(pe.q(vmt-200))
print(f"vmt {vmt:#x} selfptr ok")
name = pe.q(vmt - 136); print("class name:", pe.shortstr(name), f"instance size {pe.q(vmt-128):#x}")
parent = pe.q(vmt - 120)
if parent: 
    pv = pe.q(parent); print(f"parent vmt {pv:#x} name", pe.shortstr(pe.q(pv-136)))
for k in range(n):
    print(f"  slot {k:2d} @{vmt+8*k:#x}: {pe.q(vmt+8*k):#010x}")
