# check=skip=InvalidDefaultArgInFrom
ARG BASE_IMAGE=debian:trixie
FROM ${BASE_IMAGE} AS build

ENV DEBIAN_FRONTEND=noninteractive
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# The daemon links libxkbcommon, libpam, libpipewire, libavcodec (the
# camera's H.264 decoder), and libx264 and libva (the H.264 encoders of the
# video and the tiles' regions) — the Wayland protocol is wayland-client's own Rust,
# with no libwayland; cargo-deb reads the resulting .so
# dependencies into the package's Depends. libopus, the audio extension's
# Opus encoder, is a static archive the build downloads, its FLAC encoder
# plain Rust; jemalloc, the
# allocator, is built from source with build-essential. libclang-dev is a build-time need
# with nothing to link: pipewire-sys, libspa-sys, ffmpeg-sys-next, x264-sys and
# the daemon's build.rs, libva's, generate their bindings with bindgen, which loads libclang, and without it a clean
# build fails with "Unable to find libclang".
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl pkg-config \
        libxkbcommon-dev libpam0g-dev \
        libpipewire-0.3-dev libspa-0.2-dev libavcodec-dev libx264-dev libva-dev libclang-dev dpkg-dev && \
    rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable && \
    cargo install cargo-deb --locked

WORKDIR /build
COPY . /build

# jemalloc takes its page size from the build host, and a 4K build refuses to
# start on the 16K and 64K kernels arm64 boards ship (Raspberry Pi 5). 64K pages
# run on all three.
#
# `wlshare --version` names the release and the protocol version, wlshare-proto's
# major version, and is checked against both.
#
# The package version is the crate's, as cargo-deb writes it (<version>-1, a
# prerelease's hyphen a tilde, so that 0.0.63~alpha.1 sorts before 0.0.63); the
# distribution the package was built for is in the file name only, by its suite
# (trixie, or Ubuntu 26.04's resolute), so the release asset is
# wlshare-<suite>-<arch>.deb whatever the version. cargo-deb's $auto names the
# suite's own sonames (libavcodec61 on trixie, libavcodec62 on resolute), so a
# package installs only on the suite it was built on.
RUN set -eux; \
    version="$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"name":"wlshare","version":"\([^"]*\)".*/\1/p' | head -n 1)"; \
    protocol="$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"name":"wlshare-proto","version":"\([0-9]*\)\.0\.0".*/\1/p' | head -n 1)"; \
    arch="$(dpkg --print-architecture)"; \
    suite="$(. /etc/os-release && echo "${VERSION_CODENAME}")"; \
    if [ "${arch}" = arm64 ]; then export JEMALLOC_SYS_WITH_LG_PAGE=16; fi; \
    cargo build --release -p wlshare; \
    [ "$(target/release/wlshare --version)" = "wlshare ${version} (wlshare protocol ${protocol})" ]; \
    mkdir -p /out; \
    cargo deb -p wlshare --no-build -o "/out/wlshare-${suite}-${arch}.deb"; \
    [ "$(dpkg-deb --field "/out/wlshare-${suite}-${arch}.deb" Version)" = "${version/-/\~}-1" ]; \
    cd /out && sha256sum ./*.deb | sed 's#  \./#  #' > SHA256SUMS && cat SHA256SUMS

FROM scratch
COPY --from=build /out /
