// node wasm/tests/ble-connection.mjs MODULE FW_DIR ROM_ELF > result.json
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
import { createJitHost } from '../../web/wasm/jit.mjs';
const [modulePath, build, rom] = process.argv.slice(2);
assert(modulePath && build && rom, 'needs MODULE, public firmware directory and C3 rev3 ROM_ELF');
const moduleBytes = readFileSync(modulePath);
const logs = [];
let w;
const decoder = new TextDecoder();
const mem = () => new Uint8Array(w.memory.buffer);
const jit = createJitHost(() => w);
const { instance } = await WebAssembly.instantiate(moduleBytes, { env: {
  ...jit.imports, host_profile_now: () => performance.now(),
  host_log: (p, n) => logs.push(decoder.decode(mem().subarray(p, p + n))),
} });
w = instance.exports;
const bytes = (data, f) => {
  const p = w.esp32sim_alloc(data.length);
  mem().set(data, p);
  try { return f(p, data.length); } finally { w.esp32sim_free(p, data.length); }
};
for (const chip of ['none', 'esp32c6']) {
  const other = bytes(new TextEncoder().encode(chip), (p, n) => w.esp32sim_new(p, n, 4, 0));
  assert(other);
  assert.equal(w.esp32sim_ble_scan(other, 1), 1);
  assert.equal(bytes(new TextEncoder().encode('central-stop'), (p, n) => w.esp32sim_ble_command(other, p, n)), 1);
  assert.equal(bytes(new TextEncoder().encode('connect'), (p, n) => w.esp32sim_ble_command(other, p, n)), 1);
  w.esp32sim_delete(other);
}
const emu = bytes(new TextEncoder().encode('c3'), (p, n) => w.esp32sim_new(p, n, 4, 0));
assert(emu);
assert.equal(w.esp32sim_ble_scan(emu, 1), 1);
assert.equal(bytes(new TextEncoder().encode('central-stop'), (p, n) => w.esp32sim_ble_command(emu, p, n)), 1);
assert.equal(bytes(new TextEncoder().encode('connect'), (p, n) => w.esp32sim_ble_command(emu, p, n)), 1);

assert.equal(w.esp32sim_ble_full(emu), 0);
for (const text of ['disconnect', 'central-stop', 'discover', 'read-uuid bad bad']) {
  assert.equal(bytes(new TextEncoder().encode(text), (p, n) => w.esp32sim_ble_command(emu, p, n)), 1);
}

for (const [kind, path] of [[0, rom], [1, join(build, 'c3-ble-bootloader.bin')],
  [2, join(build, 'c3-ble-ptable.bin')], [3, join(build, 'c3-ble-server.bin')]]) {
  assert.equal(bytes(readFileSync(path), (p, n) => w.esp32sim_load(emu, kind, p, n)), 0);
}
assert.equal(w.esp32sim_boot(emu, 0), 0);
assert.equal(w.esp32sim_ble_full(emu), 1);
assert.equal(w.esp32sim_ble_scan(emu, 1), 0);
const command = text => assert.equal(bytes(new TextEncoder().encode(text), (p, n) => w.esp32sim_ble_command(emu, p, n)), 0);
const read = 'read-uuid 4fafc201-1fb5-459e-8fcc-c5c9c331914b beb5483e-36e1-4688-b7f5-ea07361b26a8';
const schedule = [[0.5, () => command('connect')], [0.7, () => command(read)],
  [3.1, () => command('central-stop')],
  [5.7, () => command('connect')], [5.9, () => command(read)],
  [7.7, () => command('disconnect')]];
const observations = [];
let consoleText = "";
while (w.esp32sim_cycles(emu) < 8 * 160_000_000) {
  if (schedule.length && w.esp32sim_cycles(emu) >= schedule[0][0] * 160_000_000) schedule.shift()[1]();
  assert.equal(w.esp32sim_run(emu, 2_000_000, 0), 0);
  const count = w.esp32sim_out_take(emu);
  for (let i = 0; i < count; i++) {
    if (w.esp32sim_out_kind(emu, i) !== 1) continue;
    const ptr = w.esp32sim_out_ptr(emu, i), len = w.esp32sim_out_len(emu, i);
    const message = JSON.parse(decoder.decode(mem().subarray(ptr, ptr + len)));
    if (message.t === 'serial') consoleText += message.data;
  }
  for (;;) {
    const len = w.esp32sim_ble_take(emu);
    if (!len) break;
    const ptr = w.esp32sim_ble_ptr(emu);
    observations.push(decoder.decode(mem().subarray(ptr, ptr + len)));
  }
}
assert(!observations.some(s => /\[ble-error\]|dropped=/.test(s)));
assert.equal(observations.filter(s => s.startsWith('[ble-state] connected')).length, 2);
assert.equal(observations.filter(s => s.includes('[ble-att] value=48656c6c6f20576f726c642073617973204e65696c')).length, 2);
const resumed = observations.findIndex(s => s.includes('disconnected advertising_resumed'));
assert(resumed > 0);
assert.equal(observations.filter(s => s.includes('disconnected advertising_resumed')).length, 2);
const packets = {};
for (const kind of ['ADV_IND', 'SCAN_RSP']) {
  const match = s => s.includes(`type=${kind}`);
  const before = observations.slice(0, resumed).filter(match);
  const after = observations.slice(resumed).filter(match);
  assert(before.length && after.length);
  assert.equal(new Set([...before, ...after].map(s => s.split('pdu=')[1])).size, 1);
  packets[kind] = {before: before.length, after: after.length, identical: true};
}
assert(observations.some(s => s.includes('name="BLE Server Example"')));
assert(!logs.some(s => /assert|panic|exception/i.test(s)));
assert(!/assert|panic|^E \(/m.test(consoleText));
assert(consoleText.includes('server: disconnected reason=520')); // NimBLE HCI base 0x200 + 0x08.
assert(consoleText.includes('server: disconnected reason=531')); // HCI base + remote-user reason 0x13.
w.esp32sim_delete(emu);
console.log(JSON.stringify({checks: 'pass', module_sha256: createHash('sha256').update(moduleBytes).digest('hex'),
  modeled_seconds: 8, connections: 2, reads: 2, timeout_reason: 8, normal_disconnect_reason: 19, packets, guest_errors: []}, null, 2));
