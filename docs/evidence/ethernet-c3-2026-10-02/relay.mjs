// Usage: node relay.mjs ROOT CHIP ROM FLASH ELF [STUB]
// Boots caller-supplied firmware and answers DHCP over the exported Ethernet relay.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
const [root, chip, rom, flash, elf, stub] = process.argv.slice(2);
assert(root && ['s3', 'c3', 'c6'].includes(chip) && rom && flash && elf, 'ROOT CHIP ROM FLASH ELF [STUB]');
const { createJitHost } = await import(pathToFileURL(resolve(root, 'web/wasm/jit.mjs')));
let w;
const mem = () => new Uint8Array(w.memory.buffer);
const enc = new TextEncoder(), dec = new TextDecoder();
const logs = [];
const jit = createJitHost(() => w);
const { instance } = await WebAssembly.instantiate(readFileSync(resolve(root, 'web/wasm/esp32sim.wasm')), {
  env: { ...jit.imports, host_profile_now: () => performance.now(), host_log: (p, n) => logs.push(dec.decode(mem().subarray(p, p + n))) },
});
w = instance.exports;
const bytes = (b, call) => {
  const p = w.esp32sim_alloc(b.length);
  mem().set(b, p);
  try { return call(p, b.length); } finally { w.esp32sim_free(p, b.length); }
};
const e = bytes(enc.encode(chip === 's3' ? 'none' : `esp32${chip}`), (p, n) => w.esp32sim_new(p, n, chip === 'c3' ? 4 : 8, 0));
assert(e);
for (const [kind, path] of [[0, rom], [5, flash], [4, elf]]) {
  assert.equal(bytes(readFileSync(path), (p, n) => w.esp32sim_load(e, kind, p, n)), 0);
}
if (stub) assert.equal(bytes(enc.encode(stub), (p, n) => w.esp32sim_stub_spec(e, p, n)), 0);
assert.equal(bytes(enc.encode('ssid=esp32sim'), (p, n) => w.esp32sim_wifi(e, p, n)), 0);
assert.equal(w.esp32sim_ethernet_relay(e, 1), 0);
w.esp32sim_set_jit(e, 1);
assert.equal(w.esp32sim_boot(e, 0), 0);
let discovers = 0, requests = 0, injected = 0, serial = '';
function reply(frame) {
  if (frame.readUInt16BE(12) !== 0x0800 || frame[23] !== 17) return;
  const udp = 14 + (frame[14] & 15) * 4;
  if (frame.readUInt16BE(udp + 2) !== 67) return;
  const d = frame.subarray(udp + 8);
  assert(d.length >= 240 && d[0] === 1);
  let type = 0;
  for (let i = 240; i < d.length && d[i] !== 255;) {
    if (d[i] === 0) { i++; continue; }
    if (d[i] === 53) type = d[i + 2];
    i += 2 + d[i + 1];
  }
  if (type !== 1 && type !== 3) return;
  if (type === 1) discovers++; else requests++;
  const bootp = Buffer.alloc(240);
  bootp.set([2, 1, 6]);
  d.copy(bootp, 4, 4, 12);
  bootp.set([10, 0, 2, 15], 16);
  bootp.set([10, 0, 2, 2], 20);
  d.copy(bootp, 28, 28, 34);
  bootp.set([99, 130, 83, 99], 236);
  const payload = Buffer.concat([bootp, Buffer.from([
    53, 1, type === 1 ? 2 : 5, 54, 4, 10, 0, 2, 2,
    51, 4, 0, 1, 81, 128, 1, 4, 255, 255, 255, 0,
    3, 4, 10, 0, 2, 2, 6, 4, 10, 0, 2, 3, 255,
  ])]);
  const out = Buffer.alloc(14 + 20 + 8 + payload.length);
  out.fill(255, 0, 6);
  out.set([2, 0x53, 0x49, 0x4d, 0, 2], 6);
  out.writeUInt16BE(0x0800, 12);
  out[14] = 0x45; out[22] = 64; out[23] = 17;
  out.writeUInt16BE(out.length - 14, 16);
  out.set([10, 0, 2, 2], 26); out.fill(255, 30, 34);
  let sum = 0;
  for (let i = 14; i < 34; i += 2) sum += out.readUInt16BE(i);
  while (sum > 65535) sum = (sum & 65535) + (sum >>> 16);
  out.writeUInt16BE(~sum & 65535, 24);
  out.writeUInt16BE(67, 34); out.writeUInt16BE(68, 36);
  out.writeUInt16BE(8 + payload.length, 38);
  out.set(payload, 42);
  assert.equal(bytes(out, (p, n) => w.esp32sim_ethernet_receive(e, p, n)), 0);
  injected++;
}
try {
  const slices = Math.ceil(w.esp32sim_cpu_hz(e) * 25 / 160_000);
  for (let slice = 0; slice < slices && !serial.includes('status=3 ip=10.0.2.15'); slice++) {
    const rc = w.esp32sim_run(e, 160_000, 0);
    assert.equal(rc, 0, logs.slice(-4).join('\n'));
    assert(!logs.some(line => line.includes('chip reset')), 'unexpected chip reset: ' + logs.slice(-4).join('\n'));
    const n = w.esp32sim_ethernet_take(e);
    for (let i = 0; i < n; i++) {
      const p = w.esp32sim_ethernet_ptr(e, i), len = w.esp32sim_ethernet_len(e, i);
      assert(len >= 14 && len <= 1518);
      reply(Buffer.from(mem().subarray(p, p + len)));
    }
    assert.equal(w.esp32sim_ethernet_take(e), 0, 'drain empties queue');
    const count = w.esp32sim_out_take(e);
    for (let i = 0; i < count; i++) {
      if (w.esp32sim_out_kind(e, i) !== 1) continue;
      const p = w.esp32sim_out_ptr(e, i), n = w.esp32sim_out_len(e, i);
      const msg = JSON.parse(dec.decode(mem().subarray(p, p + n)));
      if (msg.t === 'serial') serial += msg.data;
    }
  }
  console.log(serial);
  assert(discovers > 0 && requests > 0 && injected >= 2, `DHCP discover=${discovers} request=${requests} injected=${injected}`);
  assert(serial.includes('status=3 ip=10.0.2.15'), logs.slice(-4).join('\n'));
  console.log(JSON.stringify({ chip, discovers, requests, injected, cycles: w.esp32sim_cycles(e), insns: w.esp32sim_insns(e), pass: true }));
} finally { w.esp32sim_delete(e); }
