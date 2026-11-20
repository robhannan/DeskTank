// build-web.ts supplies these constants from the finished site.
declare const BUILD_HASH: string;
declare const ASSETS: string[];
const worker = self as unknown as ServiceWorkerGlobalScope;
const scope = worker.registration.scope;
const prefix = `desktank:${encodeURIComponent(scope)}:`;
const cacheName = prefix + BUILD_HASH;
const assets = new Set(ASSETS.map((file) => new URL(file, scope).href));

worker.addEventListener("install", (event) => {
  // Bypass the HTTP cache so the new cache holds one complete build.
  event.waitUntil(caches.open(cacheName).then((cache) =>
    cache.addAll(Array.from(assets, (url) => new Request(url, { cache: "reload" })))));
});

// Let existing tabs finish on their original build before clearing it.
worker.addEventListener("activate", (event) => {
  event.waitUntil((async () => {
    for (const key of await caches.keys()) {
      if (key.startsWith(prefix) && key !== cacheName) await caches.delete(key);
    }
    await worker.clients.claim();
  })());
});

worker.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET") return;
  const url = new URL(request.url);
  if (request.mode === "navigate") {
    url.search = "";
    if (url.href === scope) url.pathname += "index.html";
  }
  if (!assets.has(url.href)) return;
  event.respondWith(caches.match(url.href, { cacheName }).then((hit) => hit ?? fetch(request)));
});
