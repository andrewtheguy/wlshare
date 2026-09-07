# check=skip=InvalidDefaultArgInFrom
ARG BASE_IMAGE=debian:trixie
FROM ${BASE_IMAGE} AS build

ENV DEBIAN_FRONTEND=noninteractive
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# The daemon links libwayland-client and libxkbcommon; cargo-deb reads the
# resulting .so dependencies into the package's Depends.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl pkg-config \
        libwayland-dev libxkbcommon-dev dpkg-dev && \
    rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable && \
    cargo install cargo-deb --locked

WORKDIR /build
COPY . /build

# SWAYRX_BUILD is <YYYYMMDD>-<N>, the tail of the release tag trixie-<YYYYMMDD>-<N>;
# the package version is <crate version>~trixie<YYYYMMDD>.<N>, so a later build
# always sorts newer for dpkg.
ARG SWAYRX_BUILD
RUN set -eux; \
    version="$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"name":"swayrx","version":"\([^"]*\)".*/\1/p' | head -n 1)"; \
    build="${SWAYRX_BUILD:-$(date -u +%Y%m%d)-0}"; \
    deb_version="${version}~trixie$(echo "$build" | tr '-' '.')"; \
    cargo build --release -p swayrx; \
    cargo deb -p swayrx --no-build --deb-version "$deb_version" -o /out/; \
    cd /out && sha256sum ./*.deb | sed 's#  \./#  #' > SHA256SUMS && cat SHA256SUMS

FROM scratch
COPY --from=build /out /
