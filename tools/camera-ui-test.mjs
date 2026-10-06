// Exercise the actual page camera handlers without a physical webcam.
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
const html = readFileSync(new URL('../web/run.html', import.meta.url), 'utf8');
const elements = new Map();
const element = id => {
  if (!elements.has(id)) elements.set(id, { style: {}, disabled: false, textContent: '', getContext: () => ({}) });
  return elements.get(id);
};
let calls = 0, timer = null, track;
const context = vm.createContext({
  $: element, navigator: { mediaDevices: { getUserMedia: async () => { calls++; track = { stopped: false, stop() { this.stopped = true; } }; return { getTracks: () => [track] }; } } },
  window: { addEventListener() {} }, setInterval: f => { timer = f; return 1; }, clearInterval: () => { timer = null; },
});
vm.runInContext(html.slice(html.indexOf('// ---- camera source'), html.indexOf('</script>', html.indexOf('// ---- camera source'))), context);
vm.runInContext(html.slice(html.indexOf('function setEmulatorStatus'), html.indexOf('function connect()')), context);
assert.equal(calls, 0);
for (const status of ['stopped: code 1', 'boot failed: test', 'disconnected']) {
  await element('#camwebcam').onclick();
  assert.equal(track.stopped, false);
  context.status = status;
  vm.runInContext('setEmulatorStatus(status)', context);
  assert.equal(track.stopped, true);
  assert.equal(timer, null);
}
context.navigator.mediaDevices = undefined;
await element('#camwebcam').onclick();
assert.match(element('#camstat').textContent, /HTTPS or localhost/);
// A permission response arriving after stop must not restart capture.
let resolve;
context.navigator.mediaDevices = { getUserMedia: () => new Promise(r => { resolve = r; }) };
const pending = element('#camwebcam').onclick();
vm.runInContext("setEmulatorStatus('boot failed: test')", context);
track = { stopped: false, stop() { this.stopped = true; } };
resolve({ getTracks: () => [track] });
await pending;
assert.equal(track.stopped, true);
assert.equal(timer, null);
console.log('PASS: click-only webcam; stop, boot failure, disconnect and pending permission cleanup; insecure-origin message');
