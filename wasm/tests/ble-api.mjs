// node wasm/tests/ble-api.mjs MODULE FW_DIR ROM_ELF > result.json
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
const emu = bytes(new TextEncoder().encode('c3'), (p, n) => w.esp32sim_new(p, n, 4, 0));
assert(emu);
assert.equal(w.esp32sim_ble_full(emu), 0);
for (const [kind, path] of [[0, rom], [1, join(build, 'c3-ble-bootloader.bin')],
  [2, join(build, 'c3-ble-ptable.bin')], [3, join(build, 'c3-ble-advertiser.bin')]]) {
  assert.equal(bytes(readFileSync(path), (p, n) => w.esp32sim_load(emu, kind, p, n)), 0);
}
assert.equal(w.esp32sim_boot(emu, 0), 0);
assert.equal(w.esp32sim_ble_full(emu), 1);
const observations = [];
while (w.esp32sim_cycles(emu) < 320_000_000) {
  assert.equal(w.esp32sim_run(emu, 2_000_000, 0), 0);
  for (;;) {
    const len = w.esp32sim_ble_take(emu);
    if (!len) break;
    const ptr = w.esp32sim_ble_ptr(emu);
    observations.push(decoder.decode(mem().subarray(ptr, ptr + len)));
  }
}
const air = observations.filter(s => s.startsWith('[ble-air]'));
assert(air.length === 57);
assert(air.every(s => s.includes('name="esp32sim"')));
assert(!observations.some(s => s.startsWith('[ble-error]')));
for (let i = 0; i < air.length; i++) {
  assert(air[i].includes(`channel=${37 + i % 3} type=ADV_SCAN_IND AdvA=3c:84:27:b6:a7:1e`));
  assert(air[i].includes('service=180f'));
}
const starts = air.filter((_, i) => i % 3 === 0).map(s => Number(/hus=(\d+)/.exec(s)[1]));
const intervals = starts.slice(1).map((t, i) => (t - starts[i]) / 2);
assert(intervals.every(t => t >= 100000 && t <= 110000));
assert(!logs.some(s => /panic|exception/i.test(s)));
w.esp32sim_delete(emu);
console.log(JSON.stringify({ checks: 'pass', module_sha256: createHash('sha256').update(moduleBytes).digest('hex'),
  modeled_seconds: 2, packets: air.length, events: starts.length,
  first_five_event_hus: starts.slice(0, 5), interval_us: { min: Math.min(...intervals), max: Math.max(...intervals) },
  sample: air[0], configured: observations.find(s => s.startsWith('[ble-config]')) }, null, 2));
