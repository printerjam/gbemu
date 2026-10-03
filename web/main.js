import init, { Emulator } from './pkg/gb_wasm.js';

const FPS = 59.7275;
const FRAME_MS = 1000 / FPS;
const MAX_CATCHUP_FRAMES = 5;
const PREFILL_SECONDS = 0.06;

const $ = (id) => document.getElementById(id);
const canvas = $('screen');
const ctx2d = canvas.getContext('2d', { alpha: false });
const image = ctx2d.createImageData(160, 144);
const statusEl = $('status');

let wasm = null;
let emu = null;
let paused = false;
let lastTime = 0;
let acc = 0;
let frames = 0;
let fpsFrames = 0;
let fpsStart = performance.now();
let measuredFps = 0;
let saveKey = null;
let lastSaved = '';
let audio = null;

// ---------- persistence ----------
const b64 = (u8) => { let s = ''; for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode(...u8.subarray(i, i + 0x8000)); return btoa(s); };
const unb64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const romId = () => `${emu.title().trim() || 'untitled'}:${emu.rom_hash().toString(16).padStart(8, '0')}`;

function persistBattery(force = false) {
  if (!emu || !emu.has_battery()) return;
  const data = b64(emu.save_data(Date.now() / 1000));
  if (!force && data === lastSaved) return;
  try { localStorage.setItem(saveKey, data); lastSaved = data; } catch (e) { setStatus(`Cannot save: ${e.message}`); }
}
setInterval(() => persistBattery(), 5000);
addEventListener('pagehide', () => persistBattery(true));
document.addEventListener('visibilitychange', () => { if (document.hidden) persistBattery(true); });

// ---------- audio ----------
async function startAudio() {
  if (audio) { if (audio.ctx.state === 'suspended') audio.ctx.resume(); return; }
  const AC = window.AudioContext || window.webkitAudioContext;
  if (!AC) return;
  const actx = new AC({ latencyHint: 'interactive' });
  audio = { ctx: actx, push: () => {}, mode: 'none' };
  const prefill = new Float32Array(Math.round(actx.sampleRate * PREFILL_SECONDS) * 2);
  if (actx.audioWorklet && window.isSecureContext) {
    try {
      await actx.audioWorklet.addModule(new URL('./audio-worklet.js', import.meta.url));
      const node = new AudioWorkletNode(actx, 'gb-player', { numberOfInputs: 0, outputChannelCount: [2] });
      node.connect(actx.destination);
      audio.push = (f32) => node.port.postMessage(f32.slice());
      audio.clear = () => node.port.postMessage('clear');
      audio.mode = 'worklet';
      audio.push(prefill);
    } catch (e) { console.warn('AudioWorklet failed, falling back', e); }
  }
  if (audio.mode === 'none') {
    // ScriptProcessor fallback with a simple queue.
    const queue = [];
    let cur = null, pos = 0, queued = 0;
    const node = actx.createScriptProcessor(2048, 0, 2);
    node.onaudioprocess = (ev) => {
      const L = ev.outputBuffer.getChannelData(0), R = ev.outputBuffer.getChannelData(1);
      for (let i = 0; i < L.length; i++) {
        if (!cur || pos >= cur.length) { cur = queue.shift() || null; pos = 0; if (cur) queued -= cur.length; }
        if (cur) { L[i] = cur[pos++]; R[i] = cur[pos++]; } else { L[i] = R[i] = 0; }
      }
    };
    node.connect(actx.destination);
    audio.push = (f32) => {
      queue.push(f32.slice()); queued += f32.length;
      while (queued > actx.sampleRate * 0.5) { queued -= queue.shift().length; }
    };
    audio.clear = () => { queue.length = 0; queued = 0; cur = null; };
    audio.mode = 'script';
    audio.push(prefill);
  }
  if (emu) emu.set_sample_rate(actx.sampleRate);
  if (actx.state === 'suspended') actx.resume();
}

// ---------- ROM loading ----------
function setStatus(msg) { statusEl.textContent = msg; }

