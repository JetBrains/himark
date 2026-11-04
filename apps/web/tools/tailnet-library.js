// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0
//
// The emscripten js-library half of the in-page tailnet bridge
// (docs/ahp/tailnet.md, §The browser). Every function proxies to
// the MAIN thread — the Go wasm node and `globalThis.himarkTailnet`
// (tailnet-boot.js) live there, while the callers run on the wire
// pthread. Callbacks re-enter the Rust side through the exported
// _himark_tailnet_{opened,data,closed}; `closed` is guaranteed to
// be the LAST callback for a conn on every road.

addToLibrary({
  himark_tailnet_start__proxy: 'async',
  himark_tailnet_start__sig: 'v',
  himark_tailnet_start: function () {
    const t = globalThis.himarkTailnet;
    if (t && t.start) t.start();
  },

  himark_tailnet_poll_json__proxy: 'sync',
  himark_tailnet_poll_json__sig: 'p',
  himark_tailnet_poll_json__deps: ['$stringToNewUTF8'],
  himark_tailnet_poll_json: function () {
    const t = globalThis.himarkTailnet;
    if (!t || !t.started) return 0;
    return stringToNewUTF8(
      JSON.stringify({ state: t.state, authUrl: t.authUrl, hosts: t.hosts }));
  },

  himark_tailnet_dial__proxy: 'async',
  himark_tailnet_dial__sig: 'vppp',
  himark_tailnet_dial__deps: ['$UTF8ToString', '$stringToNewUTF8'],
  himark_tailnet_dial: function (conn, hostPtr, hostLen) {
    const t = globalThis.himarkTailnet;
    const fail = (message) => {
      const text = stringToNewUTF8(String(message));
      _himark_tailnet_closed(conn, text);
      _free(text);
    };
    if (!t || !t.ipn) {
      fail('tailnet is not running — Connect Tailscale first');
      return;
    }
    const hostPort = UTF8ToString(hostPtr, hostLen);
    t.ipn.dial(hostPort).then((stream) => {
      t.conns.set(conn, stream);
      stream.start({
        onData: (bytes) => {
          const copy = _malloc(bytes.length);
          HEAPU8.set(bytes, copy);
          _himark_tailnet_data(conn, copy, bytes.length);
          _free(copy);
        },
        onClose: (reason) => {
          if (!t.conns.delete(conn)) return; // already closed
          fail(reason || 'closed');
        },
      });
      _himark_tailnet_opened(conn);
    }, (error) => fail(error));
  },

  himark_tailnet_send__proxy: 'async',
  himark_tailnet_send__sig: 'vppp',
  himark_tailnet_send: function (conn, ptr, len) {
    const t = globalThis.himarkTailnet;
    const stream = t && t.conns.get(conn);
    if (stream) stream.send(HEAPU8.slice(ptr, ptr + len));
    // Ownership: the Rust side malloc'd for us.
    _free(ptr);
  },

  himark_tailnet_close__proxy: 'async',
  himark_tailnet_close__sig: 'vp',
  himark_tailnet_close: function (conn) {
    // Ask only — the map entry STAYS until onClose, which is the
    // one road to _himark_tailnet_closed (first-wins via the map
    // delete). An unknown id is a late ask after closed: no-op.
    const t = globalThis.himarkTailnet;
    const stream = t && t.conns.get(conn);
    if (stream) stream.close();
  },
});
