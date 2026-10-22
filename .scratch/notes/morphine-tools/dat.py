import sys,struct,pefile
pe=pefile.PE('/root/work/morphine/Morphine_x64.dll'); base=pe.OPTIONAL_HEADER.ImageBase; data=pe.get_memory_mapped_image()
for a in sys.argv[1:]:
    n=1
    if ':' in a: a,n=a.split(':'); n=int(n)
    o=int(a,16)-base
    for k in range(n):
        b=data[o+4*k:o+4*k+8]
        print(hex(base+o+4*k), 'f32=%r'%struct.unpack('<f',b[:4])[0], 'i32=%d'%struct.unpack('<i',b[:4])[0], 'f64=%r'%struct.unpack('<d',b)[0])
