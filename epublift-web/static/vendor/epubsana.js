// Browser loader for epubsana's published WebAssembly build.
//
// `epubsana_wasm_bg.js` and `epubsana_wasm_bg.wasm` are copied byte-for-byte
// from the npm package @veripublica/epubsana-wasm (see VENDOR.md here for the
// version and digests). Like epubveri's, that package targets bundlers: its
// entry file does `import * as wasm from "./epubsana_wasm_bg.wasm"`, which a
// browser cannot load without one. This file replaces only that entry file,
// exactly as `epubveri.js` does for the validator. Nothing in the published
// code is changed.

import * as bg from './epubsana_wasm_bg.js';

export { Session, version } from './epubsana_wasm_bg.js';

let ready = null;

/** Fetch and instantiate the WASM module once; later calls reuse it. */
export default function init() {
  if (!ready) {
    ready = (async () => {
      const url = new URL('./epubsana_wasm_bg.wasm', import.meta.url);
      const { instance } = await WebAssembly.instantiateStreaming(fetch(url), {
        './epubsana_wasm_bg.js': bg,
      });
      bg.__wbg_set_wasm(instance.exports);
      instance.exports.__wbindgen_start();
    })();
  }
  return ready;
}
