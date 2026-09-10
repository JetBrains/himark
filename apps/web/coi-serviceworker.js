// Cross-origin isolation for hosts that cannot set response headers
// (GitHub Pages): a service worker re-serves every same-origin fetch with
// COOP/COEP, which SharedArrayBuffer — and so the shared-memory wasm
// build — requires. First visit registers the worker and reloads once.
if (typeof window === "undefined") {
  self.addEventListener("install", () => self.skipWaiting());
  self.addEventListener("activate", (event) => event.waitUntil(self.clients.claim()));
  self.addEventListener("fetch", (event) => {
    const request = event.request;
    if (request.cache === "only-if-cached" && request.mode !== "same-origin") return;
    event.respondWith(
      fetch(request).then((response) => {
        if (response.status === 0) return response;
        const headers = new Headers(response.headers);
        headers.set("Cross-Origin-Opener-Policy", "same-origin");
        headers.set("Cross-Origin-Embedder-Policy", "require-corp");
        return new Response(response.body, {
          status: response.status,
          statusText: response.statusText,
          headers,
        });
      })
    );
  });
} else if (!window.crossOriginIsolated && "serviceWorker" in navigator) {
  navigator.serviceWorker.register(document.currentScript.src).then((registration) => {
    // Once the worker controls the page, a reload picks up the headers.
    if (navigator.serviceWorker.controller) return;
    const reload = () => window.location.reload();
    if (registration.active) reload();
    else navigator.serviceWorker.ready.then(reload);
  });
}
