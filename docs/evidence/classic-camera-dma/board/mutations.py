#!/usr/bin/env python3
"""Rule-removal mutations: run from a disposable worktree, never in parallel with Cargo."""
import json, os, subprocess, sys
from pathlib import Path
root = Path(sys.argv[1]).resolve()
out = Path(sys.argv[2]).resolve(); out.mkdir(parents=True, exist_ok=True)
b = 'esp32/src/board/aithinker.rs'
c = 'esp-soc/src/camera.rs'
rows = [
 ('board-selection', 'esp32/src/board/mod.rs', '"ai-thinker-cam" | "esp32cam" =>', '"disabled-camera" =>', 'selection_and_only_sensor_attachment'),
 ('no-reset-sentinel', b, 'PIN_RESET: Option<u8> = None', 'PIN_RESET: Option<u8> = Some(0)', 'selection_and_only_sensor_attachment'),
 ('sccb-sda', b, 'Some((26, 27))', 'Some((27, 27))', 'sccb_requires_both_physical_wires'),
 ('sccb-scl', b, 'Some((26, 27))', 'Some((26, 26))', 'sccb_requires_both_physical_wires'),
 ('sccb-unqualified', b, 'Some((26, 27))', 'None', 'sccb_requires_both_physical_wires'),
 ('sccb-power', b, '!*self.powered_down.lock().unwrap() && self.sensor.start(read)', 'self.sensor.start(read)', 'pwdn_stop_and_reset_withhold_frames'),
 ('frame-power', b, '*self.powered_down.lock().unwrap() || ', '', 'pwdn_stop_and_reset_withhold_frames'),
 ('pwdn-release', b, 'enabled & (1 << PIN_PWDN) == 0 || ', '', 'pwdn_stop_and_reset_withhold_frames'),
 ('stream-stop', b, '!config.streaming || ', '', 'pwdn_stop_and_reset_withhold_frames'),
 ('xclk-route', b, 'clock(PIN_XCLK)?', 'clock(PIN_XCLK).unwrap_or((10_000_000.0, 32768))', 'xclk_requires_live_routed_pwm_not_gpio_level'),
 ('xclk-pin', b, 'clock(PIN_XCLK)?', 'clock(1)?', 'xclk_requires_live_routed_pwm_not_gpio_level'),
 ('xclk-finite', b, '!hz.is_finite() || ', '', 'xclk_rejects_constant_or_invalid_clock'),
 ('xclk-positive', b, 'hz <= 0.0 || ', '', 'xclk_rejects_constant_or_invalid_clock'),
 ('xclk-duty-zero', b, 'duty == 0 || ', '', 'xclk_rejects_constant_or_invalid_clock'),
 ('xclk-duty-full', b, ' || duty >= 65535', '', 'xclk_rejects_constant_or_invalid_clock'),
 ('grayscale-wire', b, '0 => PixelFormat::Yuyv', '0 => PixelFormat::Grayscale', 'grayscale_requests_keep_yuv_wire_bytes'),
 ('rgb565-wire', b, '8 => PixelFormat::Rgb565', '8 => PixelFormat::Yuyv', 'host_and_register_updates_invalidate_shared_cache'),
 ('unsupported-jpeg', b, 'return None;\n            }\n        };', 'PixelFormat::Rgb565\n            }\n        };', 'host_and_register_updates_invalidate_shared_cache'),
 ('picture-validation', c, 'if !p.valid() { return Err', 'if false { return Err', 'host_and_register_updates_invalidate_shared_cache'),
 ('picture-invalidation', c, 'self.picture = Some(p); self.invalidate(); Ok(())', 'self.picture = Some(p); Ok(())', 'host_and_register_updates_invalidate_shared_cache'),
 ('register-invalidation', c, 'self.cached.as_ref().is_none_or(|(old, _)| *old != config)', 'self.cached.is_none()', 'host_and_register_updates_invalidate_shared_cache'),
]
selected = set(sys.argv[3:])
if selected:
 rows = [row for row in rows if row[0] in selected]
 assert len(rows) == len(selected), 'unknown mutation name'
results = []
for name, file, old, new, test in rows:
 p = root / file; original = p.read_text()
 assert original.count(old) == 1, (name, original.count(old))
 try:
  p.write_text(original.replace(old, new))
  cmd = ['nice', '-n', '10', 'cargo', '+1.99.0', 'test', '--release', '-p', 'esp32', '--test', 'aithinker', 'aithinker_' + test, '--', '--exact']
  run = subprocess.run(cmd, cwd=root, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
  log = run.stdout
  killed = run.returncode != 0 and 'test result: FAILED. 0 passed; 1 failed;' in log and 'panicked at' in log
  (out / (name + '.log')).write_text(log)
  results.append(dict(mutation=name, file=file, old=old, new=new, test='aithinker_' + test, killed=killed, returncode=run.returncode, command=cmd))
  print(name, 'KILLED' if killed else 'NOT KILLED', flush=True)
 finally:
  p.write_text(original)
(out / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
assert all(row['killed'] for row in results)