async function loadRom(bytes, name) {
  let next;
  try {
    next = new Emulator(bytes);
  } catch (e) {
    setStatus(`Cannot load ${name}: ${e.message || e}`);
    return;
  }
  persistBattery(true);
  emu = next;
  saveKey = `gb:sav:${romId()}`;
  lastSaved = '';
  await startAudio();
  if (audio) { emu.set_sample_rate(audio.ctx.sampleRate); audio.clear?.(); audio.push(new Float32Array(Math.round(audio.ctx.sampleRate * PREFILL_SECONDS) * 2)); }
  emu.set_palette(Number($('palette').value));
  const sav = emu.has_battery() && localStorage.getItem(saveKey);
  if (sav) { try { emu.load_save_data(unb64(sav), Date.now() / 1000); lastSaved = sav; } catch (e) { console.warn(e); } }
  $('hint').style.display = 'none';
  for (const id of ['pause', 'reset']) $(id).disabled = false;
  const states = typeof emu.save_state === 'function';
  $('savestate').disabled = !states;
  $('loadstate').disabled = !states;
  lastRom = { bytes, name };
  paused = false;
  $('pause').textContent = 'Pause';
  acc = 0; lastTime = 0; frames = 0;
  setStatus(`${emu.title() || name}${emu.has_battery() ? ' · battery save' : ''}`);
  document.title = `gbemu — ${emu.title() || name}`;
}
let lastRom = null;

async function loadFile(file) {
  if (!file) return;
  let bytes;
  try { bytes = new Uint8Array(await file.arrayBuffer()); } catch (e) { setStatus(`Cannot read ${file.name}: ${e.message || e}`); return; }
  await loadRom(bytes, file.name);
}

$('file').addEventListener('change', async (e) => { await loadFile(e.target.files[0]); e.target.value = ''; });
addEventListener('dragover', (e) => { e.preventDefault(); document.body.classList.add('drag'); });
addEventListener('dragleave', () => document.body.classList.remove('drag'));
addEventListener('drop', async (e) => {
  e.preventDefault();
  document.body.classList.remove('drag');
  await loadFile(e.dataTransfer.files[0]);
});

// ---------- controls ----------
$('palette').value = localStorage.getItem('gb:palette') || '0';
$('palette').addEventListener('change', (e) => {
  localStorage.setItem('gb:palette', e.target.value);
  emu?.set_palette(Number(e.target.value));
  e.target.blur();
  if (emu) draw();
});
$('pause').addEventListener('click', togglePause);
$('reset').addEventListener('click', () => lastRom && loadRom(lastRom.bytes, lastRom.name));
function togglePause() {
  if (!emu) return;
  paused = !paused;
  $('pause').textContent = paused ? 'Resume' : 'Pause';
  if (paused) audio?.clear?.();
  $('pause').blur();
}

$('savestate').addEventListener('click', () => {
  try {
    localStorage.setItem(`gb:state:${romId()}`, b64(emu.save_state()));
    setStatus('State saved');
  } catch (e) { setStatus(`Save state failed: ${e.message || e}`); }
  $('savestate').blur();
});
$('loadstate').addEventListener('click', () => {
  const s = localStorage.getItem(`gb:state:${romId()}`);
  if (!s) { setStatus('No saved state for this ROM'); return; }
  try { emu.load_state(unb64(s)); draw(); audio?.clear?.(); setStatus('State loaded'); } catch (e) { setStatus(`Load state failed: ${e.message || e}`); }
  $('loadstate').blur();
});

// Button codes match gb-wasm: 0 Right, 1 Left, 2 Up, 3 Down, 4 A, 5 B, 6 Select, 7 Start.
const KEYS = {
  ArrowRight: 0, ArrowLeft: 1, ArrowUp: 2, ArrowDown: 3,
  d: 0, a: 1, w: 2, s: 3,
  x: 4, k: 4, z: 5, j: 5,
  Backspace: 6, Shift: 6, Enter: 7,
};
const touchHeld = new Map(); // pointerId -> code
const keysDown = new Map(); // key -> code

const pressedAt = new Array(8).fill(-1); // frame count when each button went down
const deferredRelease = new Set(); // releases held back until the game has seen one full frame

