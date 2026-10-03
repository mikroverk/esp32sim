from pathlib import Path
import subprocess,json
cases=[('unconditional_idle_refresh','esp32c3/src/bus.rs','self.devices(cycles) as u32','self.devices(cycles); 1'),('lost_rmt_irq_change','esp32c3/src/periph.rs','changed |= before != self.rmt.rmt.irq();','let _ = before;'),('lost_uart_only_board','esp32c3/src/bus.rs','self.board_edges || self.uart_pins','self.board_edges')]
results=[]
for name,file,old,new in cases:
 p=Path(file);saved=p.read_text();assert old in saved
 try:
  p.write_text(saved.replace(old,new))
  args=['-p','esp32sim','--test','uart_endpoint'] if name=='lost_uart_only_board' else ['-p','esp32c3','--test','pin_transport','--','--skip','external_']
  r=subprocess.run(['cargo','+1.99.0','test',*args],stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  Path('/tmp/idle-mutation-'+name+'.log').write_text(r.stdout)
  failed=[line for line in r.stdout.splitlines() if line.startswith('test ') and 'FAILED' in line]
  results.append(dict(mutation=name,killed=r.returncode!=0 and bool(failed),failed_tests=failed))
 finally:p.write_text(saved)
assert all(r['killed'] for r in results),results
Path('/tmp/idle-mutations.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results),flush=True)
