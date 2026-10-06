import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import assert from 'node:assert/strict';
const scratch = mkdtempSync('.camera-cli-test-');
try {
  for (const [flags, message] of [
    [['--cam-stream', '-'], /must be supplied together/],
    [['--cam-size', '2x2'], /must be supplied together/],
    [['--cam-stream', '-', '--cam-size', '2x2'], /requires --board waveshare-cam/],
    [['--board', 'waveshare-cam', '--cam-stream', '-', '--cam-size', '+2x+2'], /nonzero decimal WIDTHxHEIGHT/],
    [['--board', 'waveshare-cam', '--cam-stream', '-', '--cam-size', '4096x4096'], /8 MiB/],
    [['--chip', 'c3', '--cam-stream', '-'], /--cam-stream is not available on the C3/],
    [['--chip', 'c6', '--cam-size', '2x2'], /--cam-size is not available on the C6/],
  ]) {
    const result = spawnSync('target/release/esp32sim', flags, { encoding: 'utf8', timeout: 10000 });
    assert.equal(result.status, 2, result.stderr);
    assert.match(result.stderr, message);
  }
  const image = scratch + '/oversize.ppm';
  writeFileSync(image, Buffer.concat([Buffer.from('P6\n4096 4096\n255\n'), Buffer.alloc(4096 * 4096 * 3)]));
  const rejected = spawnSync('target/release/esp32sim', ['--board', 'waveshare-cam', '--cam-image', image], { encoding: 'utf8', timeout: 10000 });
  assert.equal(rejected.status, 2, rejected.stderr);
  assert.match(rejected.stderr, /invalid camera picture or frame exceeds 8 MiB/);
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
  console.log('PASS: per-chip camera flags and oversize still image rejected; FIFO without writer boots and exits at max-seconds; no web server');
} finally { rmSync(scratch, { recursive: true, force: true }); }
