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

# The OCR models Import PDF reads scans with (paginaverto's ocrs detection model
# and PaddlePaddle's PP-OCRv5 Latin recognition model, 10.5 MB together), baked
# in at the SHA-256 paginaverto publishes, so the service never downloads them
# and runs on a network with no route out. Change both together with paginaverto.
# ADD from a URL would write the files, and a folder it makes, readable by root
# only: the folder is made first and the files opened to all.
RUN mkdir -p /usr/local/share/epublift/ocr-models
ADD --chmod=644 --checksum=sha256:a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe \
    https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx \
    /usr/local/share/epublift/ocr-models/text-detection.onnx
ADD --chmod=644 --checksum=sha256:7888113072263cb471b93f66dd5e2ad70548dc526fa1ace760d0d973dd121498 \
    https://huggingface.co/PaddlePaddle/latin_PP-OCRv5_mobile_rec_onnx/resolve/89d3a50e2c27e2e7cceeab0e944c25c807d5db4f/inference.onnx \
    /usr/local/share/epublift/ocr-models/latin_PP-OCRv5_mobile_rec.onnx
ENV EPUBLIFT_OCR_MODELS=/usr/local/share/epublift/ocr-models

USER epublift
EXPOSE 8080

# The service writes only to the system temp dir (mount it as tmpfs at runtime).
ENTRYPOINT ["/usr/local/bin/epublift-web"]
