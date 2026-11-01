#!/usr/bin/env bash
# Build the wlshare .deb with Docker buildx for each suite it is released for,
# Debian trixie and Ubuntu 26.04 (resolute), one architecture per call or amd64
# and arm64 by default, into dist/<suite>/<arch>/wlshare-<suite>-<arch>.deb
# beside its SHA256SUMS. SUITES picks the suites, both by default.
#
#   [SUITES="trixie resolute"] ./scripts/build-debs.sh [arch ...]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
read -r -a suites <<< "${SUITES:-trixie resolute}"
arches=("$@")
[[ ${#arches[@]} -gt 0 ]] || arches=(arm64 amd64)

for suite in "${suites[@]}"; do
	case "${suite}" in
		trixie) image=debian:trixie ;;
		resolute) image=ubuntu:26.04 ;;
		*) echo "unknown suite '${suite}': trixie or resolute" >&2; exit 1 ;;
	esac
	for arch in "${arches[@]}"; do
		dest="${here}/dist/${suite}/${arch}"
		rm -rf "${dest}"
		mkdir -p "${dest}"
		docker buildx build \
			--platform "linux/${arch}" \
			--build-arg "BASE_IMAGE=${image}" \
			--file "${here}/docker/Dockerfile" \
			--output "type=local,dest=${dest}" \
			"${here}"
		echo "== ${suite} ${arch}"
		cat "${dest}/SHA256SUMS"
	done
done
