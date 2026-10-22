import sys,pefile,capstone,struct
pe=pefile.PE('/root/work/morphine/Morphine_x64.dll'); base=pe.OPTIONAL_HEADER.ImageBase; d=pe.get_memory_mapped_image()
md=capstone.Cs(capstone.CS_ARCH_X86,capstone.CS_MODE_64); md.detail=False
a=int(sys.argv[1],16); n=int(sys.argv[2]) if len(sys.argv)>2 else 64
for ins in md.disasm(d[a-base:a-base+n*16],a):
    s='%x: %s %s'%(ins.address,ins.mnemonic,ins.op_str)
    if 'rip' in ins.op_str and '[' in ins.op_str:
        import re
        m=re.search(r'\[rip ([+-]) 0x([0-9a-f]+)\]',ins.op_str)
        if m:
            t=ins.address+ins.size+(int(m.group(2),16)*(1 if m.group(1)=='+' else -1))
            o=t-base
            if 0<=o<len(d)-8:
                s+='   ; %x f32=%r f64=%r'%(t,struct.unpack_from('<f',d,o)[0],struct.unpack_from('<d',d,o)[0])
    print(s)
    n-=1
    if n<=0: break
