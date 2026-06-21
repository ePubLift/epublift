# syntax=docker/dockerfile:1

# ---- builder -------------------------------------------------------------
# Alpine's Rust toolchain targets musl natively, so the resulting binary is
# fully static — it runs on a bare Alpine (or scratch) without glibc.
FROM rust:1-alpine AS builder

# musl-dev provides the C runtime headers the linker needs. The project itself
# is pure-Rust (no C deps), so nothing else is required.
RUN apk add --no-cache musl-dev

# Strip symbols during the build (smaller binary, no separate strip step).
ENV RUSTFLAGS="-C strip=symbols"

WORKDIR /app

# Git commit the image is built from (the build context has no `.git`, so the
# web crate's build.rs reads it from this env). Passed by the release workflow;
# empty for a plain local build (the footer then shows just the version).
ARG GIT_SHA=""
ENV GIT_SHA=$GIT_SHA

# Only the sources needed to build the web binary (see .dockerignore).
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY epublift-web ./epublift-web

RUN cargo build --release --locked -p epublift-web

# ---- runtime -------------------------------------------------------------
FROM alpine:3.20

# Run as an unprivileged user.
RUN adduser -D -u 10001 epublift

COPY --from=builder /app/target/release/epublift-web /usr/local/bin/epublift-web

USER epublift
EXPOSE 8080

# The service writes only to the system temp dir (mount it as tmpfs at runtime).
ENTRYPOINT ["/usr/local/bin/epublift-web"]
