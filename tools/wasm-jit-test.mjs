#!/usr/bin/env node
// Build with: tools/wasm-jit-test.sh
import { readFileSync } from 'node:fs';
import { createJitHost } from '../web/wasm/jit.mjs';
let w;
const host = createJitHost(() => w);
const bytes = readFileSync(new URL('../target/wasm32-unknown-unknown/release/esp32sim_wasm.wasm', import.meta.url));
w = (await WebAssembly.instantiate(bytes, {env: {...host.imports, host_profile_now: () => performance.now(), host_log(p,n) {
  console.error(new TextDecoder().decode(new Uint8Array(w.memory.buffer,p,n)));
}}})).instance.exports;
const waveform = process.argv[2] === 'gpio-waveform';
const count = waveform ? w.esp32sim_test_gpio_waveform() : w.esp32sim_test_block_jit();
if (!count || host.stats.failed || !host.stats.compiled || host.stats.compiled !== host.stats.released) {
  throw Error(`incomplete JIT test run: ${JSON.stringify(host.stats)}`);
}
console.log(waveform ? `PASS: ${count} GPIO waveform cases; ${host.stats.compiled} compiled modules released` : `PASS: ${count} WASM differential cases (ALU, helper continuation, memory, faults, loops, windows, budget/resume, invalidation, timer, script stop, peer-state MMIO); ${host.stats.compiled} compiled modules released`);
