# syntax=docker/dockerfile:1

# ---- chef ----------------------------------------------------------------
# Alpine's Rust toolchain targets musl natively, so the resulting binary is
# fully static — it runs on a bare Alpine (or scratch) without glibc.
# cargo-chef splits the build into a dependency layer (rebuilt only when
# Cargo.toml / Cargo.lock change) and the project's own crates.
FROM rust:1-alpine AS chef

# musl-dev provides the C runtime headers the linker needs. The project itself
# is pure-Rust (no C deps), so nothing else is required.
RUN apk add --no-cache musl-dev
RUN cargo install cargo-chef --locked --version 0.1.78

# Strip symbols during the build (smaller binary, no separate strip step).
# Set before `cook` so the dependencies are built with the same flags.
ENV RUSTFLAGS="-C strip=symbols"

WORKDIR /app

# ---- planner -------------------------------------------------------------
# The recipe is the dependency graph alone: a source change leaves it as it
# was, so the `cook` layer below stays cached.
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY epublift-web ./epublift-web
RUN cargo chef prepare --recipe-path recipe.json

# ---- builder -------------------------------------------------------------
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked -p epublift-web --recipe-path recipe.json

# Only the sources needed to build the web binary (see .dockerignore).
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY epublift-web ./epublift-web

# Git commit the image is built from (the build context has no `.git`, so the
# web crate's build.rs reads it from this env). Passed by the release workflow;
# empty for a plain local build (the footer then shows just the version).
# Declared after `cook`, so a new commit does not invalidate the dependencies.
ARG GIT_SHA=""
ENV GIT_SHA=$GIT_SHA

RUN cargo build --release --locked -p epublift-web

# ---- runtime -------------------------------------------------------------
FROM alpine:3.24

# Run as an unprivileged user.
RUN adduser -D -u 10001 epublift

COPY --from=builder /app/target/release/epublift-web /usr/local/bin/epublift-web

USER epublift
EXPOSE 8080

# The service writes only to the system temp dir (mount it as tmpfs at runtime).
ENTRYPOINT ["/usr/local/bin/epublift-web"]
