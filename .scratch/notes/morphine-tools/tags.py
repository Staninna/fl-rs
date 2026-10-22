import sys,re,struct
b=open(sys.argv[1],'rb').read()
lim=int(sys.argv[2]) if len(sys.argv)>2 else 200
n=0
for m in re.finditer(rb'0[A-Za-z]{3}',b):
    o=m.start()
    nxt=b[o+4:o+20]
    print(hex(o), m.group().decode(), nxt.hex(' '))
    n+=1
    if n>lim: break
