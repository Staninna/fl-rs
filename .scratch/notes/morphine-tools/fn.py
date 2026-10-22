import sys,re
src=open('/root/work/morphine/out/all.c').read()
for a in sys.argv[1:]:
    a=a.lower().replace('0x','')
    i=src.find(' @ %s size='%a)
    if i<0: print('no',a); continue
    i=src.rfind('// =====',0,i+1)
    j=src.find('\n// =====',i+10)
    print(src[i:j if j>0 else len(src)])
