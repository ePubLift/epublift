# Smart Import — AI OCR (experimental)

**Smart Import** turns a **PDF** — including scans and photos with no text layer —
into a reflowable EPUB by sending it to an **AI OCR** provider, which returns
Markdown that epublift's offline [Markdown importer](markdown-import.md) then
builds into the book.

Status: **experimental**, web-only, behind the `smart-import` build feature.

## How it fits

Smart Import is deliberately the **only** part of epublift that sends your content
to a third party. Everything else — Optimize, Archive, Restore, Metadata, and the
plain Markdown/PDF **Import** — runs fully locally. So Smart Import is:

- **Opt-in and isolated.** It's a separate mode; the offline core never depends
  on it. Under the hood the AI returns Markdown, and the same pure-Rust
  Markdown → EPUB engine (with image embedding) produces the book.
- **Bring-your-own-key.** The provider API key lives only in the **server's**
  environment (e.g. `MISTRAL_API_KEY`). It is **never** sent to the browser.

## Enabling it (self-host)

Smart Import is **off** until a provider key is configured. Set one in your
`.env` (next to `docker-compose.yml`) and restart:

```bash
# .env
MISTRAL_API_KEY=your-key-here
```

Get a key at <https://console.mistral.ai/>. With no key set, the Smart Import
mode shows an "add an API key to enable this" notice instead of the upload form.

The page asks the server a small capability question on load (`GET /config`) and
renders the locked notice or the provider controls accordingly — it only ever
learns *whether* a key exists and which providers are available, never the key.

## Providers

| Provider | Env var | Notes |
| :--- | :--- | :--- |
| Mistral OCR | `MISTRAL_API_KEY` | Calls `POST /v1/ocr` (`mistral-ocr-latest`). Good with layout, tables and equations. |

The provider is a dropdown so more (Claude, GPT, …) can be added later without
UI changes.

## Privacy & cost

- The uploaded PDF is sent to the selected provider for OCR. **This leaves your
  machine** — unlike every other mode.
- epublift stores nothing: the upload and the result live in memory only, and the
  result is offered for a single short-lived download.
- The **server's** key pays for the calls. On a public instance, that means the
  operator pays; many operators will leave Smart Import off there and rely on
  self-hosting, where each user runs their own instance with their own key.

## Limitations

- Web-only for now (no CLI subcommand).
- The whole request must finish within the service's request timeout, so very
  large PDFs may not complete on the hosted instance.
- Experimental — quality depends on the provider. Please report bad conversions.
