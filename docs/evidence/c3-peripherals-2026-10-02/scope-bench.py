#!/usr/bin/env python3
"""Seven c3-hello CPU-time samples per supplied LABEL=BINARY, plus one warmup each."""
import hashlib,json,os,platform,re,resource,subprocess,sys,time
from pathlib import Path
fw=Path(sys.argv[1]).resolve()
arms=[item.split('=',1) for item in sys.argv[2:]]
args=['--chip','c3','--boot','rom','--board','none','--rom',str(fw/'esp32c3_rev3_rom.elf'),'--bootloader',str(fw/'public/c3-hello-bootloader.bin'),'--ptable',str(fw/'public/c3-hello-ptable.bin'),'--app',str(fw/'public/c3-hello_world.bin'),'--flash-mb','4','--mac','00:00:00:00:00:00','--max-seconds','30']
results=[]
binary_hashes={label:hashlib.sha256(Path(binary).read_bytes()).hexdigest() for label,binary in arms}
def load_one():
 raw=subprocess.check_output(['uptime'],text=True)
 return float(re.search(r'load averages?:\s*([0-9.]+)',raw).group(1))
def run(label,binary,round):
 while (load_before:=load_one()) >= 3:
  print('waiting for one-minute load below 3',flush=True);time.sleep(30)
 before=resource.getrusage(resource.RUSAGE_CHILDREN);start=time.monotonic()
 p=subprocess.run([binary,*args],stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True)
 wall=time.monotonic()-start;after=resource.getrusage(resource.RUSAGE_CHILDREN)
 report=re.search(rb'stop: (.*?) .*? (\d+) insns .*?emulated ([\d.]+)s \((\d+) cycles\); (\d+) exceptions, (\d+) interrupts',p.stderr)
 if report is None:raise RuntimeError(p.stderr.decode())
 stop,insns,seconds,cycles,exceptions,interrupts=[x.decode() for x in report.groups()]
 sample=dict(label=label,round=round,user_s=after.ru_utime-before.ru_utime,system_s=after.ru_stime-before.ru_stime,wall_s=wall,instructions=int(insns),cycles=int(cycles),emulated_seconds=float(seconds),exceptions=int(exceptions),interrupts=int(interrupts),console_sha256=hashlib.sha256(p.stdout).hexdigest())
 sample['cpu_s']=sample['user_s']+sample['system_s']
 sample['load_before']=load_before
 sample['loadavg']=list(os.getloadavg())
 assert sample['instructions']==sample['cycles']==4_800_000_000
 assert sample['exceptions']==0 and sample['interrupts']==3028
 Path(f'/tmp/pin-scope-final-{label}-{round}.stdout').write_bytes(p.stdout)
 Path(f'/tmp/pin-scope-final-{label}-{round}.stderr').write_bytes(p.stderr)
 print(json.dumps(sample),flush=True)
 assert float(f"{sample['loadavg'][0]:.2f}")<3, 'load rose above the acceptance limit'
 return sample
for label,binary in arms:run(label,binary,'warmup')
for r in range(1,8):
 for label,binary in arms if r%2 else reversed(arms):results.append(run(label,binary,r))
assert len({s['console_sha256'] for s in results})==1
assert binary_hashes=={label:hashlib.sha256(Path(binary).read_bytes()).hexdigest() for label,binary in arms}
out=dict(command=['BINARY',*[a.replace(str(fw),'FW') for a in args]],measurement='resource.getrusage(RUSAGE_CHILDREN) user + system CPU seconds around one completed CLI subprocess; seven samples per arm after one excluded warmup; reverse order in even rounds',host=dict(system=platform.system(),release=platform.release(),arch=platform.machine()),toolchain=subprocess.check_output(['rustc','+1.99.0','--version'],text=True).strip(),inputs={str(p.relative_to(fw)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [fw/'esp32c3_rev3_rom.elf',fw/'public/c3-hello-bootloader.bin',fw/'public/c3-hello-ptable.bin',fw/'public/c3-hello_world.bin']},binaries={label:hashlib.sha256(Path(binary).read_bytes()).hexdigest() for label,binary in arms},samples=results,limitations=['Local macOS arm64 CPU-time comparison, not browser or hardware timing.','No process inventory retained; uptime one-minute load is below 3 before every subprocess, and rounded post-run load is also below 3. Aggregate load cannot exclude all brief contention.','All arms use unchanged native scheduling and identical firmware, instructions, cycles and console hash.'])
Path('/tmp/pin-scope-final.json').write_text(json.dumps(out,indent=2)+'\n')
