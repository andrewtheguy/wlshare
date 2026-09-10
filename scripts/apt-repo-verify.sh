#!/usr/bin/env bash
# Check an assembled APT repository the way apt will before it is deployed:
#   1. InRelease and Release.gpg verify against the served keyring, and
#      InRelease's payload is Release.
#   2. Every file Release lists has its size and SHA-256, and a by-hash copy.
#   3. Every Packages.gz decompresses to its Packages.
#   4. Every Packages stanza names a pool file with its Size and SHA-256.
# Any mismatch exits 1.
#
#   ./scripts/apt-repo-verify.sh <repo-root>
set -euo pipefail

[[ $# -eq 1 ]] || { echo "usage: $0 <repo-root>" >&2; exit 2; }
root="$(cd "$1" && pwd)"
suite="${root}/dists/trixie"
keyring="${root}/wlshare.gpg"

failures=0
fail() { echo "  MISMATCH: $*" >&2; failures=$((failures + 1)); }
sha256() { sha256sum "$1" | awk '{ print $1 }'; }
size() { stat -c %s "$1"; }

[[ -f ${suite}/Release ]] || { echo "ERROR: ${suite}/Release missing" >&2; exit 1; }

gpgv --quiet --keyring "${keyring}" "${suite}/InRelease" 2>/dev/null || fail "InRelease signature"
gpgv --quiet --keyring "${keyring}" "${suite}/Release.gpg" "${suite}/Release" 2>/dev/null || fail "Release.gpg signature"
gpgv --quiet --keyring "${keyring}" --output - "${suite}/InRelease" 2>/dev/null |
	cmp -s - "${suite}/Release" || fail "InRelease payload differs from Release"

while read -r hash bytes rel; do
	file="${suite}/${rel}"
	[[ -f ${file} ]] || { fail "Release lists ${rel}, which is missing"; continue; }
	[[ $(size "${file}") == "${bytes}" && $(sha256 "${file}") == "${hash}" ]] || fail "${rel} differs from Release"
	cmp -s "${file}" "$(dirname "${file}")/by-hash/SHA256/${hash}" || fail "${rel} has no by-hash copy"
done < <(awk '$0 == "SHA256:" { on = 1; next } /^[A-Za-z0-9-]+:/ { on = 0 } on { print $1, $2, $3 }' "${suite}/Release")

for packages in "${suite}"/*/binary-*/Packages; do
	gzip -dc "${packages}.gz" | cmp -s - "${packages}" || fail "${packages#"${root}/"}.gz differs from Packages"
	while read -r filename bytes hash; do
		file="${root}/${filename}"
		[[ -f ${file} ]] || { fail "${filename} missing from the pool"; continue; }
		[[ $(size "${file}") == "${bytes}" && $(sha256 "${file}") == "${hash}" ]] || fail "${filename} differs from its index"
	done < <(awk '/^Filename:/ { f = $2 } /^Size:/ { s = $2 } /^SHA256:/ { h = $2 }
		/^$/ { print f, s, h }' "${packages}")
done

[[ ${failures} -eq 0 ]] || { echo ">>> ${failures} mismatch(es): not publishable" >&2; exit 1; }
echo ">>> repository verified"
