import sys, struct, collections
def states(path):
    out=[]
    for line in open(path, encoding='utf-8', errors='replace'):
        parts=line.rstrip('\n').split('\t')
        if len(parts)<4: continue
        hexs=parts[3].split()
        try: b=bytes(int(h,16) for h in hexs)
        except ValueError: continue
        out.append((parts[0],parts[1],b))
    return out
def params(b):
    v=struct.unpack_from('<i',b,0)[0]
    n=99 if v<9 else 100
    p=list(struct.unpack_from('<%di'%n,b,4))+([0] if n==99 else [])
    at=4+4*n
    sel=list(struct.unpack_from('<7i',b,at)) if v>3 else [0]*7
    at+=28 if v>3 else 0
    img=0
    if v>7:
        img=struct.unpack_from('<i',b,at)[0]
    return v,p,sel,img
for path in sys.argv[1:]:
    S=states(path); print(path.split('/')[-1], len(S))
    c=collections.defaultdict(collections.Counter)
    for f,ch,b in S:
        v,p,sel,img=params(b)
        c['version'][v]+=1; c['filter'][sel[0]]+=1; c['res'][sel[1]]+=1; c['phaser type'][sel[2]]+=1; c['comp'][sel[3]]+=1; c['colour'][sel[4]]+=1; c['image'][img]+=1
        c['pluck<1000'][p[31]<1000]+=1; c['unison>1'][p[65]>1]+=1; c['pmix>0'][p[71]>0]+=1; c['sfxmix>0'][p[79]>0]+=1
        c['chorus type'][p[90]]+=1; c['delay type'][p[92]]+=1; c['reverb type'][p[96]]+=1; c['trem>0'][p[25]>0]+=1; c['grit!=0'][p[40]!=0]+=1
        c['lfo shape'][p[86]]+=1; c['res amt>0'][p[59]>0]+=1; c['osc>0'][p[62]>0]+=1; c['vib>0'][p[41]>0]+=1; c['plfo!=0'][p[42]!=0]+=1
        c['flfo!=0'][p[58]!=0]+=1; c['fenv!=0'][p[52]!=0]+=1; c['detune!=0'][p[39]!=0]+=1; c['hzslope'][p[55]]+=1; c['reshz'][p[60]]+=1; c['mono'][p[44]]+=1; c['porta'][p[43]]+=1
        c['subs>0'][any(x>0 for x in p[2:5])]+=1; c['global lfo'][p[87]]+=1; c['lfo speed<0'][p[89]<0]+=1; c['attack>0'][p[26]>0]+=1; c['decay<1000'][p[27]<1000]+=1
        c['fenvalt'][p[47]]+=1; c['resadaptive'][p[63]]+=1; c['resmt'][p[64]]+=1; c['fattype'][p[70]]+=1; c['sfxtype'][p[84]]+=1; c['sfxpos'][p[85]]+=1
    for k in sorted(c): print(' ',k, dict(sorted(c[k].items())))
