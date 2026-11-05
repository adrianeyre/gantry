# The Gantry image: one binary, the frontend embedded, the data directory a
# volume. Built for linux/amd64 and linux/arm64 (PRD §11).
#
# Rust is cross-compiled on the build machine's own architecture rather than
# under QEMU, which is many times slower. Only the last stage is per-platform.
#
#   docker buildx build --platform linux/amd64,linux/arm64 .
#   docker buildx build --platform linux/amd64,linux/arm64 --target binaries --output type=local,dest=dist .
#
# The second form exports just the binaries, for the Linux release archives.

# Kept in step with rust-toolchain.toml and the root package.json.
ARG RUST_VERSION=1.99.1
ARG NODE_VERSION=26
ARG PNPM_VERSION=12.16.1

# --- The frontend. Plain JavaScript, so it is built once for every platform.
FROM --platform=$BUILDPLATFORM node:${NODE_VERSION}-bookworm-slim AS web
ARG PNPM_VERSION
# The footer shows the release's version; left empty, vite.config.ts reads
# the workspace version from Cargo.toml.
ARG GANTRY_VERSION=
ENV GANTRY_VERSION=${GANTRY_VERSION}
# Node stopped shipping corepack as of 25, so pnpm is installed directly.
RUN npm install -g pnpm@${PNPM_VERSION} && rm -rf /root/.npm
WORKDIR /src
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY web/package.json web/
RUN pnpm install --frozen-lockfile --filter @gantry/web
COPY web/ web/
# vite.config.ts reads the workspace version from it.
COPY Cargo.toml ./
RUN pnpm --filter @gantry/web build

# --- The binary, cross-compiled for TARGETARCH. Bookworm, so the binary needs
# no newer glibc than the distroless runtime below has.
FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim-bookworm AS build
ARG RUST_VERSION
ARG TARGETARCH
# The release's version, compiled into the binary (`gantry --version`). Left
# empty, the workspace version is used.
ARG GANTRY_VERSION=
ENV GANTRY_VERSION=${GANTRY_VERSION}
# The image's toolchain, not rust-toolchain.toml's list of components:
# clippy and rustfmt are not needed to build.
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}
# OpenH264 and SQLite are C and C++, so each target needs a C/C++ compiler.
# nasm gives OpenH264 its x86 assembly; without it the decoder still builds.
RUN apt-get update && apt-get install -y --no-install-recommends \
    g++ nasm pkg-config \
    g++-aarch64-linux-gnu libc6-dev-arm64-cross \
    && rm -rf /var/lib/apt/lists/*
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-gnu ;; \
      arm64) echo aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported TARGETARCH: $TARGETARCH" >&2; exit 1 ;; \
    esac > /rust-target \
    && rustup target add "$(cat /rust-target)"
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    # web/dist is copied in below. If it were missing, the build would fail
    # loudly rather than embed the placeholder page.
    GANTRY_SKIP_WEB_BUILD=1
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ crates/
COPY --from=web /src/web/dist web/dist
RUN test -f web/dist/index.html
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,id=gantry-target-${TARGETARCH} \
    target="$(cat /rust-target)" \
    && cargo build --release --locked -p gantry --target "$target" \
    && mkdir -p /out \
    && cp "target/$target/release/gantry" /out/gantry
# The data directory, made here so the runtime image has it owned by the
# non-root user. A new named volume copies this ownership, so it is writable.
RUN mkdir -p /data

# --- Just the binaries, for `--target binaries --output type=local`.
FROM scratch AS binaries
COPY --from=build /out/gantry /gantry

# --- The runtime image. Distroless "cc" has glibc and libstdc++ (the decoder
# is C++) and nothing else: no shell, no package manager.
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
LABEL org.opencontainers.image.title="Gantry" \
      org.opencontainers.image.description="Monitor and control a Fleet of 3D printers from one screen." \
      org.opencontainers.image.source="https://github.com/adrianeyre/gantry" \
      org.opencontainers.image.licenses="ISC"
COPY --from=build /out/gantry /usr/local/bin/gantry
COPY --from=build --chown=65532:65532 /data /data
ENV GANTRY_BIND=0.0.0.0:8080 \
    GANTRY_DATA_DIR=/data \
    GANTRY_LOG_FORMAT=json
# The distroless "nonroot" user. Named numerically so Kubernetes and
# `docker inspect` can tell it is not root.
USER 65532:65532
WORKDIR /data
VOLUME /data
EXPOSE 8080
# No HEALTHCHECK: the image has no shell or curl to run one. Point the
# orchestrator at GET /healthz instead.
ENTRYPOINT ["/usr/local/bin/gantry"]
CMD ["serve"]
