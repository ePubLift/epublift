# Self-hosting the web service (`epublift-web`)

ePubLift ships a small **web service**: drag-and-drop an EPUB in your browser and
choose a mode from the switcher at the top of the panel:

- **Optimize** — modernize to EPUB 3.3 and re-encode images to WebP, with an
  in-page audit report (the classic flow). Options: image quality, ASCII
  filenames, *keep original images*, and Kobo `.kepub` output. A **Target
  version** selector also offers **EPUB 3.4** (experimental) — the draft spec's
  new **AVIF** / **JPEG XL** image formats; pick AVIF, JPEG XL, or keep original.
- **Archive** — pack a book into a compact, **lossless** [`.eparc`](archiving.md)
  archive (solid Zstandard on text + fonts, media stored verbatim). Optional
  ASCII output name. An `.eparc` is an archive, not a readable e-book — keep it
  for storage and restore it when you want to read.
- **Restore** — turn an `.eparc` back into a working `.epub`. **Content-exact by
  default** (the original book, byte-for-byte per entry); flip on **Modernize**
  to re-run the optimizer on the way out (EPUB 3.3, WebP, `--keep-images`,
  `.kepub`).
- **Metadata** — fix a book's [metadata](metadata.md): drop an `.epub` to load an
  editable form, optionally **Fetch** by ISBN from **Open Library** or **Google
  Books** to fill the gaps (**language-aware** — only the book's own language),
  then **Save & download**. The ISBN is written as a `dc:identifier` so Calibre /
  Apple Books recognize it. The lookup runs server-side over a pure-Rust TLS
  client (no C). Google Books needs an API key for reliable use — see
  [Configuration](#configuration-env).
- **Import PDF** *(experimental)* — turn a [PDF into a reflowable EPUB](pdf-import.md):
  drop a `.pdf`, pick the book's language, convert. Works for PDFs that carry a
  text layer (born-digital books and already-searchable scans), including modern
  Type0/CID fonts. Scanned-only PDFs need OCR, a later phase — they're detected
  and reported, never silently broken.

It's powered by the same pure-Rust core, and every upload is processed **in memory
and deleted immediately** — nothing is ever stored or logged, in any mode. The
interface is available in **13 languages**, auto-detecting your browser language
with a switcher in the top-right.

> 💡 A hosted instance runs at **<https://epublift.itpax.net>**.

## Run with Docker

A hardened, multi-arch (amd64 + arm64) image is published to the GitHub Container
Registry on every release:

```bash
docker run -d --name epublift-web \
  -p 127.0.0.1:8080:8080 \
  ghcr.io/epublift/epublift-web:latest
```

Then open <http://127.0.0.1:8080>. Pin a specific version with a tag instead of
`latest`, e.g. `ghcr.io/epublift/epublift-web:1.4.0`. The image is a static musl
binary on Alpine, runs as a non-root user, and is only ~14 MB.

## Run with Docker Compose (recommended)

The repo's [`docker-compose.yml`](../docker-compose.yml) adds the full hardening
profile — read-only root filesystem, all Linux capabilities dropped,
`no-new-privileges`, memory/PID limits, and a `tmpfs` for the only writable path:

```bash
docker compose up -d
```

## Configuration (`.env`)

Optional settings are read from a **`.env`** file next to `docker-compose.yml` —
Docker Compose loads it automatically, so there's no `export` and no editing the
compose file. Copy the template and fill in what you need:

```bash
cp .env.example .env      # then edit .env
docker compose up -d
```

| Variable | Purpose |
| :--- | :--- |
| `GOOGLE_BOOKS_API_KEY` | A [Google Books API key](https://console.cloud.google.com/) for the **Metadata** editor's ISBN enrichment when you pick the *Google Books* provider. Anonymous requests share a small daily quota (HTTP 429 when exhausted); a key raises it. **Optional** — leave it blank to use Open Library only (the default provider needs no key). |
| `MISTRAL_API_KEY` | A [Mistral API key](https://console.mistral.ai/) that enables the **[EXPERIMENTAL] Smart Import** mode (AI OCR: PDF → EPUB, including scans/photos). **Optional** — leave it blank and Smart Import stays switched off (the UI shows an "add an API key" notice). The key stays on the server and is never sent to browsers; with it set, uploaded PDFs are sent to Mistral for OCR and **this key pays for the calls**. See [Smart Import](smart-import.md). |

Your real `.env` is git-ignored, so your key stays private.

| `EPUBLIFT_LOG_DIR` | Directory for the rolling **WARN+** log file (default `logs/`). The service always logs to stdout too (`docker logs`). In Docker the container's working dir isn't writable, so to keep file logs point this at a mounted directory (e.g. `EPUBLIFT_LOG_DIR=/logs` with a `./logs:/logs` volume). If the directory can't be created, it falls back to stdout only. |
| `RUST_LOG` | Log verbosity, e.g. `RUST_LOG=epublift_web=debug,tower_http=debug` (default `epublift_web=info,tower_http=warn`). |

No API keys or uploaded content are ever logged.

## Put it behind a reverse proxy (TLS)

The service speaks plain HTTP on port `8080` and binds to localhost, so terminate
TLS with a reverse proxy (Nginx Proxy Manager, Caddy, Traefik, …) in front of it.
One required setting: raise the proxy's max request-body size to match the
service's **50 MiB** upload limit — otherwise large uploads are rejected at the
proxy with `413`. For Nginx (and Nginx Proxy Manager's *Advanced* tab):

```nginx
client_max_body_size 50M;
```

If your proxy **also runs as a container**, it can't reach the host's
`127.0.0.1:8080`. Put both on a shared Docker network and point the proxy at the
service by name — `http://epublift-web:8080` (the container's internal port
`8080`, regardless of how the host port is published).

## Security & privacy

The endpoint is public and the source is open, so these limits are the real
defense — not obscurity:

*   **No retention** — each request is converted in a temp dir wiped on success *or* error; no upload is stored or logged.
*   **Strict Content-Security-Policy** (`default-src 'none'`) plus `X-Frame-Options`, `X-Content-Type-Options`, and locked-down CORS on every response.
*   **Abuse limits** — a 50 MiB body cap, a request timeout, per-IP rate limiting, and a concurrency cap that keeps latency predictable.
*   **Input hardening** (shared with the CLI) — zip-bomb (uncompressed-size + entry-count caps) and image decode-bomb (dimension/allocation limits) guards.
*   **Optional egress-blocking** — the converter never makes outbound connections, so `docker-compose.yml` documents how to run it on an `internal` Docker network with no route to the internet at all.

> **AGPL-3.0 note:** if you run a **modified** copy of this service over a network, §13 requires you to offer your modified source to its users. The page carries a visible **Source** link to satisfy this.
