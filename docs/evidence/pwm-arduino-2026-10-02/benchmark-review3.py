import argparse, hashlib, json, platform, re, resource, statistics, subprocess, time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('workload');p.add_argument('output');p.add_argument('arms',nargs='+');p.add_argument('--rounds',type=int,default=5);p.add_argument('--fw',type=Path,required=True);a=p.parse_args()
fw=a.fw.resolve();work=a.workload
manifest=json.loads((fw/(work+'.json')).read_text())
exe={'c6-hello':'esp32sim-c6','c3-hello':'esp32sim-c3'}.get(work,'esp32sim')
args=['--boot','rom','--rom',str(fw/manifest['files']['rom']),'--flash-mb',str(manifest['flash_mb'])]
for key in ['bootloader','ptable','app']:args+=['--'+key,str(fw/manifest['files'][key])]
if exe=='esp32sim':args+=['--board',manifest['board'],'--psram-mb',str(manifest['psram_mb'])]
for off,file in manifest.get('flash_at',{}).items():args+=['--flash-at',off+'='+str(fw/file)]
args+=['--max-seconds','30','--no-dump']
arms=[x.split('=',1) for x in a.arms];samples=[];waits=[]
def load():
 s=subprocess.check_output(['uptime'],text=True)
 return float(re.search(r'load averages?:\s*([\d.]+)',s).group(1))
def run(label,directory,round):
 while True:
  before=load()
  if before>=3:
   waits.append(before);print('waiting load',before,flush=True);time.sleep(30);continue
  start=resource.getrusage(resource.RUSAGE_CHILDREN);t=time.monotonic()
  r=subprocess.run([str(Path(directory)/exe),*args],capture_output=True,check=True)
  end=resource.getrusage(resource.RUSAGE_CHILDREN);wall=time.monotonic()-t;after=load()
  if after>=3:print('discard load crossed threshold',before,after,flush=True);continue
  report=re.search(rb'stop: (.*?) .*? (\d+) insns .*?emulated ([\d.]+)s \((\d+) cycles\); (\d+) exceptions, (\d+) interrupts',r.stderr)
  if not report:raise RuntimeError(r.stderr.decode())
  stop,insns,seconds,cycles,exceptions,interrupts=[x.decode() for x in report.groups()]
  sample=dict(arm=label,round=round,user_s=end.ru_utime-start.ru_utime,system_s=end.ru_stime-start.ru_stime,wall_s=wall,load_before=before,load_after=after,instructions=int(insns),cycles=int(cycles),exceptions=int(exceptions),interrupts=int(interrupts),console_sha256=hashlib.sha256(r.stdout).hexdigest())
  sample['cpu_s']=sample['user_s']+sample['system_s'];print(json.dumps(sample),flush=True)
  return sample
for label,directory in arms:run(label,directory,0)
for round in range(1,a.rounds+1):
 for label,directory in (arms if round%2 else list(reversed(arms))):
  samples.append(run(label,directory,round))
  Path(a.output+'.partial').write_text(json.dumps(samples,indent=2)+'\n')
assert len({(s['instructions'],s['cycles'],s['console_sha256'],s['exceptions'],s['interrupts']) for s in samples})==1
out=dict(workload=work,command=['BINARY',*[v.replace(str(fw),'FW') for v in args]],samples=samples,wait_loads=waits,host=dict(system=platform.system(),release=platform.release(),arch=platform.machine()),inputs={v:hashlib.sha256((fw/v).read_bytes()).hexdigest() for v in [*manifest['files'].values(),*manifest.get('flash_at',{}).values()]},binaries={label:hashlib.sha256((Path(directory)/exe).read_bytes()).hexdigest() for label,directory in arms},medians={label:{k:statistics.median(s[k] for s in samples if s['arm']==label) for k in ['user_s','system_s','cpu_s']} for label,_ in arms})
Path(a.output).write_text(json.dumps(out,indent=2)+'\n');print(json.dumps(out['medians']),flush=True)
