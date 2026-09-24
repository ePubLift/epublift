// Browser loader for epubveri's published WebAssembly build.
//
// `epubveri_bg.js` and `epubveri_bg.wasm` are copied byte-for-byte from the npm
// package @veripublica/epubveri-wasm (see VENDOR.md here for the version and
// digests), which epubveri's CI publishes with a provenance attestation. That
// package targets bundlers: its own 240-byte entry file does
// `import * as wasm from "./epubveri_bg.wasm"`, which a browser cannot load
// without one. This file replaces only that entry file, doing what a bundler
// would: instantiate the .wasm with the glue as its import module, hand the
// instance to the glue, and run the start function. Nothing in the published
// code is changed.

import * as bg from './epubveri_bg.js';

export { validate, version } from './epubveri_bg.js';

let ready = null;

/** Fetch and instantiate the WASM module once; later calls reuse it. */
export default function init() {
  if (!ready) {
    ready = (async () => {
      const url = new URL('./epubveri_bg.wasm', import.meta.url);
      const { instance } = await WebAssembly.instantiateStreaming(fetch(url), {
        './epubveri_bg.js': bg,
      });
      bg.__wbg_set_wasm(instance.exports);
      instance.exports.__wbindgen_start();
    })();
  }
  return ready;
}