function refreshButton(code, flush = false) {
  const pressed = [...keysDown.values()].includes(code) || [...touchHeld.values()].includes(code);
  if (pressed) {
    if (pressedAt[code] < 0) pressedAt[code] = frames;
    deferredRelease.delete(code);
  } else if (!flush && pressedAt[code] === frames && emu) {
    // A tap shorter than one emulated frame would never be seen by the game: keep the button down
    // until the next frame has run.
    deferredRelease.add(code);
    return;
  } else {
    pressedAt[code] = -1;
  }
  emu?.set_button(code, pressed);
  document.querySelectorAll(`[data-btn="${code}"]`).forEach((el) => el.classList.toggle('on', pressed));
}

addEventListener('keydown', (e) => {
  if (e.metaKey || e.ctrlKey || e.altKey) return;
  if (e.target instanceof HTMLSelectElement && e.key.startsWith('Arrow')) return;
  startAudio();
  const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  if (k === 'p') { if (!e.repeat) togglePause(); return; }
  if (!(k in KEYS)) return;
  e.preventDefault();
  const code = KEYS[k];
  keysDown.set(k, code);
  refreshButton(code);
});
addEventListener('keyup', (e) => {
  const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  if (!keysDown.has(k)) return;
  const code = keysDown.get(k);
  keysDown.delete(k);
  refreshButton(code);
});
addEventListener('blur', () => { const codes = new Set(keysDown.values()); keysDown.clear(); codes.forEach(refreshButton); });

for (const el of document.querySelectorAll('[data-btn]')) {
  const code = Number(el.dataset.btn);
  el.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    el.setPointerCapture(e.pointerId);
    touchHeld.set(e.pointerId, code);
    startAudio();
    refreshButton(code);
  });
  const up = (e) => { if (touchHeld.delete(e.pointerId)) refreshButton(code); };
  el.addEventListener('pointerup', up);
  el.addEventListener('pointercancel', up);
  el.addEventListener('contextmenu', (e) => e.preventDefault());
}

// ---------- main loop ----------
function draw() {
  // The wasm memory may have grown (detaching old views), so create the view each time.
  const px = new Uint8ClampedArray(wasm.memory.buffer, emu.frame_ptr(), emu.frame_len());
  image.data.set(px);
  ctx2d.putImageData(image, 0, 0);
}

function pushAudio() {
  if (!audio) return;
  const n = emu.audio_len();
  if (n) audio.push(new Float32Array(wasm.memory.buffer, emu.audio_ptr(), n));
}

function tick(now) {
  requestAnimationFrame(tick);
  if (!emu || paused) { lastTime = 0; return; }
  if (!lastTime) lastTime = now;
  acc += now - lastTime;
  lastTime = now;
  // Avoid a spiral of death after a tab switch / long stall.
  if (acc > FRAME_MS * MAX_CATCHUP_FRAMES) acc = FRAME_MS;
  let ran = 0;
  while (acc >= FRAME_MS) {
    acc -= FRAME_MS;
    emu.run_frame();
    pushAudio();
    ran++;
    frames++;
  }
  if (ran) {
    for (const code of [...deferredRelease]) { deferredRelease.delete(code); refreshButton(code, true); }
    draw();
    fpsFrames += ran;
    const dt = now - fpsStart;
    if (dt >= 1000) {
      measuredFps = (fpsFrames * 1000) / dt;
      fpsFrames = 0; fpsStart = now;
      setStatus(`${emu.title()} · ${measuredFps.toFixed(1)} fps${audio ? ` · audio ${audio.mode}` : ''}`);
    }
  }
}

(async () => {
  wasm = await init();
  requestAnimationFrame(tick);
  // Test/debug hook.
  window.gbdebug = {
    get frames() { return frames; },
    get fps() { return measuredFps; },
    get emu() { return emu; },
    get wasm() { return wasm; },
    get audioState() { return audio ? `${audio.mode}/${audio.ctx.state}/${audio.ctx.currentTime.toFixed(2)}` : null; },
    loadRom,
    press: (code, down) => { emu?.set_button(code, down); },
  };
  window.gbdebug.ready = true;
})();
