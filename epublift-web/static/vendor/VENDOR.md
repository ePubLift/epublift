# Vendored: epubveri and epubsana (client-side Validate and Repair)

Two published veripublica WebAssembly packages run in the browser, so the book
never leaves the page: epubveri for Validate, epubsana for Repair. Each is
copied verbatim from npm; only its tiny entry file is replaced by ours.

## epubveri (Validate)

| | |
| --- | --- |
| Package | [`@veripublica/epubveri-wasm`](https://www.npmjs.com/package/@veripublica/epubveri-wasm) **0.20.0** |
| npm integrity | `sha512-DxI2g05RQcN49lJSkxwJMhBNAg9yBDPOU/yVtDr2TPFFcpgDEA3r07g46FdP8iCA1NVUMgZBEiL+T/LnwSyl5g==` |
| Built from | [veripublica/epubveri](https://github.com/veripublica/epubveri) tag `v0.20.0`, commit `1c58e13f8eec03dfd9f72ea7b12314d27fd9ed06`, by its `publish-npm.yml` workflow (SLSA provenance attestation on the npm release) |
| Must match | the `epubveri` version in the root `Cargo.toml` — the CLI's `epublift check` and this browser build have to agree about the same book |

### Files

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

### Updating

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

## epubsana (Repair)

| | |
| --- | --- |
| Package | [`@veripublica/epubsana-wasm`](https://www.npmjs.com/package/@veripublica/epubsana-wasm) **0.22.0** |
| npm integrity | `sha512-q2+mUGRDUKqL/hp5MBK/szZugpWGuAhxsmS1tFfNJxK+tYUnLppZKwW6uO4R+kvbrp8uq9fkND6001w3r2Ehhw==` |
| Built from | [veripublica/epubsana](https://github.com/veripublica/epubsana) tag `v0.22.0`, commit `f15fa7da6e1001c3f096d254ee736ff85abcc9f3` |
| Must match | the `epubsana` version in the root `Cargo.toml` — `epublift repair` and this browser build run the same `repair()`, so the same book with the same fixes approved comes back identical |

It carries its own copy of epubveri (the repairer validates before and after
every repair), so it does not share the Validate module above. The two must
still be on the same epubveri minor; epubsana's `Cargo.toml` states its floor.

### Files

| File | Origin | SHA-256 |
| --- | --- | --- |
| `epubsana_wasm_bg.wasm` | verbatim from the package | `dd6370ae553b9e0ed09fcbc6258d1ff69730316cbb976b5d9ee6de071fa38e16` |
| `epubsana_wasm_bg.js` | verbatim from the package (wasm-bindgen glue) | `864a325245dee7034944137842ecf63b1b5a7bff780513b8b41986cac8fc08ea` |
| `epubsana-LICENSE`, `epubsana-LICENSE-COMMERCIAL.md` | the package's `LICENSE` and `LICENSE-COMMERCIAL.md` | — |
| `epubsana.js` | **ours** — the browser loader, the same shape as `epubveri.js` | — |

### Updating

Bump the `epubsana` crate in `Cargo.toml` in the same change, then:

```sh
npm pack @veripublica/epubsana-wasm@<version>      # prints the integrity
npm view @veripublica/epubsana-wasm@<version> gitHead
tar xzf veripublica-epubsana-wasm-<version>.tgz
cp package/epubsana_wasm_bg.wasm package/epubsana_wasm_bg.js epublift-web/static/vendor/
cp package/LICENSE epublift-web/static/vendor/epubsana-LICENSE
cp package/LICENSE-COMMERCIAL.md epublift-web/static/vendor/epubsana-LICENSE-COMMERCIAL.md
shasum -a 256 epublift-web/static/vendor/epubsana_wasm_bg.*
```

Update the table above, check that the package still imports only from
`./epubsana_wasm_bg.js` and still exports `Session`, `version`,
`__wbg_set_wasm` and `__wbindgen_start`, and check that the `Session`, `Plan`,
`Fix` and `Report` shapes in `package/epubsana_wasm.d.ts` still match what
`app.js` reads. Note the two indices: `Fix.index` (0-based) is what
`Session.repair` takes; a report item's `data.index` is 1-based, for the CLI.
