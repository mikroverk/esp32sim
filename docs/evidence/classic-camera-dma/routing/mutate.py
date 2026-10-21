#!/usr/bin/env python3
"""Run serially in a disposable worktree. Every kill must be an assertion failure."""
import json, subprocess, sys
from pathlib import Path
root, out = map(lambda s: Path(s).resolve(), sys.argv[1:3])
out.mkdir(parents=True, exist_ok=True)
b = 'esp32/src/board/aithinker.rs'
r = 'esp32/src/bus.rs'
p = 'esp32/src/periph.rs'
timing = 'c6_vsync_unarmed_stopped_irq_and_one_us_level_without_dma'
power = 'c6_power_clock_and_sensor_stop_cancel_and_restart_vsync'
routes = 'c6_physical_routes_inversion_and_reconfiguration'
receiver = 'c6_receiver_unarmed_reset_and_modes_withhold_routed_frames'
rows = [
 ('vsync-pin', b, 'pub const PIN_VSYNC: u8 = 25', 'pub const PIN_VSYNC: u8 = 24', timing),
 ('pulse-shorter-than-isr', b, 'crate::periph::CPU_HZ / 1000', 'crate::periph::CPU_HZ / 1_000_000', timing),
 ('missing-deadline', b, 'fn next_deadline(&self) -> Option<u64> { self.next }', 'fn next_deadline(&self) -> Option<u64> { None }', timing),
 ('vsync-stuck-low', b, 'self.vsync = !self.vsync;', 'self.vsync = false;', timing),
 ('vsync-stuck-high', b, 'self.vsync = !self.vsync;', 'self.vsync = true;', timing),
 ('first-edge-too-early', b, 'cycle.checked_add(FRAME_CYCLES)', 'cycle.checked_add(FRAME_CYCLES - 1)', timing),
 ('capture-start-gates-vsync', r, 'let periph = &self.periph;\n        self.board.advance_inputs_with_clock', 'if !self.periph.i2s[0].camera_rx().is_some_and(|r| r.started) { return; }\n        let periph = &self.periph;\n        self.board.advance_inputs_with_clock', timing),
 ('no-gpio-delivery', r, 'self.periph.gpio.set_input(edge.pin, edge.level);', 'self.periph.gpio.set_input(edge.pin, false);', timing),
 ('no-board-deadline-in-scheduler', 'esp32/src/soc.rs', 'self.board.next_deadline().map', 'None::<u64>.map', timing),
 ('dma-write-before-transport', r, 'self.i2s_step(elapsed);', 'self.i2s_step(elapsed); if self.board.name() == "ai-thinker-cam" { self.dram[0] = 1; }', timing),
 ('vsync-power-ignored', b, '!*self.powered_down.lock().unwrap() && streaming && live', 'streaming && live', power),
 ('vsync-xclk-ignored', b, '&& streaming && live', '&& streaming && (live || true)', power),
 ('vsync-stream-ignored', b, '&& streaming && live', '&& (streaming || true) && live', power),
 ('stop-retains-high', b, 'if self.vsync {\n                self.vsync = false;', 'if false {\n                self.vsync = false;', power),
 ('stop-retains-deadline', b, 'self.next = None;', 'self.next = self.next;', power),
 ('edge-timestamps-lost', b, 'BoardEdge { cycle: at, pin: PIN_VSYNC', 'BoardEdge { cycle, pin: PIN_VSYNC', 'c6_deadline_edges_preserve_timestamps_and_large_steps'),
 ('skip-intermediate-edges', b, 'while let Some(at) = self.next.filter', 'if let Some(at) = self.next.filter', 'c6_deadline_edges_preserve_timestamps_and_large_steps'),
 ('data-wire-unqualified', r, 'if actual != *pin { return None; }', 'if actual != *pin && false { return None; }', routes),
 ('clock-wire-unqualified', r, '[(28, pins[8]), (190, pins[9]), (191, pins[10])]', '[(190, pins[9]), (191, pins[10])]', routes),
 ('href-wire-unqualified', r, '[(28, pins[8]), (190, pins[9]), (191, pins[10])]', '[(28, pins[8]), (191, pins[10])]', routes),
 ('vsync-wire-unqualified', r, '[(28, pins[8]), (190, pins[9]), (191, pins[10])]', '[(28, pins[8]), (190, pins[9])]', routes),
 ('control-polarity-ignored', r, 'gpio.matrix_input_route(signal) != Some((pin, false))', 'gpio.matrix_input_route(signal).map(|(p, _)| p) != Some(pin)', routes),
 ('ignore-data-inversion', r, '*byte ^= invert;', '*byte ^= invert & 0;', routes),
 ('ignore-enable', r, 'gpio.signal_input(192) != Some(true)', 'gpio.signal_input(192).is_none()', routes),
 ('pad-input-disable-ignored', p, '&& self.input_enabled(pin))\n            .then_some', ')\n            .then_some', routes),
 ('pad-function-ignored', p, '&& self.matrix_pad(pin) && self.input_enabled(pin))', '&& self.input_enabled(pin))', routes),
 ('rx-start-ignored', r, '!rx.started || ', '', receiver),
 ('rx-slave-ignored', r, '!rx.slave || ', '', receiver),
 ('rx-descriptor-ignored', r, '!rx.descriptor_enabled || ', '', receiver),
 ('rx-force-ignored', r, '!rx.fifo_mode_forced || ', '', receiver),
 ('rx-fifo-mode-ignored', r, ' || ![1, 3].contains(&rx.fifo_mode)', '', receiver),
 ('rx-clock-gate-ignored', r, 'if !self.periph.i2s[0].clock_enabled { return None; }', '', receiver),
 ('camera-free-input-poll', r, 'if self.pins_active {\n            self.deliver_board_inputs();\n            for input', 'if true {\n            self.deliver_board_inputs();\n            for input', 'c6_camera_free_has_no_producer_deadlines_or_steps'),
 ('camera-free-deadline-poll', 'esp32/src/soc.rs', 'if !self.pins_active { return timer; }', '', 'c6_camera_free_has_no_producer_deadlines_or_steps'),
 ('non-camera-clock-poll', 'esp-soc/src/board.rs', '_clock: &dyn Fn(u8) -> Option<(f64, u32)>) { self.advance_to(cycle); }', '_clock: &dyn Fn(u8) -> Option<(f64, u32)>) { _clock(0); self.advance_to(cycle); }', 'c6_active_non_camera_board_never_queries_camera_clock'),
]
selected = set(sys.argv[3:])
if selected:
 rows = [row for row in rows if row[0] in selected]
 assert len(rows) == len(selected), 'unknown mutation name'
