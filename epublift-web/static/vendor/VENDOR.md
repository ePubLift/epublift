# Vendored: epubveri (client-side Validate mode)

| | |
| --- | --- |
| Package | [`@veripublica/epubveri-wasm`](https://www.npmjs.com/package/@veripublica/epubveri-wasm) **0.18.0** |
| npm integrity | `sha512-xWK9N4HaGrHD4gb5pTIHUUoCG0dzhxhlfyaM8XtM7l8KO6WBOW6x/M5PF3n6PORVyFPGuX84ByWlBnyb/joBpw==` |
| Built from | [veripublica/epubveri](https://github.com/veripublica/epubveri) tag `v0.18.0`, commit `de7bd2dcfaba5118fad7263e2dc6ed80d6bda374`, by its `publish-npm.yml` workflow (SLSA provenance attestation on the npm release) |
| Must match | the `epubveri` version in the root `Cargo.toml` — the CLI's `epublift check` and this browser build have to agree about the same book |

## Files

| File | Origin | SHA-256 |
| --- | --- | --- |
| `epubveri_bg.wasm` | verbatim from the package | `06f8caffacf72e7e51a19a30796786adea565b53a622b9926b4dc8edecc37238` |
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
