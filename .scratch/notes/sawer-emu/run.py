"""run.py <state name> "<events>" <frames> [tol]: the DLL under unicorn vs the port."""
import glob
import os
import subprocess
import sys

H = os.path.dirname(os.path.abspath(__file__))
WT = os.environ.get('FL_RS', os.path.join(H, '../../..'))
name, events, frames = sys.argv[1], sys.argv[2], sys.argv[3]
tol = sys.argv[4] if len(sys.argv) > 4 else '1e-6'
st = os.path.join(H, 'states', name + '.bin')
tag = ''.join(c if c.isalnum() else '_' for c in name + '_' + events + '_' + frames)
os.makedirs(os.path.join(H, 'out64'), exist_ok=True)
ref = os.path.join(H, 'out64', 'ref_' + tag + '.txt')
port = os.path.join(H, 'out64', 'port_' + tag + '.txt')
if not os.path.exists(ref) or os.path.getsize(ref) == 0:
    with open(ref, 'w') as f:
        subprocess.run(['python3', '-I', os.path.join(H, 'emu_sawer.py'), st, events, frames, '64'],
                       stdout=f, stderr=subprocess.DEVNULL, check=True)
kind = os.environ.get('PROFILE', 'debug')
bins = [b for b in glob.glob(WT + '/target/' + kind + '/deps/plugins-*')
        if not b.endswith('.d') and os.access(b, os.X_OK)]
b = max(bins, key=os.path.getmtime)
env = dict(os.environ, SAWER_STATE=st, SAWER_EVENTS=events, SAWER_FRAMES=frames, SAWER_OUT=port,
           FL_RS_CORPUS=os.environ.get('FL_RS_CORPUS', '/root/fl-install/Data'))
subprocess.run([b, 'sawer::tests::dump_for_dll', '--exact'], env=env, stdout=subprocess.DEVNULL,
               stderr=subprocess.DEVNULL, check=True)
subprocess.run(['python3', '-I', os.path.join(H, 'cmp.py'), ref, port, tol])
