#!/usr/bin/env bash
# Build the swayrx .deb for Debian trixie on amd64 and arm64 with Docker buildx,
# into output/trixie/<arch>/.
#
#   SWAYRX_BUILD=<YYYYMMDD>-<N> ./scripts/build-debs.sh [arch ...]
#
# default arches: arm64 amd64; default build: today's date, revision 0, which
# the release workflow never uses, so a local build never collides with one.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
arches=("$@")
[[ ${#arches[@]} -gt 0 ]] || arches=(arm64 amd64)
build="${SWAYRX_BUILD:-$(date -u +%Y%m%d)-0}"

for arch in "${arches[@]}"; do
	dest="${here}/output/trixie/${arch}"
	rm -rf "${dest}"
	mkdir -p "${dest}"
	docker buildx build \
		--platform "linux/${arch}" \
		--file "${here}/docker/Dockerfile" \
		--build-arg "SWAYRX_BUILD=${build}" \
		--output "type=local,dest=${dest}" \
		"${here}"
	echo "== ${arch}"
	cat "${dest}/SHA256SUMS"
done
