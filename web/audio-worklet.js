// Ring-buffer player: the main thread posts interleaved stereo f32 chunks, we play them back.
// If the buffer runs dry we output silence; if it grows beyond MAX frames we drop the oldest audio
// so latency never drifts upward.
const CAPACITY = 1 << 16; // stereo frames (power of two not required, just large)
const MAX_LATENCY_FRAMES = 0.25; // seconds

class GbPlayer extends AudioWorkletProcessor {
  constructor() {
    super();
    this.l = new Float32Array(CAPACITY);
    this.r = new Float32Array(CAPACITY);
    this.read = 0;
    this.write = 0;
    this.count = 0;
    this.port.onmessage = (e) => {
      const d = e.data;
      if (d === 'clear') { this.read = this.write = this.count = 0; return; }
      const n = d.length >> 1;
      for (let i = 0; i < n; i++) {
        this.l[this.write] = d[2 * i];
        this.r[this.write] = d[2 * i + 1];
        this.write = (this.write + 1) % CAPACITY;
      }
      this.count += n;
      const max = Math.floor(sampleRate * MAX_LATENCY_FRAMES);
      if (this.count > max) {
        const drop = this.count - max;
        this.read = (this.read + drop) % CAPACITY;
        this.count = max;
      }
    };
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const L = out[0], R = out[1] || out[0];
    for (let i = 0; i < L.length; i++) {
      if (this.count > 0) {
        L[i] = this.l[this.read];
        R[i] = this.r[this.read];
        this.read = (this.read + 1) % CAPACITY;
        this.count--;
      } else {
        L[i] = R[i] = 0;
      }
    }
    return true;
  }
}

registerProcessor('gb-player', GbPlayer);
