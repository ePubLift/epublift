# Vendored: epubveri (client-side Validate mode)

| | |
| --- | --- |
| Package | [`@veripublica/epubveri-wasm`](https://www.npmjs.com/package/@veripublica/epubveri-wasm) **0.20.0** |
| npm integrity | `sha512-DxI2g05RQcN49lJSkxwJMhBNAg9yBDPOU/yVtDr2TPFFcpgDEA3r07g46FdP8iCA1NVUMgZBEiL+T/LnwSyl5g==` |
| Built from | [veripublica/epubveri](https://github.com/veripublica/epubveri) tag `v0.20.0`, commit `1c58e13f8eec03dfd9f72ea7b12314d27fd9ed06`, by its `publish-npm.yml` workflow (SLSA provenance attestation on the npm release) |
| Must match | the `epubveri` version in the root `Cargo.toml` — the CLI's `epublift check` and this browser build have to agree about the same book |

## Files

| File | Origin | SHA-256 |
| --- | --- | --- |
| `epubveri_bg.wasm` | verbatim from the package | `7e3a835d67769fb1328d269597bdd86a3040e14ae513057e39837851ff42a84a` |
| `epubveri_bg.js` | verbatim from the package (wasm-bindgen glue) | `34a75cedc6de02dfe0381c4ae80cb26e96868457f49a90f3a5bb341fa892959b` |
| `epubveri-LICENSE`, `epubveri-LICENSE.COMMERCIAL.md` | the package's `LICENSE` and `LICENSE.COMMERCIAL.md` | — |
| `epubveri.js` | **ours** — the browser loader | — |

The package is built for bundlers: its own entry file does
`import * as wasm from "./epubveri_bg.wasm"`, which a browser cannot load
without one. `epubveri.js` replaces only that entry file and does what a bundler
would — instantiate the `.wasm` with the glue as its import module, hand the
instance to the glue, run the start function. Nothing published is modified.

## Updating

Bump the `epubveri` crate in `Cargo.toml` in the same change, then:

```sh
npm pack @veripublica/epubveri-wasm@<version>      # prints the integrity
npm view @veripublica/epubveri-wasm@<version> gitHead
tar xzf veripublica-epubveri-wasm-<version>.tgz
cp package/epubveri_bg.wasm package/epubveri_bg.js epublift-web/static/vendor/
cp package/LICENSE epublift-web/static/vendor/epubveri-LICENSE
cp package/LICENSE.COMMERCIAL.md epublift-web/static/vendor/epubveri-LICENSE.COMMERCIAL.md
shasum -a 256 epublift-web/static/vendor/epubveri_bg.*
```

Update the table above, check that the package still imports only from
`./epubveri_bg.js` and still exports `validate`, `version` and
`__wbindgen_start` (the loader relies on those three), and check that the
`Report` shape in `package/epubveri.d.ts` still matches what `app.js` reads.
