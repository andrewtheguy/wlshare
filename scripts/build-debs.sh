#!/usr/bin/env bash
# Build the swayrx .deb for Debian trixie with Docker buildx, one architecture per
# call or amd64 and arm64 by default, into dist/<arch>/swayrx-trixie-<arch>.deb
# beside its SHA256SUMS.
#
#   ./scripts/build-debs.sh [arch ...]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
arches=("$@")
[[ ${#arches[@]} -gt 0 ]] || arches=(arm64 amd64)

for arch in "${arches[@]}"; do
	dest="${here}/dist/${arch}"
	rm -rf "${dest}"
	mkdir -p "${dest}"
	docker buildx build \
		--platform "linux/${arch}" \
		--file "${here}/docker/Dockerfile" \
		--output "type=local,dest=${dest}" \
		"${here}"
	echo "== ${arch}"
	cat "${dest}/SHA256SUMS"
done
