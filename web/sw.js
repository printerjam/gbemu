// Service worker: precache the app shell and the wasm so the page works offline. Network first (so a
// rebuilt app is picked up on the next load), falling back to the cache when offline.
const CACHE = 'gbemu-v1';
const SHELL = [
  './', 'index.html', 'main.js', 'audio-worklet.js', 'manifest.webmanifest',
  'icons/icon-192.png', 'icons/icon-512.png', 'pkg/gb_wasm.js', 'pkg/gb_wasm_bg.wasm',
];

self.addEventListener('install', (e) => {
  e.waitUntil(
    caches.open(CACHE).then((c) => Promise.all(SHELL.map((u) => c.add(u).catch((err) => console.warn('precache miss', u, err))))
    ).then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches.keys().then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (e) => {
  const req = e.request;
  if (req.method !== 'GET' || new URL(req.url).origin !== location.origin) return;
  e.respondWith((async () => {
    const cache = await caches.open(CACHE);
    try {
      const res = await fetch(req);
      if (res.ok) cache.put(req, res.clone());
      return res;
    } catch (err) {
      const hit = await cache.match(req, { ignoreSearch: true });
      if (hit) return hit;
      if (req.mode === 'navigate') { const shell = await cache.match('index.html'); if (shell) return shell; }
      throw err;
    }
  })());
});
