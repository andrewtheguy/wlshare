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

# The package version is the crate's, as cargo-deb writes it (<version>-1); the
# distribution the package was built for is in the file name only, so the
# release asset is swayrx-trixie-<arch>.deb whatever the version.
RUN set -eux; \
    version="$(cargo metadata --no-deps --format-version 1 | sed -n 's/.*"name":"swayrx","version":"\([^"]*\)".*/\1/p' | head -n 1)"; \
    arch="$(dpkg --print-architecture)"; \
    cargo build --release -p swayrx; \
    [ "$(target/release/swayrx --version)" = "swayrx ${version}" ]; \
    mkdir -p /out; \
    cargo deb -p swayrx --no-build -o "/out/swayrx-trixie-${arch}.deb"; \
    [ "$(dpkg-deb --field "/out/swayrx-trixie-${arch}.deb" Version)" = "${version}-1" ]; \
    cd /out && sha256sum ./*.deb | sed 's#  \./#  #' > SHA256SUMS && cat SHA256SUMS

FROM scratch
COPY --from=build /out /
