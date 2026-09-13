#!/usr/bin/env bash
# Build the wlroots, Sway and labwc packages for Debian trixie that the APT repository
# publishes, with Docker buildx, one architecture per call or arm64 and amd64 by
# default, into dist/sway/<arch>/ beside their SHA256SUMS.
#
#   [BUILD_VERSION=<YYYYMMDD>] [BUILD_REVISION=<N>] ./scripts/build-sway-debs.sh [arch ...]
#
# The packages are versioned <upstream>+<BUILD_VERSION>-<BUILD_REVISION>~trixie.
# The release workflow passes its release's date and revision; a local build
# defaults to today and revision 0, below every build released that day.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
build_version="${BUILD_VERSION:-$(date -u +%Y%m%d)}"
build_revision="${BUILD_REVISION:-0}"
arches=("$@")
[[ ${#arches[@]} -gt 0 ]] || arches=(arm64 amd64)

for arch in "${arches[@]}"; do
	dest="${here}/dist/sway/${arch}"
	rm -rf "${dest}"
	mkdir -p "${dest}"
	docker buildx build \
		--platform "linux/${arch}" \
		--pull --no-cache \
		--build-arg "BUILD_VERSION=${build_version}" \
		--build-arg "BUILD_REVISION=${build_revision}" \
		--file "${here}/docker/sway.Dockerfile" \
		--output "type=local,dest=${dest}" \
		"${here}"
	echo "== ${arch}"
	cat "${dest}/SHA256SUMS"
done
