import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

let Processor;
const packets = [];
vm.runInNewContext(readFileSync(new URL('../web/pao-microphone.js', import.meta.url), 'utf8'), {
  AudioWorkletProcessor: class { constructor() { this.port = {postMessage: packet => packets.push(packet)}; } },
  registerProcessor: (name, processor) => { assert.equal(name, 'pao-microphone'); Processor = processor; },
});
const processor = new Processor();
assert.equal(processor.process([]), true);
for (let i = 0; i < 7; i++) processor.process([[new Float32Array(128).fill(1)]]);
assert.equal(packets.length, 0);
processor.process([[new Float32Array(128).fill(-2)]]);
assert.equal(packets.length, 1);
const data = new DataView(packets[0]);
assert.equal(data.byteLength, 2050);
assert.equal(data.getUint8(0), 5); assert.equal(data.getUint8(1), 1);
assert.equal(data.getInt16(2, true), 2047);
assert.equal(data.getInt16(2 + 896 * 2, true), -2047);
console.log('PASS: microphone batching, PCM16 framing, clipping and slot gain');
