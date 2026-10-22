import struct,sys
b=open(sys.argv[1],'rb').read()
o=0x44+4
def dec(o,n,mode):
    out=[];prev=0.0
    while len(out)<n:
        fl=b[o];o+=1
        for i in range(8):
            if len(out)>=n:break
            if fl>>i&1:
                v=struct.unpack_from('<f',b,o)[0];o+=4
            else:
                v=0.0 if mode==0 else prev
            out.append(v);prev=v
    return out,o
for n in (128,256,384,512):
    out,e=dec(o,n,0); print(n,hex(e), b[e:e+8].hex(' '))
out,e=dec(o,384,0)
for k in range(3): print([round(x,4) for x in out[k*128:k*128+24]])
