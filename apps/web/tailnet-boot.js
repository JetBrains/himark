// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//
// The main-thread half of the in-page tailnet (docs/ahp/tailnet.md,
// §The browser): lazily loads the Go wasm node on the first
// Connect Tailscale, runs the ephemeral login (browse-to-URL in a
// new tab), and keeps the snapshot the engine polls — state,
// authUrl and the himark-* hosts from the netmap.
(() => {
  const t = (globalThis.himarkTailnet = {
    started: false,
    state: 'Stopped',
    authUrl: '',
    hosts: [],
    conns: new Map(),
    ipn: null,
    start() {
      if (t.started) {
        // A re-ask while the login is pending re-opens the URL —
        // the first tab may have been lost.
        if (t.authUrl && t.state === 'NeedsLogin') window.open(t.authUrl, '_blank');
        return;
      }
      t.started = true;
      t.state = 'Loading';
      (async () => {
        if (!globalThis.Go) {
          await new Promise((resolve, reject) => {
            const script = document.createElement('script');
            script.src = './wasm_exec.js';
            script.onload = resolve;
            script.onerror = () => reject(new Error('wasm_exec.js failed to load'));
            document.head.appendChild(script);
          });
        }
        const go = new Go();
        const { instance } = await WebAssembly.instantiateStreaming(
          fetch('./tailnet.wasm'), go.importObject);
        go.run(instance); // resolves only when the node dies
        const ipn = newIPN({
          // Per-TAB custody: the ephemeral node's key dies with the
          // session — the browser is the weakest custodian
          // (docs/ahp/tailnet.md), so nothing durable is written.
          stateStorage: {
            getState: (key) => sessionStorage.getItem('himark-ts-' + key) || '',
            setState: (key, value) => sessionStorage.setItem('himark-ts-' + key, value),
          },
        });
        t.ipn = ipn;
        ipn.run({
          notifyState: (state) => {
            t.state = state;
            if (state === 'NeedsLogin') ipn.login();
            if (state === 'Running') t.authUrl = '';
          },
          notifyBrowseToURL: (url) => {
            t.authUrl = url;
            window.open(url, '_blank');
          },
          notifyNetMap: (json) => {
            try {
              const map = JSON.parse(json);
              t.hosts = (map.peers || [])
                .filter((peer) =>
                  peer.name &&
                  peer.name.startsWith('himark-') &&
                  !peer.name.startsWith('himark-client-') &&
                  peer.online !== false)
                .map((peer) => ({
                  name: peer.name.split('.')[0],
                  dns: peer.name.replace(/\.$/, ''),
                }));
            } catch (error) {
              console.warn('[tailnet] netmap', error);
            }
          },
          notifyPanicRecover: (error) => console.error('[tailnet] panic', error),
        });
      })().catch((error) => {
        t.state = 'Failed';
        console.error('[tailnet] start', error);
      });
    },
  });
})();
