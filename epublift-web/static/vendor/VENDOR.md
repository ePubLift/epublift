# Vendored: epubveri and epubsana (client-side Validate and Repair)

Two published veripublica WebAssembly packages run in the browser, so the book
never leaves the page: epubveri for Validate, epubsana for Repair. Each is
copied verbatim from npm; only its tiny entry file is replaced by ours.

## epubveri (Validate)

| | |
| --- | --- |
| Package | [`@veripublica/epubveri-wasm`](https://www.npmjs.com/package/@veripublica/epubveri-wasm) **0.43.0** |
| npm integrity | `sha512-tziNXYDSeWPhMihEUJKmYugAWg9UPHU4gZnqnzK008Nj4ZfqbejiOgnZrJhGs5UxewUFlCJaV9sg/LhDTD+wbw==` |
| Built from | [veripublica/epubveri](https://github.com/veripublica/epubveri) tag `v0.43.0`, commit `5ea8094f06df3a2d71a3ca831feeb356024060ad`, by its `publish-npm.yml` workflow (SLSA provenance attestation on the npm release) |
| Must match | the `epubveri` version in the root `Cargo.toml` — the CLI's `epublift check` and this browser build have to agree about the same book |

### Files

| File | Origin | SHA-256 |
| --- | --- | --- |
| `epubveri_bg.wasm` | verbatim from the package | `27b9b88fcbe8ace2ff52c79042e1743f952040d8c576cb3290a5fee7c41f6ad9` |
| `epubveri_bg.js` | verbatim from the package (wasm-bindgen glue) | `ab0ddf49e971e56befa03d12bc97bca54fc49fc261a0a5567121b36ddf9cb311` |
| `epubveri-LICENSE`, `epubveri-LICENSE.COMMERCIAL.md`, `epubveri-LICENSE-THIRD-PARTY.txt` | the package's `LICENSE`, `LICENSE.COMMERCIAL.md` and `LICENSE-THIRD-PARTY.txt` | — |
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
cp package/LICENSE-THIRD-PARTY.txt epublift-web/static/vendor/epubveri-LICENSE-THIRD-PARTY.txt
shasum -a 256 epublift-web/static/vendor/epubveri_bg.*
```

Update the table above, check that the package still imports only from
`./epubveri_bg.js` and still exports `validate`, `version` and
`__wbindgen_start` (the loader relies on those three), and check that the
`Report` shape in `package/epubveri.d.ts` still matches what `app.js` reads.

## epubsana (Repair)

| | |
| --- | --- |
| Package | [`@veripublica/epubsana-wasm`](https://www.npmjs.com/package/@veripublica/epubsana-wasm) **0.41.0** |
| npm integrity | `sha512-434e1ZuC8UYAkV4k97kshtzyuSMx16i6zEikR9xOaZ9izAX3/gX2weNjDvr7eqd5JpZFKiPzhUzt9/R9TJb8eg==` |
| Built from | [veripublica/epubsana](https://github.com/veripublica/epubsana) tag `v0.41.0`, commit `82c5beaaa52a64bc2cf6d80cbcfc19dcd85541ba` |
| Must match | the `epubsana` version in the root `Cargo.toml` — `epublift repair` and this browser build run the same `repair()`, so one pass over the same book with the same fixes approved comes back identical. `repair` goes on by itself while a fix reveals more, up to 3 passes; the page offers "Find more fixes" instead, and after it the result is the same |

It carries its own copy of epubveri (the repairer validates before and after
every repair), so it does not share the Validate module above. The two must
still be on the same epubveri minor; epubsana's `Cargo.toml` states its floor.

### Files

| File | Origin | SHA-256 |
| --- | --- | --- |
| `epubsana_wasm_bg.wasm` | verbatim from the package | `ed7afb10f4764b690a6f6600faeede03b962e9a84a7a74a04f82ce8ca76b67b5` |
| `epubsana_wasm_bg.js` | verbatim from the package (wasm-bindgen glue) | `e5a8b7a18987c531c7d806d061859b5a8f7cbc9ecb6d6b9616b53eb8c53fa599` |
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
