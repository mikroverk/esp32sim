from pathlib import Path
import random, subprocess, tempfile
rng = random.Random(199)
s = '''use esp32::crypto::ClassicRsa;
use esp_periph::Device;
fn load(r: &mut ClassicRsa, base: u32, a: &[u32]) { for (i,v) in a.iter().enumerate() { r.write(base+4*i as u32,*v); } }
fn main() {
'''
cases = 0
for mode in range(8):
  words = 16*(mode+1)
  for short in [False, True]:
    bits = words*32 if not short else 67
    for _ in range(3):
      m = rng.getrandbits(bits) | 1 | (1 << (bits-1))
      x, y = rng.randrange(m), rng.randrange(m)
      want = x*y*pow(1<<(words*32),-1,m)%m
      def limbs(n): return '['+','.join(str((n>>(i*32))&0xffffffff) for i in range(words))+']'
      mp=(-pow(m,-1,1<<32))&0xffffffff
      s += '{ let mut r=ClassicRsa::new();\n'
      s += f'load(&mut r,0,&{limbs(m)});load(&mut r,0x600,&{limbs(x)});load(&mut r,0x200,&{limbs(y)});\n'
      s += f'r.write(0x800,{mp});r.write(0x80c,{mode});r.write(0x810,1);\n'
      s += f'for (i, w) in {limbs(want)}.iter().enumerate() {{ assert_eq!(r.read(0x200+4*i as u32),*w,"case {cases} limb {{}}",i); }} }}\n'
      cases+=1
s += f'println!("{cases} independent Python Montgomery cases passed, all 512..4096-bit modes and short moduli"); }}\n'
temporary = tempfile.TemporaryDirectory(prefix='classic-crypto-check-')
p = Path(temporary.name) / 'montgomery_review.rs'
p.write_text(s)
repo=Path(__file__).resolve().parents[3]
deps=repo/'target/debug/deps'
def newest(pattern): return str(max(deps.glob(pattern), key=lambda p:p.stat().st_mtime))
subprocess.run(['rustc','--edition=2021',str(p),'-L','dependency='+str(deps),'--extern','esp32='+newest('libesp32-*.rlib'),'--extern','esp_periph='+newest('libesp_periph-*.rlib'),'-o',str(p.with_suffix(''))],check=True)
subprocess.run([str(p.with_suffix(''))],check=True)

temporary.cleanup()
