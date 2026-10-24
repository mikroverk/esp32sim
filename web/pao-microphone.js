// PAO's 32-bit I2S slot is shifted by 12 in the production codec. PcmInput
// left-aligns PCM16 by 16, so send 1/16 scale to retain the microphone's level.
class PaoMicrophone extends AudioWorkletProcessor {
  constructor() { super(); this.samples = new Int16Array(1024); this.used = 0; }
  process(inputs) {
    const mono = inputs[0]?.[0];
    if (!mono) return true;
    for (const sample of mono) {
      this.samples[this.used++] = Math.round(Math.max(-1, Math.min(1, sample)) * 2047);
      if (this.used === this.samples.length) {
        const packet = new ArrayBuffer(2 + this.samples.length * 2);
        const bytes = new DataView(packet); bytes.setUint8(0, 5); bytes.setUint8(1, 1);
        for (let i = 0; i < this.samples.length; i++) bytes.setInt16(2 + i * 2, this.samples[i], true);
        this.port.postMessage(packet, [packet]); this.used = 0;
      }
    }
    return true;
  }
}
registerProcessor('pao-microphone', PaoMicrophone);
