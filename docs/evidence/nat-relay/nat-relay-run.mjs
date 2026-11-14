// The receipt's page-side run: a station firmware in the WASM build, its NAT carried through
// `esp32sim_nat_relay`, the records served by Node's own sockets — a stand-in for a page relaying
// them over a WebSocket. Prints UART0, then the relay's counts.
//   node nat-relay-run.mjs MODULE ROM BOOTLOADER PTABLE APP SECONDS
import { readFileSync } from 'node:fs';
import net from 'node:net';
import dgram from 'node:dgram';
import dns from 'node:dns/promises';
import { createJitHost } from '../../../web/wasm/jit.mjs';

const [modulePath, rom, bootloader, ptable, app, seconds] = process.argv.slice(2);
const logs = [];
let w;
const dec = new TextDecoder(), enc = new TextEncoder();
const mem = () => new Uint8Array(w.memory.buffer);
const jit = createJitHost(() => w);
const { instance } = await WebAssembly.instantiate(readFileSync(modulePath), { env: {
  ...jit.imports, host_profile_now: () => performance.now(),
  host_log: (p, n) => logs.push(dec.decode(mem().subarray(p, p + n))),
} });
w = instance.exports;
const bytes = (data, f) => { const p = w.esp32sim_alloc(Math.max(data.length, 1)); mem().set(data, p); try { return f(p, data.length); } finally { w.esp32sim_free(p, Math.max(data.length, 1)); } };

const emu = bytes(enc.encode('none'), (p, n) => w.esp32sim_new(p, n, 4, 0));
for (const [kind, path] of [[0, rom], [1, bootloader], [2, ptable], [3, app]]) {
  if (bytes(readFileSync(path), (p, n) => w.esp32sim_load(emu, kind, p, n)) !== 0) throw new Error(`load ${path}: ${logs.join(' | ')}`);
}
if (bytes(enc.encode('ssid=esp32sim'), (p, n) => w.esp32sim_wifi(emu, p, n)) !== 0) throw new Error(logs.join(' | '));
if (w.esp32sim_nat_relay(emu, 1 | 2) !== 0) throw new Error(logs.join(' | '));
if (w.esp32sim_boot(emu, 0) !== 0) throw new Error(`boot: ${logs.join(' | ')}`);

const EV = { CONNECTED: 1, FAILED: 2, TCP_DATA: 3, TCP_EOF: 4, TCP_RESET: 5, TX_CREDIT: 6, UDP_DATA: 7, RESOLVED: 8, UNRESOLVED: 9 };
const event = (kind, id, arg, data = new Uint8Array()) => bytes(data, (p, n) => w.esp32sim_nat_event(emu, kind, id, arg, p, n));
const ip = b => `${b[0]}.${b[1]}.${b[2]}.${b[3]}`;
const tcp = new Map(), udp = new Map(), pending = [];
const counts = { tcp_connect: 0, tcp_connected: 0, tcp_failed: 0, bytes_out: 0, bytes_in: 0, udp_open: 0, resolve: 0, resolved: 0 };
const WINDOW = 65536;

// Bytes the guest's side did not take yet are offered again after the next run.
const offer = id => {
  const f = tcp.get(id);
  while (f && f.queue.length) {
    const chunk = f.queue[0];
    const taken = event(EV.TCP_DATA, id, 0, chunk);
    counts.bytes_in += taken;
    if (taken < chunk.length) { f.queue[0] = chunk.subarray(taken); return; }
    f.queue.shift();
  }
  if (f && f.eof && !f.queue.length) { event(EV.TCP_EOF, id, 0); f.eof = false; }
};

