#!/usr/bin/env bash
# Install Sway and labwc from an assembled APT repository in a debian:trixie
# container, as a client would, and check that sway, labwc and libwlroots-0.19
# came from it. Runs on the host's architecture only.
#
#   ./scripts/apt-repo-smoke.sh <repo-root>
set -euo pipefail

[[ $# -eq 1 ]] || { echo "usage: $0 <repo-root>" >&2; exit 2; }
root="$(cd "$1" && pwd)"

docker run --rm --pull=always -v "${root}:/repo:ro" -e DEBIAN_FRONTEND=noninteractive \
	debian:trixie bash -euo pipefail -c '
		install -D -m 644 /repo/wlshare.gpg /etc/apt/keyrings/wlshare.gpg
		printf "Types: deb\nURIs: file:/repo\nSuites: trixie\nComponents: main\nSigned-By: /etc/apt/keyrings/wlshare.gpg\n" \
			> /etc/apt/sources.list.d/wlshare.sources
		apt-get update
		apt-get install -y --no-install-recommends sway labwc
		for pkg in sway labwc libwlroots-0.19; do
			ver="$(dpkg-query -W -f="\${Version}" "${pkg}")"
			compgen -G "/repo/pool/main/*/${pkg}/${pkg}_${ver#*:}_*.deb" >/dev/null ||
				{ echo "${pkg} ${ver} did not come from the repository" >&2; exit 1; }
			echo "${pkg} ${ver}: from the repository"
		done
		sway --version
		labwc --version
	'