results = json.loads((out/'mutations.json').read_text()) if selected and (out/'mutations.json').exists() else []
def run(test):
 cmd = ['nice','-n','10','cargo','+1.99.0','test','--release','-p','esp32','--test','aithinker',test,'--','--exact']
 return cmd, subprocess.run(cmd, cwd=root, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
# The whole integration test is the baseline; every mutant selects one exact test.
cmd = ['nice','-n','10','cargo','+1.99.0','test','--release','-p','esp32','--test','aithinker']
baseline = subprocess.run(cmd, cwd=root, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
(out/'baseline.log').write_text(baseline.stdout)
assert baseline.returncode == 0, baseline.stdout
for name, file, old, new, test in rows:
 path = root/file; original = path.read_text()
 assert original.count(old) == 1, (name, original.count(old))
 try:
  path.write_text(original.replace(old,new))
  cmd, result = run(test)
  (out/(name+'.log')).write_text(result.stdout)
  killed = result.returncode != 0 and 'test result: FAILED. 0 passed; 1 failed;' in result.stdout and 'panicked at' in result.stdout
  results = [row for row in results if row['mutation'] != name]
  results.append(dict(mutation=name,file=file,old=old,new=new,test=test,killed=killed,returncode=result.returncode,command=cmd))
  (out/'mutations.json').write_text(json.dumps(results,indent=2)+'\n')
  print(name, 'KILLED' if killed else 'NOT KILLED', flush=True)
 finally:
  path.write_text(original)
assert all(row['killed'] for row in results)
