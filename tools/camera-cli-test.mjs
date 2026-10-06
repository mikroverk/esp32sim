import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync } from 'node:fs';
import assert from 'node:assert/strict';
const scratch = mkdtempSync('.camera-cli-test-');
try {
  const fifo = scratch + '/input';
  assert.equal(spawnSync('mkfifo', [fifo]).status, 0);
  const fw = 'web/wasm/fw/';
  const args = ['--board', 'waveshare-cam', '--boot', 'rom', '--flash-mb', '8', '--psram-mb', '2',
    '--rom', fw + 'esp32s3_rev0_rom.elf', '--bootloader', fw + 'public/hello-bootloader.bin',
    '--ptable', fw + 'public/hello-ptable.bin', '--app', fw + 'public/hello_world.bin',
    '--cam-stream', fifo, '--cam-size', '2x2', '--max-seconds', '0.2', '--no-dump'];
  const child = spawn('target/release/esp32sim', args, { stdio: ['ignore', 'pipe', 'pipe'] });
  let out = '', err = '';
  child.stdout.on('data', d => out += d); child.stderr.on('data', d => err += d);
  const timer = setTimeout(() => child.kill('SIGKILL'), 10000);
  const code = await new Promise((resolve, reject) => { child.on('error', reject); child.on('close', resolve); });
  clearTimeout(timer);
  assert.equal(code, 0, err);
  assert.match(out, /ESP-ROM/);
  assert.match(err, /emulated 0\.200s/);
  assert.doesNotMatch(err, /board UI:/);
  console.log('PASS: FIFO without writer boots and exits at max-seconds; no web server');
} finally { rmSync(scratch, { recursive: true, force: true }); }
