#!/usr/bin/env python3
"""Seven c3-hello CPU-time samples per supplied LABEL=BINARY, plus one warmup each."""
import hashlib,json,os,platform,re,resource,subprocess,sys,time
from pathlib import Path
fw=Path(sys.argv[1]).resolve()
arms=[item.split('=',1) for item in sys.argv[2:]]
args=['--chip','c3','--boot','rom','--board','none','--rom',str(fw/'esp32c3_rev3_rom.elf'),'--bootloader',str(fw/'public/c3-hello-bootloader.bin'),'--ptable',str(fw/'public/c3-hello-ptable.bin'),'--app',str(fw/'public/c3-hello_world.bin'),'--flash-mb','4','--mac','00:00:00:00:00:00','--max-seconds','30']
results=[]
def run(label,binary,round):
 while os.getloadavg()[0] >= 3:
  print('waiting for load < 3', os.getloadavg()[0], file=sys.stderr, flush=True); time.sleep(15)
 load_before=os.getloadavg()[0]
 before=resource.getrusage(resource.RUSAGE_CHILDREN);start=time.monotonic()
 p=subprocess.run([binary,*args],stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True)
 wall=time.monotonic()-start;after=resource.getrusage(resource.RUSAGE_CHILDREN)
 report=re.search(rb'stop: (.*?) .*? (\d+) insns .*?emulated ([\d.]+)s \((\d+) cycles\); (\d+) exceptions, (\d+) interrupts',p.stderr)
 if report is None:raise RuntimeError(p.stderr.decode())
 stop,insns,seconds,cycles,exceptions,interrupts=[x.decode() for x in report.groups()]
 sample=dict(label=label,round=round,user_s=after.ru_utime-before.ru_utime,system_s=after.ru_stime-before.ru_stime,wall_s=wall,instructions=int(insns),cycles=int(cycles),emulated_seconds=float(seconds),exceptions=int(exceptions),interrupts=int(interrupts),console_sha256=hashlib.sha256(p.stdout).hexdigest())
 sample['cpu_s']=sample['user_s']+sample['system_s']
 sample['loadavg']=list(os.getloadavg()); sample['load_before']=load_before
 if sample['loadavg'][0] >= 3:
  print('discarding sample: load crossed 3', file=sys.stderr, flush=True); return run(label,binary,round)
 assert sample['instructions']==sample['cycles']==4_800_000_000
 assert sample['exceptions']==0
 Path(f'/tmp/pr167-quiet-{label}-{round}.stdout').write_bytes(p.stdout)
 Path(f'/tmp/pr167-quiet-{label}-{round}.stderr').write_bytes(p.stderr)
 print(json.dumps(sample),flush=True)
 return sample
for label,binary in arms:run(label,binary,'warmup')
for r in range(1,8):
 for label,binary in arms if r%2 else reversed(arms):results.append(run(label,binary,r))
assert len({s['console_sha256'] for s in results})==1
out=dict(command=['BINARY',*[a.replace(str(fw),'FW') for a in args]],measurement='resource.getrusage(RUSAGE_CHILDREN) user + system CPU seconds around one completed CLI subprocess; seven samples per arm after one excluded warmup; reverse order in even rounds; wait until 1-minute load < 3 before every run; discard any run ending at load >= 3',host=dict(system=platform.system(),release=platform.release(),arch=platform.machine()),toolchain=subprocess.check_output(['rustc','+1.99.0','--version'],text=True).strip(),inputs={str(p.relative_to(fw)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [fw/'esp32c3_rev3_rom.elf',fw/'public/c3-hello-bootloader.bin',fw/'public/c3-hello-ptable.bin',fw/'public/c3-hello_world.bin']},binaries={label:hashlib.sha256(Path(binary).read_bytes()).hexdigest() for label,binary in arms},samples=results,limitations=['Local macOS arm64 CPU-time comparison, not browser or hardware timing.','No process inventory retained; unrelated system load was not controlled.','All arms use unchanged native scheduling and identical firmware, instructions, cycles and console hash.'])
Path('/tmp/pr167-quiet.json').write_text(json.dumps(out,indent=2)+'\n')
