# ePubLift

ePubLift turns an old EPUB into a current one. It is for people who keep a library of e-books, and for people who publish them. It checks a book against the specification, repairs what can be safely repaired, fixes the metadata and upgrades the book to EPUB 3.3. It can also make a KEPUB for a Kobo, shrink a library into lossless archives, and import a PDF or Markdown into a reflowable EPUB. PDF and Markdown import are still experimental.

The easiest way to use it is [the web page](https://epublift.itpax.net). Validate and Repair run in your browser, so your book is never uploaded. The other modes do upload it. It is processed in a temporary folder and deleted, and you can [run the service yourself](docs/web-service.md) with Docker if you prefer:

```sh
docker run -d -p 127.0.0.1:8080:8080 ghcr.io/epublift/epublift-web:latest
```

For the command line, download a binary or a `.deb` or `.rpm` from the [latest release](https://github.com/ePubLift/epublift/releases/latest). The macOS and Windows binaries aren't signed yet, so your system warns you the first time you run one. Then:

```sh
epublift check -i book.epub
epublift repair -i book.epub
epublift -i book_repaired.epub
```

The last command writes `book_repaired_v3.3.epub`. Your original is never touched.

Images stay as they are unless you ask otherwise, because a Kobo shows WebP images blank. With `--webp` they are re-encoded, and an image that wouldn't get smaller keeps its original form.

ePubLift doesn't remove DRM. For that, look at DeDRM or calibre.

The checks and repairs come from my other tools, [epubveri](https://github.com/veripublica/epubveri) and [epubsana](https://github.com/veripublica/epubsana). The Kobo conversion comes from [kepubverto](https://github.com/veripublica/kepubverto) and PDF reading from [paginaverto](https://github.com/veripublica/paginaverto). The guides are in [docs](docs/), and the options, exit codes, limits and build steps are in [AGENTS.md](https://github.com/ePubLift/epublift/blob/main/AGENTS.md).

The code is under the [AGPL-3.0](LICENSE) or later. If you run a modified version as a service, you have to share your changes with its users.
