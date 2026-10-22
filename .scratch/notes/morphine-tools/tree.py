import sys,re
src=open('/root/work/morphine/out/all.c').read()
idx={}
for m in re.finditer(r'// ===== (\S+) @ ([0-9a-f]+) size=(\d+)',src):
    idx[m.group(2)]=m.start()
def body(a):
    i=idx.get(a)
    if i is None: return None
    j=src.find('\n// =====',i+10)
    return src[i:j if j>0 else len(src)]
depth=int(sys.argv[1]); maxsize=int(sys.argv[2]); roots=[a.lower().replace('0x','') for a in sys.argv[3:]]
seen=set(); out=[]
def go(a,d):
    if a in seen: return
    seen.add(a); b=body(a)
    if b is None: return
    size=int(re.search(r'size=(\d+)',b).group(1))
    if size>maxsize and a not in roots: out.append('// skipped big %s size=%d\n'%(a,size)); return
    out.append(b)
    if d<depth:
        for c in re.findall(r'FUN_([0-9a-f]{9})\(',b): go(c,d+1)
for r in roots: go(r,0)
print('\n'.join(out))
