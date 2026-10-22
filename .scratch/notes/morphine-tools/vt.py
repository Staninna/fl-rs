import pefile,struct,re,sys
pe=pefile.PE('/root/work/morphine/Morphine_x64.dll'); base=pe.OPTIONAL_HEADER.ImageBase; d=pe.get_memory_mapped_image()
names={}
for m in re.finditer(rb'\.\?AV([A-Za-z0-9_@?$<>]+)@@\0',d):
    td=m.start()-0x10  # type descriptor starts 16 bytes before name
    names[td]=m.group(1).decode()
# COL: sig(4)=1, off, cdoff, pTypeDesc(rva), pClassDesc(rva), pSelf(rva)
res=[]
for td,n in names.items():
    rva=struct.pack('<I',td)
    for m in re.finditer(re.escape(rva),d):
        col=m.start()-12
        if col<0: continue
        sig,off=struct.unpack_from('<II',d,col)
        if sig!=1: continue
        selfr=struct.unpack_from('<I',d,col+20)[0]
        if selfr!=col: continue
        # vtable: pointer to COL followed by vtable
        p=struct.pack('<Q',base+col)
        for mm in re.finditer(re.escape(p),d):
            vt=mm.start()+8
            res.append((n,off,base+vt))
want=sys.argv[1:] 
for n,off,vt in sorted(res):
    if not want or n in want:
        slots=[struct.unpack_from('<Q',d,vt-base+8*i)[0] for i in range(int(90))]
        k=0
        while k<len(slots) and 0x180001000<=slots[k]<0x180158000: k+=1
        print(n,hex(off),hex(vt),' '.join(hex(s) for s in slots[:k]))
