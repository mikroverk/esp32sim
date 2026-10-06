// Run from the repository root after building release binaries and fetching demo ROMs.
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';
const port = process.env.CAMERA_TEST_PORT;
assert(port, 'Set CAMERA_TEST_PORT to a free localhost port');
const fw = 'web/wasm/fw/';
const child = spawn('target/release/esp32sim', [
  '--board', 'waveshare-cam', '--boot', 'rom', '--flash-mb', '8', '--psram-mb', '2',
  '--rom', fw + 'esp32s3_rev0_rom.elf', '--bootloader', fw + 'public/hello-bootloader.bin',
  '--ptable', fw + 'public/hello-ptable.bin', '--app', fw + 'public/hello_world.bin',
  '--cam-stream', '-', '--cam-size', '1x1', '--console', 'usb', '--web', port,
  '--max-seconds', '3', '--no-dump',
], { stdio: ['pipe', 'ignore', 'pipe'] });
let error = '', socket;
const exit = new Promise((resolve, reject) => { child.on('error', reject); child.on('close', resolve); });
try {
  const preview = new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('No camera preview from CLI stream')), 15000);
    child.stderr.on('data', data => {
      error += data.toString();
      if (socket || !error.includes('board UI:')) return;
      socket = new WebSocket(`ws://127.0.0.1:${port}`);
      socket.binaryType = 'arraybuffer';
      socket.onerror = () => { clearTimeout(timer); reject(new Error('WebSocket failed')); };
      socket.onmessage = ({ data }) => {
        if (!(data instanceof ArrayBuffer)) return;
        const bytes = new Uint8Array(data);
        if (bytes[0] !== 4) return;
        clearTimeout(timer);
        resolve(bytes);
      };
    });
  });
  // Last complete frame wins; a truncated third frame must not replace it.
  child.stdin.end(Buffer.from([255, 0, 0, 0, 0, 255, 1]));
  const frame = await preview;
  assert.equal(frame.length, 5 + 320 * 240 * 3);
  for (let i = 5; i < frame.length; i += 3) assert.deepEqual([...frame.subarray(i, i + 3)], [0, 0, 255]);
  assert.equal(await exit, 0);
  assert.match(error, /camera stream:/);
  console.log(JSON.stringify({stdinRgb24: true, latestCompleteFrame: 'blue', truncatedFrameRejected: true, consoleOptionAccepted: true, preview: [320, 240]}));
} finally { socket?.close(); child.kill(); }
