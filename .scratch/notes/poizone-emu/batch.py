"""batch.py <every nth> "<events>" <frames> [nofx]: run.py over the states."""
import os
import struct
import subprocess
import sys

H = os.path.dirname(os.path.abspath(__file__))
step = int(sys.argv[1])
events, frames = sys.argv[2], sys.argv[3]
nofx = len(sys.argv) > 4
zero = (31, 69) if nofx and sys.argv[4] == 'nofx' else (69,)
pre = 'nofx_' if nofx and sys.argv[4] == 'nofx' else 'noarp_'
names = sorted(f[:-4] for f in os.listdir(os.path.join(H, 'states')) if f.endswith('.bin') and not f.startswith(('nofx_', 'noarp_')))
if nofx:
    for name in names[::step]:
        b = bytearray(open(os.path.join(H, 'states', name + '.bin'), 'rb').read())
        for p in zero:
            struct.pack_into('<f', b, 12 + 4 * p, 0.0)
        open(os.path.join(H, 'states', pre + name + '.bin'), 'wb').write(bytes(b))
for name in names[::step]:
    n = (pre + name) if nofx else name
    r = subprocess.run(['python3', '-I', os.path.join(H, 'run.py'), n, events, frames, '0'],
                       capture_output=True, text=True)
    line = (r.stdout.splitlines() or ['? ' + r.stderr[-200:]])[0]
    print(name, line, flush=True)
