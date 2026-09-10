#!/usr/bin/env bash
# Runs inside docker/sway.Dockerfile: rebuilds the Debian wlroots and Sway source
# packages pinned in packaging/apt/sources.env for this container's Debian
# release, each with its repository patch series applied after Debian's, and
# leaves the architecture-dependent .debs in /out beside their SHA256SUMS.
#
# Every package is versioned <upstream>+<BUILD_VERSION>-<BUILD_REVISION>~<suite>,
# which sorts above both the suite's own package and Debian's build of the same
# upstream release, and above every earlier build of this repository.
set -euo pipefail

: "${BUILD_VERSION:?}" "${BUILD_REVISION:?}"
[[ ${BUILD_VERSION} =~ ^[0-9]{8}$ ]] || { echo "BUILD_VERSION must be YYYYMMDD, got '${BUILD_VERSION}'" >&2; exit 1; }
[[ ${BUILD_REVISION} =~ ^[0-9]+$ ]] || { echo "BUILD_REVISION must be a number, got '${BUILD_REVISION}'" >&2; exit 1; }

here=/build
# shellcheck source=/dev/null
source "${here}/packaging/apt/sources.env"
# shellcheck source=/dev/null
suite="$(. /etc/os-release && echo "${VERSION_CODENAME}")"
jobs="$(nproc)"
export DEBEMAIL="wlshare@users.noreply.github.com" DEBFULLNAME="wlshare"
export DEB_BUILD_OPTIONS="noautodbgsym parallel=${jobs}"

# fetch <dsc-url> <dsc-sha256> <dir>: download the .dsc and the files it lists
# from beside it, and unpack it into <dir>. dpkg-source refuses a tarball whose
# SHA-256 differs from the .dsc's.
fetch() {
	local url=$1 sha=$2 dir=$3 base dsc
	base="${url%/*}"
	dsc="${url##*/}"
	curl -fsSLO "${url}"
	echo "${sha}  ${dsc}" | sha256sum --check --strict
	awk '/^Checksums-Sha256:/ { on = 1; next } /^[^ ]/ { on = 0 } on { print $3 }' "${dsc}" |
		while read -r file; do curl -fsSLO "${base}/${file}"; done
	dpkg-source --require-strong-checksums -x "${dsc}" "${dir}"
}

# build <dir> <patches>: append the patch series to Debian's, bump the version,
# install the build dependencies and build the architecture-dependent packages.
build() {
	local dir=$1 patches=$2 version upstream
	(
		cd "${dir}"
		sed -e '/^#/d' -e '/^[[:space:]]*$/d' "${patches}/series" | while read -r patch; do
			install -D -m 644 "${patches}/${patch}" "debian/patches/wlshare/${patch}"
			echo "wlshare/${patch}" >> debian/patches/series
		done
		version="$(dpkg-parsechangelog -S Version)"
		upstream="${version%-*}"
		dch --newversion "${upstream}+${BUILD_VERSION}-${BUILD_REVISION}~${suite}" \
			--distribution "${suite}" --force-distribution \
			"Rebuild Debian ${version} for ${suite} with the wlshare patch series."
		mk-build-deps --install --remove \
			--tool 'apt-get -y --no-install-recommends' debian/control
		dpkg-buildpackage -B -us -uc
	)
}

mkdir -p /src /out
cd /src

fetch "${WLROOTS_DSC_URL}" "${WLROOTS_DSC_SHA256}" wlroots
build wlroots "${here}/packaging/apt/patches/wlroots"
# Sway builds against the wlroots just built, not the suite's.
apt-get install -y --no-install-recommends ./libwlroots-0.19_*.deb ./libwlroots-0.19-dev_*.deb

fetch "${SWAY_DSC_URL}" "${SWAY_DSC_SHA256}" sway
build sway "${here}/packaging/apt/patches/sway"

cp ./*.deb /out/
cd /out
sha256sum ./*.deb | sed 's#  \./#  #' > SHA256SUMS
cat SHA256SUMS
