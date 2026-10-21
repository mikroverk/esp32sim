#!/usr/bin/env python3
"""Download/hash pinned public driver; compile its actual filters and check FIFO vectors."""
from pathlib import Path
import hashlib, json, os, subprocess, sys, urllib.request
r=Path(sys.argv[1]).resolve(); out=Path(sys.argv[2]).resolve(); out.mkdir(parents=True,exist_ok=True)
url='https://raw.githubusercontent.com/espressif/esp32-camera/b817e9b81041980608d1a3d3e0b08d86e3666382/target/esp32/ll_cam.c'
source=urllib.request.urlopen(url).read(); text=source.decode(); (out/'ll_cam.c').write_bytes(source)
manifest=r/'docs/evidence/classic-camera-dma/fifo/source.json'
assert hashlib.sha256(source).hexdigest()==json.loads(manifest.read_text())['sha256']
def function(name):
 start=text.index('static size_t IRAM_ATTR '+name+'('); end=text.index('\n}',start)+2
 return text[start:end]
struct=text[text.index('typedef union {'):text.index('} dma_elem_t;')+len('} dma_elem_t;')]
filters=['ll_cam_dma_filter_grayscale','ll_cam_dma_filter_grayscale_highspeed','ll_cam_dma_filter_yuyv','ll_cam_dma_filter_yuyv_highspeed']
c='#include <stdint.h>\n#include <stddef.h>\n#include <stdio.h>\n#include <stdlib.h>\n#define IRAM_ATTR\n'+struct+'\n'+'\n'.join(function(n) for n in filters)
c+='''
int main(int argc, char **argv) {
 if(argc!=4) return 2;
 uint8_t src[4096]={0}, dst[4096]={0};
 FILE *f=fopen(argv[2],"rb"); if(!f) return 3;
 size_t len=fread(src,1,sizeof(src),f); fclose(f);
 size_t n=0; int mode=atoi(argv[1]), gray=atoi(argv[3]);
 if(mode==1) n=gray?ll_cam_dma_filter_grayscale(dst,src,len):ll_cam_dma_filter_yuyv(dst,src,len);
 else n=gray?ll_cam_dma_filter_grayscale_highspeed(dst,src,len):ll_cam_dma_filter_yuyv_highspeed(dst,src,len);
 return fwrite(dst,1,n,stdout)==n?0:4;
}
'''
(out/'filters.c').write_text(c)
subprocess.run(['cc','-std=c11','-Wall','-Wextra','-Werror',str(out/'filters.c'),'-o',str(out/'filters')],check=True)
subprocess.run(['nice','-n','10','cargo','+1.99.0','run','--release','-p','esp32','--example','camera_fifo_probe','--',str(out/'vectors')],cwd=r,check=True)
rows=[]; limits=[]
for wire_file in sorted((out/'vectors').glob('*.wire')):
 wire=wire_file.read_bytes()
 for mode in [1,3]:
  packed=wire_file.with_suffix(f'.{mode}.fifo')
  for gray in [0,1]:
   # Mode-1 filters and mode-3 YUV require eight-byte sensor groups.
   # The highspeed YUV sample2 remainder is for overlapping mode 0, not 3.
   # Row12 exercises the actual mode-3 grayscale remainder only.
   got=subprocess.check_output([str(out/'filters'),str(mode),str(packed),str(gray)])
   expected=wire[::2] if gray else wire
   if len(wire)%8 and not (mode==3 and gray):
    assert got != expected, 'driver partial-group limitation changed'
    limits.append(dict(vector=wire_file.stem,mode=mode,grayscale=bool(gray),returned_bytes=len(got),output_hex=got.hex(),expected_hex=expected.hex()))
    continue
   assert got==expected,(wire_file.name,mode,gray,got.hex(),expected.hex())
   rows.append(dict(vector=wire_file.stem,mode=mode,grayscale=bool(gray),bytes=len(got),sha256=hashlib.sha256(got).hexdigest()))
(out/'driver-results.json').write_text(json.dumps(dict(source_url=url,source_sha256=hashlib.sha256(source).hexdigest(),rows=rows,partial_group_limits=limits),indent=2)+'\n')
print(f'PASS: {len(rows)} actual public C filter comparisons')
