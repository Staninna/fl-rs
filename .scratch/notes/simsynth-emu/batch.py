import os, subprocess, sys
H = os.path.dirname(os.path.abspath(__file__))
names = sorted(f[:-4] for f in os.listdir(os.path.join(os.environ.get('SIMSYNTH_WORK', H), 'states')))
step = int(sys.argv[1]) if len(sys.argv) > 1 else 1
key = sys.argv[2] if len(sys.argv) > 2 else '48'
frames = sys.argv[3] if len(sys.argv) > 3 else '6000'
rel = sys.argv[4] if len(sys.argv) > 4 else '3000'
for name in names[::step]:
    r = subprocess.run(['python3', '-I', os.path.join(H, 'run.py'), name, key, frames, rel],
                       capture_output=True, text=True)
    line = (r.stdout.splitlines() or ['?'])[0]
    print(name, line, flush=True)
