# Self-hosting the web service (`epublift-web`)

ePubLift ships a small **web service**: drag-and-drop an EPUB in your browser and
get back the modernized file plus an in-page audit report. It's powered by the
same pure-Rust `convert()` core, and uploads are processed **in memory and deleted
immediately** — nothing is ever stored or logged. The interface is available in
**13 languages**, auto-detecting your browser language with a switcher in the
top-right.

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
