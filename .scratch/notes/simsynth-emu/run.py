"""run.py <state name> <key> <frames> <release> : DLL under unicorn vs the port, compared."""
import glob
import os
import subprocess
import sys

H = os.path.dirname(os.path.abspath(__file__))
WT = os.environ.get('FL_RS', os.path.join(H, '../../..'))
DLL = os.environ.get('SIMSYNTH_DLL', '/root/fl-install/Plugins/Fruity/Generators/SimSynth/SimSynth_Eng_x64.dll')
name, key, frames, rel = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
tol = sys.argv[5] if len(sys.argv) > 5 else '1e-6'
st = os.path.join(os.environ.get('SIMSYNTH_WORK', H), 'states', name + '.bin')
tag = ''.join(c if c.isalnum() else '_' for c in name)
os.makedirs(os.path.join(os.environ.get('SIMSYNTH_WORK', H), 'out'), exist_ok=True)
ref = os.path.join(os.environ.get('SIMSYNTH_WORK', H), 'out', f'ref_{tag}_{key}_{frames}_{rel}.txt')
port = os.path.join(os.environ.get('SIMSYNTH_WORK', H), 'out', f'port_{tag}_{key}_{frames}_{rel}.txt')
if not os.path.exists(ref) or os.path.getsize(ref) == 0:
    ev = f'on:{key}:0' + (f' off:{key}:{rel}' if int(rel) >= 0 else '')
    with open(ref, 'w') as f:
        subprocess.run(['python3', '-I', os.path.join(H, 'emu_simsynth.py'), DLL, st, ev, frames, '512'],
                       stdout=f, stderr=subprocess.DEVNULL, check=True)
bins = [b for b in glob.glob(WT + '/target/release/deps/plugins-*') if not b.endswith('.d')]
b = max(bins, key=os.path.getmtime)
env = dict(os.environ, SIMSYNTH_STATE=st, SIMSYNTH_KEY=key, SIMSYNTH_FRAMES=frames,
           SIMSYNTH_OUT=port)
if int(rel) >= 0:
    env['SIMSYNTH_RELEASE'] = rel
subprocess.run([b, 'simsynth::tests::dump_for_dll', '--exact'], env=env, stdout=subprocess.DEVNULL,
               stderr=subprocess.DEVNULL, check=True)
subprocess.run(['python3', '-I', os.path.join(H, 'cmp.py'), ref, port, tol])
