# check=skip=InvalidDefaultArgInFrom
ARG BASE_IMAGE=debian:trixie
FROM ${BASE_IMAGE} AS build

ENV DEBIAN_FRONTEND=noninteractive
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

# devscripts brings dch and mk-build-deps, which needs equivs; each source
# package's own Build-Depends are installed from the suite by mk-build-deps.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl devscripts dpkg-dev equivs fakeroot

ARG BUILD_VERSION
ARG BUILD_REVISION
COPY packaging/apt /build/packaging/apt
COPY docker/sway-build.sh /build/docker/sway-build.sh
RUN BUILD_VERSION="${BUILD_VERSION}" BUILD_REVISION="${BUILD_REVISION}" /build/docker/sway-build.sh

FROM scratch
COPY --from=build /out /
