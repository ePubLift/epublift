# Security policy

## Reporting a vulnerability

**Please do not open a public issue for a security problem.** Report it
privately through GitHub:
[**Report a vulnerability**](https://github.com/ePubLift/epublift/security/advisories/new)
(the *Security* tab of this repository). Only the maintainer can see the
report.

A useful report includes the smallest input that shows the problem (an EPUB,
PDF, Markdown file or `.eparc`, or the file inside it that matters), where you
ran it (the CLI with `epublift --version`, a self-hosted `epublift-web`, or the
hosted instance at <https://epublift.itpax.net>), what you did, and what
happened: the crash message, the memory or time used, or what was read, written
or sent.

## What counts

epublift reads files that other people made, and `epublift-web` does it for
anyone who can reach it. That makes these security problems, in the CLI, the
library and the web service alike:

- **a crash**: a panic, an abort or a stack overflow on any input;
- **resource exhaustion out of proportion to the input**: memory or time that
  a small file can drive far past what its size justifies — the web service's
  limits (50 MiB per upload, 120 s per request, a per-IP rate limit) are
  meant to hold against any input;
- **any file read other than the input, or written other than the output**;
- **network access other than the documented ones**: ISBN metadata lookups
  (Open Library, Google Books), the one-time OCR model download of the CLI's
  PDF OCR, and — only when its operator has configured an API key — Smart
  Import sending the uploaded PDF to that AI provider. Anything else would be
  a defect;
- **in the web service, one visitor's file or result reaching another**, an
  upload outliving its request, a result kept longer than its download window,
  or an operator's API key exposed to a visitor;
- **the release pipeline**: anything that could put code into a published
  binary, package or container image that is not in this repository.

A wrong result — a book converted badly, a validation finding that is wrong or
missing — is not a security problem. Please report it as an ordinary
[issue](https://github.com/ePubLift/epublift/issues), where it helps everyone
who meets it. Validation and repair come from the
[veripublica](https://github.com/veripublica) tools; a problem that lies in
[epubveri](https://github.com/veripublica/epubveri) or
[epubsana](https://github.com/veripublica/epubsana) themselves belongs there.

## How the web service handles uploads

So a report can be measured against what is promised:

- each upload is processed in a temporary directory that is deleted when the
  request ends; no upload is stored;
- a result waits in memory for its download for at most 5 minutes, then is
  dropped;
- uploads are limited to 50 MiB, requests to 120 seconds, and each client IP
  to a small burst of conversions.

## Supported versions

Only the **latest release** of each component (`cli-v*`, `web-v*`) receives
fixes. A fix ships as a new release, never as a patch to an old one. The
hosted instance runs the latest web release.

## What happens next

epublift has one maintainer, so these are aims, not guarantees:

- an acknowledgement within a week;
- a fix released **before** the details are public. The release's
  `CHANGELOG.md` entry then describes the problem under **Security**;
- credit to the reporter in that entry, if they want it.

## Verifying what you download

Every release is built and published by this repository's GitHub Actions
workflows.

- **Release binaries and packages.** Each archive, `.deb` and `.rpm` on a
  release has a `.sha256` file beside it:
  ```sh
  shasum -a 256 -c epublift-<version>-<target>.tar.gz.sha256
  ```
- **Container image.** `ghcr.io/epublift/epublift-web` is pushed only by the
  `docker.yml` workflow. Pin it by digest (`@sha256:…`) rather than by tag if
  you need to know exactly what you run.
