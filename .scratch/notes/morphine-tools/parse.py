import struct,sys
NAMES={}
for line in open('/root/work/morphine/names.txt'):
    k,_,n=line.strip().split(' ',2); NAMES[int(k)]=n
class R:
    def __init__(s,b,o=0,e=None): s.b=b; s.o=o; s.e=len(b) if e is None else e
    def u32(s): v=struct.unpack_from('<I',s.b,s.o)[0]; s.o+=4; return v
    def i32(s): v=struct.unpack_from('<i',s.b,s.o)[0]; s.o+=4; return v
    def f32(s): v=struct.unpack_from('<f',s.b,s.o)[0]; s.o+=4; return v
    def tag(s): v=s.b[s.o:s.o+4]; s.o+=4; return v
    def sub(s,n): r=R(s.b,s.o,s.o+n); s.o+=n; return r
def mbp(r):
    assert r.tag()==b'0PBM'
    arrs=[]
    for a in range(3):
        v=[0.0]*128; i=0
        while i<128 and r.o<r.e:
            fl=r.b[r.o]; r.o+=1
            for bit in range(8):
                if fl>>bit&1: v[i]=r.f32()
                i+=1
        arrs.append(v)
    return arrs
def msp(r):
    assert r.tag()==b'0PSM'
    n=r.u32(); frames=[]
    for i in range(n):
        dur=r.u32(); g=r.f32(); sz=r.u32(); frames.append((dur,g,mbp(r.sub(sz))))
    tr=None
    if r.e-r.o>=4:
        sz=r.u32(); t=R(r.b,r.o-4,r.o-4+sz); r.o+=sz-4; t.o+=4
        vals=[t.i32(),t.i32(),t.i32(),t.i32(),t.f32(),t.f32(),t.f32(),t.i32()]
        tr=dict(loop_start=vals[0],loop_end=vals[1],release_loop=vals[2],loop_mode=vals[3],rate=vals[4],keytrk=vals[5],veltrk=vals[6],sync=vals[7],sz=sz)
        if sz>=0x424:
            tr['vs']=[t.f32() for _ in range(128)]; tr['gain']=[t.f32() for _ in range(128)]
    return frames,tr
def mzh(r):
    assert r.tag()==b'0HZM'
    n=r.u32(); zones=[]
    for i in range(n):
        keys=r.i32(); lvl=r.f32(); sz=r.u32(); zones.append((keys,lvl,msp(r.sub(sz))))
    return zones
def mvc(r):
    assert r.tag()==b'0CVM'
    sz=r.u32(); z=mzh(r.sub(sz)); tr=None
    if r.e-r.o>=4:
        sz=r.u32(); t=R(r.b,r.o-4,r.o-4+sz); r.o+=sz-4; t.o+=4
        tr=dict(pwm_rate=t.f32(),pwm_mode=t.i32(),pwm_phase=t.f32(),pwm_free=t.i32(),detune=t.f32(),pan=t.f32())
    return z,tr
def mml(r):
    assert r.tag()==b'0LMM'
    n=r.u32(); st=r.u32(); pts=[]
    for i in range(n):
        s=r.sub(st); pts.append((s.f32(),s.f32(),s.i32()))
    sz=r.u32(); t=R(r.b,r.o-4,r.o-4+sz); r.o+=sz-4; t.o+=4
    v=[t.i32(),t.i32(),t.i32(),t.i32()]+[t.f32() for _ in range(8)]+[t.i32() for _ in range(4)]+[t.f32() for _ in range(4)]
    return pts,v
def meg(r):
    assert r.tag()==b'0GEM'
    n=r.u32(); st=r.u32(); pts=[]
    for i in range(n):
        s=r.sub(st); pts.append((s.f32(),s.i32()))
    sz=r.u32(); t=R(r.b,r.o-4,r.o-4+sz); r.o+=sz-4; t.o+=4
    v=[]
    while t.o<t.e: v.append(t.b[t.o:t.o+4]); t.o+=4
    v=[struct.unpack('<i',x)[0] if abs(struct.unpack('<i',x)[0])<100000 else round(struct.unpack('<f',x)[0],4) for x in v]
    return pts,v,sz
def mdm(r):
    assert r.tag()==b'0DMM'
    n=r.u32(); egs=[]
    for i in range(n):
        sz=r.u32(); egs.append(meg(r.sub(sz)))
    sz=r.u32(); t=R(r.b,r.o-4,r.o-4+sz); r.o+=sz-4; t.o+=4
    raw=[t.i32() for _ in range(24)]+[round(t.f32(),4) for _ in range(12)]
    t.o=4+48*4
    amts=[]
    t2=R(r.b,0); 
    return egs,raw
def top(b):
    r=R(b)
    assert r.tag()==b'SSLF'; n=r.u32()
    assert r.tag()==b'0roM'
    assert r.tag()==b'0RPM'
    p=r
    gens=[]
    for g in range(4):
        s=p.u32(); gens.append(mvc(p.sub(s)))
    s=p.u32(); morph=mml(p.sub(s))
    s=p.u32(); mods=mdm(p.sub(s))
    s=p.u32(); st=R(b,p.o-4,p.o-4+s); p.o+=s-4
    params={}
    vals=[struct.unpack_from('<f',b,st.o+o)[0] for o in range(0,s,4)]
    name=b[st.o+0x4b0:st.o+0x5b0].split(b'\0')[0].decode('latin1')
    noise=b[st.o+0xac:st.o+0x4ac].split(b'\0')[0].decode('latin1')
    def at(o): return struct.unpack_from('<f',b,st.o+o)[0]
    for i in range(0x2b): params[i]=at(4+i*4)  # approx: ordering as prog offsets
    return dict(gens=gens,morph=morph,mods=mods,struct=st,name=name,noise=noise,rest=r.e-p.o, size=s)
if __name__=='__main__':
    for f in sys.argv[1:]:
        b=open(f,'rb').read(); d=top(b)
        print('==',f,d['name'],'noise=',d['noise'],'structsz',hex(d['size']))
        for gi,(z,tr) in enumerate(d['gens']):
            print(' gen',gi,tr)
            for keys,lvl,(frames,ptr) in z:
                print('   zone keys',keys,'lvl',lvl,'frames',len(frames),'durs',sorted(set(fr[0] for fr in frames)),'trailer',{k:v for k,v in (ptr or {}).items() if k not in('vs','gain')})
                if ptr and 'vs' in ptr:
                    if any(x!=1.0 for x in ptr['vs']) or any(x!=1.0 for x in ptr['gain']): print('     vs/gain non-default', ptr['vs'][:6], ptr['gain'][:6])
        print(' morph',d['morph'])
        for e in d['mods'][0]: print(' eg',e)
        print(' matrix',d['mods'][1])