const records = () => {
  const n = w.esp32sim_nat_take(emu);
  if (!n) return;
  const buf = mem().slice(w.esp32sim_nat_ptr(emu), w.esp32sim_nat_ptr(emu) + n);
  const view = new DataView(buf.buffer);
  for (let at = 0; at < n;) {
    const op = buf[at], flags = buf[at + 1], port = view.getUint16(at + 2, true), id = view.getUint32(at + 4, true), len = view.getUint32(at + 8, true);
    const data = buf.slice(at + 12, at + 12 + len);
    at += Math.ceil((12 + len) / 4) * 4;
    if (op === 1) {
      counts.tcp_connect++;
      const f = { queue: [], eof: false, sock: net.connect(port, ip(data)) };
      tcp.set(id, f);
      f.sock.on('connect', () => { counts.tcp_connected++; pending.push(() => event(EV.CONNECTED, id, WINDOW)); });
      f.sock.on('data', d => { f.queue.push(new Uint8Array(d)); pending.push(() => offer(id)); });
      f.sock.on('end', () => { f.eof = true; pending.push(() => offer(id)); });
      f.sock.on('error', e => { counts.tcp_failed += f.sock.connecting ? 1 : 0; pending.push(() => event(f.sock.connecting ? EV.FAILED : EV.TCP_RESET, id, e.code === 'ECONNREFUSED' ? 1 : 2)); });
    } else if (op === 2) {
      const f = tcp.get(id);
      counts.bytes_out += len;
      if (f) f.sock.write(data, () => pending.push(() => event(EV.TX_CREDIT, id, len)));
    } else if (op === 3) tcp.get(id)?.sock.end();
    else if (op === 4) { const f = tcp.get(id); if (f) { flags & 1 ? f.sock.resetAndDestroy?.() ?? f.sock.destroy() : f.sock.end(); tcp.delete(id); } }
    else if (op === 5) {
      counts.udp_open++;
      const s = dgram.createSocket('udp4');
      s.connect(port, ip(data));
      s.on('message', m => { counts.bytes_in += m.length; pending.push(() => event(EV.UDP_DATA, id, 0, new Uint8Array(m))); });
      s.on('error', () => {});
      udp.set(id, { s, ready: new Promise(r => s.once('connect', r)) });
    } else if (op === 6) { const u = udp.get(id); counts.bytes_out += len; u?.ready.then(() => u.s.send(data)); }
    else if (op === 7) { udp.get(id)?.s.close(); udp.delete(id); }
    else if (op === 8) {
      counts.resolve++;
      const name = dec.decode(data);
      (port === 28 ? dns.resolve6(name) : dns.resolve4(name, { ttl: true })).then(
        a => { counts.resolved++; const out = port === 28 ? [] : a.map(r => r.address.split('.').map(Number)).flat(); pending.push(() => event(EV.RESOLVED, id, port === 28 ? 60 : (a[0]?.ttl ?? 60), new Uint8Array(out))); },
        e => pending.push(() => event(EV.UNRESOLVED, id, e.code === 'ENOTFOUND' || e.code === 'ENODATA' ? 3 : 2)));
    }
  }
};

const hz = Number(w.esp32sim_cpu_hz(emu));
const end = Number(seconds) * hz;
let uart = '';
while (w.esp32sim_cycles(emu) < end) {
  if (w.esp32sim_run(emu, Math.round(hz / 1000), Date.now()) !== 0) throw new Error(`run: ${logs.slice(-3).join(' | ')}`);
  for (let i = 0, n = w.esp32sim_out_take(emu); i < n; i++) {
    if (w.esp32sim_out_kind(emu, i) !== 1) continue;
    const p = w.esp32sim_out_ptr(emu, i), len = w.esp32sim_out_len(emu, i);
    const msg = JSON.parse(dec.decode(mem().subarray(p, p + len)));
    if (msg.t === 'serial' && msg.src === 'uart0') uart += msg.data;
  }
  records();
  await new Promise(r => setImmediate(r));
  for (const f of pending.splice(0)) f();
  for (const id of tcp.keys()) offer(id);
}
for (const f of tcp.values()) f.sock.destroy();
for (const u of udp.values()) u.s.close();
for (const line of uart.split('\n').filter(Boolean)) console.log(`uart: ${line}`);
console.log(`relay: ${JSON.stringify(counts)}`);
