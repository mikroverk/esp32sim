import resource,subprocess,sys,statistics,re,os
import argparse
parser=argparse.ArgumentParser()
parser.add_argument('--fw', required=True)
parser.add_argument('--model', required=True)
parser.add_argument('--main', required=True)
parser.add_argument('--pr', required=True)
args=parser.parse_args()
F=args.fw
cases={'c6-hello':['esp32sim-c6','--boot','rom','--rom',f'{F}/esp32c6_rev0_rom.elf','--flash-mb','4','--bootloader',f'{F}/public/c6-hello-bootloader.bin','--ptable',f'{F}/public/c6-hello-ptable.bin','--app',f'{F}/public/c6-hello_world.bin','--max-seconds','30','--no-dump']}
cases['s3-hello']=['esp32sim','--boot','rom','--rom',f'{F}/esp32s3_rev0_rom.elf','--flash-mb','8','--psram-mb','2','--board','none','--bootloader',f'{F}/public/hello-bootloader.bin','--ptable',f'{F}/public/hello-ptable.bin','--app',f'{F}/public/hello_world.bin','--max-seconds','30','--no-dump']
cases['c3-hello']=['esp32sim-c3','--boot','rom','--rom',f'{F}/esp32c3_rev3_rom.elf','--flash-mb','4','--bootloader',f'{F}/public/c3-hello-bootloader.bin','--ptable',f'{F}/public/c3-hello-ptable.bin','--app',f'{F}/public/c3-hello_world.bin','--max-seconds','30','--no-dump']
cases['pocket-tank']=['esp32sim','--boot','rom','--rom',f'{F}/esp32s3_rev0_rom.elf','--flash-mb','16','--psram-mb','8','--board','waveshare-amoled18-v2','--bootloader',f'{F}/public/pocket-tank-bootloader.bin','--ptable',f'{F}/public/pocket-tank-ptable.bin','--app',f'{F}/public/pocket-tank.bin','--flash-at',f'0x290000={args.model}','--max-seconds','30','--no-dump']
arms={'main':args.main.rstrip('/')+'/', 'pr':args.pr.rstrip('/')+'/'}
def run(arm,case):
    a=cases[case]; b=resource.getrusage(resource.RUSAGE_CHILDREN)
    p=subprocess.run([arms[arm]+a[0],*a[1:]],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,check=True)
    e=resource.getrusage(resource.RUSAGE_CHILDREN)
    m=re.search(rb'(\d+) insns',p.stderr)
    return e.ru_utime-b.ru_utime, int(m.group(1)) if m else None
for case in cases:
    res={k:[] for k in arms}; ins={k:set() for k in arms}
    for k in arms: run(k,case)
    for r in range(9):
        for k in (list(arms) if r%2==0 else list(arms)[::-1]):
            u,i=run(k,case); res[k].append(u); ins[k].add(i)
    mm,mp=statistics.median(res['main']),statistics.median(res['pr'])
    print(case,'insns',ins,'load',os.getloadavg())
    for k in arms: print(' ',k,' '.join(f'{x:.6f}' for x in res[k]),'median',f'{statistics.median(res[k]):.3f}')
    print(f'  delta {100*(mp-mm)/mm:+.2f}%')
