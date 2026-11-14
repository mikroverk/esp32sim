// The receipt's page-side run: a Wi-Fi firmware on the S3 in the WASM build, with the virtual
// access point of `esp32sim_wifi` and the networks around set through `esp32sim_wifi_nearby`
// before boot and, optionally, while it runs.
//   node wifi-nearby-run.mjs MODULE ROM BOOTLOADER PTABLE APP WIFI PLAN SECONDS
// WIFI is `esp32sim_wifi`'s spec. PLAN is a JSON list of
// `{ "at": SECONDS, "specs": [--wifi-nearby SPEC, ...] }`; `at` 0 is before boot. Prints each line
// of UART0 (the CLI's `--console uart0`) as `uart: LINE`, each list set as `nearby AT: N specs` or
// `nearby AT: refused`, and the `host_log` lines that followed it as `log: LINE`. With TIMES=1 a
// UART line carries the emulated time at which the page took it (`uart T: LINE`).
import { readFileSync } from 'node:fs';
import { createJitHost } from '../../../web/wasm/jit.mjs';

const [modulePath, rom, bootloader, ptable, app, wifi, planPath, seconds] = process.argv.slice(2);
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
const bytes = (data, f) => { const p = w.esp32sim_alloc(data.length); mem().set(data, p); try { return f(p, data.length); } finally { w.esp32sim_free(p, data.length); } };
const times = process.env.TIMES === '1';

const emu = bytes(enc.encode('none'), (p, n) => w.esp32sim_new(p, n, 4, 0));
if (!emu) throw new Error(`esp32sim_new: ${logs.join(' | ')}`);
for (const [kind, path] of [[0, rom], [1, bootloader], [2, ptable], [3, app]]) {
  if (bytes(readFileSync(path), (p, n) => w.esp32sim_load(emu, kind, p, n)) !== 0) throw new Error(`load ${path}: ${logs.join(' | ')}`);
}
if (bytes(enc.encode(wifi), (p, n) => w.esp32sim_wifi(emu, p, n)) !== 0) throw new Error(`esp32sim_wifi: ${logs.at(-1)}`);

const plan = JSON.parse(readFileSync(planPath, 'utf8'));
const out = [];
const setNearby = (at, specs) => {
  const before = logs.length;
  const rc = bytes(enc.encode(specs.join('\n')), (p, n) => w.esp32sim_wifi_nearby(emu, p, n));
  out.push(`nearby ${at}: ${rc === 0 ? `${specs.length} specs` : 'refused'}`);
  for (const line of logs.slice(before)) out.push(`log: ${line}`);
};
for (const step of plan.filter(s => s.at === 0)) setNearby(0, step.specs);
if (w.esp32sim_boot(emu, 0) !== 0) throw new Error(`boot: ${logs.join(' | ')}`);

const hz = Number(w.esp32sim_cpu_hz(emu));
let pending = '';
const drain = () => {
  const n = w.esp32sim_out_take(emu);
  for (let i = 0; i < n; i++) {
    if (w.esp32sim_out_kind(emu, i) !== 1) continue;
    const p = w.esp32sim_out_ptr(emu, i), len = w.esp32sim_out_len(emu, i);
    const msg = JSON.parse(dec.decode(mem().subarray(p, p + len)));
    if (msg.t !== 'serial' || msg.src !== 'uart0') continue;
    pending += msg.data;
    for (let nl; (nl = pending.indexOf('\n')) >= 0; pending = pending.slice(nl + 1)) {
      const line = pending.slice(0, nl).replace(/\r$/, '');
      if (!line) continue;
      const t = (Number(w.esp32sim_cycles(emu)) / hz).toFixed(3);
      out.push(times ? `uart ${t}: ${line}` : `uart: ${line}`);
    }
  }
};
const runTo = cycles => {
  while (Number(w.esp32sim_cycles(emu)) < cycles) {
    const left = Math.min(cycles - Number(w.esp32sim_cycles(emu)), 2_000_000);
    const rc = w.esp32sim_run(emu, left, Date.now());   // SNTP answers from the wall clock, as the CLI's does
    if (rc !== 0) throw new Error(`esp32sim_run ${rc}: ${logs.slice(-3).join(' | ')}`);
    drain();
  }
};
for (const step of plan.filter(s => s.at > 0).sort((a, b) => a.at - b.at)) {
  runTo(Math.round(step.at * hz));
  setNearby(step.at, step.specs);
}
runTo(Math.round(Number(seconds) * hz));
drain();
if (pending) out.push(`uart: ${pending}`);
for (const line of out) console.log(line);
