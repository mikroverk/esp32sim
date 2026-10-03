from pathlib import Path
import subprocess,json
cases=[
 ('FUN_IE','esp-soc/src/pins.rs','&& self.mux(pin) & (1 << 9) != 0','',None),
 ('GPIO_ENABLE','esp-soc/src/uart.rs','gpio.enable & (1u64 << pin) != 0 && ','',None),
 ('baud_30_percent','esp-soc/src/uart.rs','baud as u64 * 3)','baud as u64 * 30)',None),
 ('C6_TX_mask','esp-soc/src/pins.rs','valid: (1 << 31) - 1, input_select: 0x80, output_mask: 0x1ff','valid: (1 << 31) - 1, input_select: 0x80, output_mask: 0x3ff',None),
 ('GPIO46','esp-soc/src/pins.rs','& !(15 << 22),','& !(15 << 22) & !(1 << 46),',None),
 ('S3_RC_clock','esp32s3/src/periph.rs','baud(clock, 20_000_000)','baud(clock, 8_000_000)',None),
 ('C3_RC_clock','esp32c3/src/periph.rs','baud(clock, 20_000_000)','baud(clock, 8_000_000)',None),
 ('C3_reset_selector','esp32c3/src/periph.rs','g.func_out_sel.fill(128);','g.func_out_sel.fill(256);',['-p','esp32c3','--test','pin_transport','--','--skip','external_']),
]
cases.extend([
 ('UART_cached_capability','esp32c3/src/bus.rs','if self.uart_pins {','if self.board.uses_uart_pins() {',None),
 ('C3_idle_board_gate','esp32c3/src/bus.rs','self.board_edges && ','',['-p','esp32c3','--test','pin_transport','idle_board']),
 ('RMT_running_mask_clear','esp-periph/src/rmt.rs','if !c.running { self.running &= !(1 << n); }','',['-p','esp-periph','running_mask_tracks_stop']),
])
results=[]
for name,file,old,new,args in cases:
 p=Path(file);original=p.read_text();assert old in original
 try:
  p.write_text(original.replace(old,new));cmd=['cargo','+1.99.0','test']+(args or ['-p','esp32sim','--test','uart_endpoint'])
  r=subprocess.run(cmd,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
  Path('/tmp/mutation-'+name+'.log').write_text(r.stdout)
  failed=[line for line in r.stdout.splitlines() if line.startswith('test ') and 'FAILED' in line]
  results.append(dict(mutation=name,killed=r.returncode!=0 and bool(failed),failed_tests=failed))
 finally:p.write_text(original)
Path('/tmp/pr171-mutations.json').write_text(json.dumps(results,indent=2)+'\n')
print(json.dumps(results,indent=2))
