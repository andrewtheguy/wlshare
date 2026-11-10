#!/usr/bin/env bash
# Rewrite a .deb in place so that its Depends names each package once, at the
# highest floor given for it. cargo-deb's $auto floors libva2 and libva-drm2 at
# the oldest version that exports every function the daemon calls, while
# Cargo.toml states 2.1.0, the oldest libva the daemon's hand-written bindings
# hold for. Both reach Depends, and only the higher floor matters.
#
#   docker/merge-depends.sh PACKAGE.deb
set -euo pipefail

deb="$(realpath "$1")"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

# A package and an optional ">=" floor: all that cargo-deb's $auto and
# Cargo.toml's depends write. Anything else (an alternative, another relation)
# stops the build rather than being merged wrongly.
entry_re='^([a-z0-9][a-z0-9+.:-]*)( \(>= ([^)]+)\))?$'
names=()
declare -A floor
while IFS= read -r entry; do
	[[ ${entry} =~ ${entry_re} ]] || { echo "unexpected Depends entry '${entry}'" >&2; exit 1; }
	name="${BASH_REMATCH[1]}"
	version="${BASH_REMATCH[3]}"
	if [[ ! -v "floor[${name}]" ]]; then
		names+=("${name}")
		floor["${name}"]="${version}"
	elif [[ -n ${version} ]] && { [[ -z ${floor[${name}]} ]] ||
		dpkg --compare-versions "${version}" gt "${floor[${name}]}"; }; then
		floor["${name}"]="${version}"
	fi
done < <(dpkg-deb --field "${deb}" Depends | sed 's/, */\n/g')

depends=""
for name in "${names[@]}"; do
	depends+="${depends:+, }${name}${floor[${name}]:+ (>= ${floor[${name}]})}"
done

# Only control.tar is rewritten: its members' order, owner and time are kept,
# and data.tar, the payload, stays byte for byte as cargo-deb wrote it.
control="$(ar t "${deb}" | grep '^control\.tar')"
[[ ${control} == control.tar.xz ]] || { echo "unexpected control member '${control}'" >&2; exit 1; }
mkdir "${work}/control"
ar p "${deb}" control.tar.xz | xz -dc | tar -x -C "${work}/control"
mapfile -t members < <(ar p "${deb}" control.tar.xz | xz -dc | tar -t)
mtime="$(stat -c %Y "${work}/control/control")"
# Depends as one line, replacing the field and any continuation lines it had.
awk -v depends="Depends: ${depends}" '
	/^Depends:/ { print depends; skip = 1; next }
	skip && /^[ \t]/ { next }
	{ skip = 0; print }
' "${work}/control/control" > "${work}/control.new"
mv "${work}/control.new" "${work}/control/control"
touch -d "@${mtime}" "${work}/control/control"
tar -c --format=gnu --owner=0 --group=0 --numeric-owner --no-recursion \
	-C "${work}/control" "${members[@]}" | xz -c > "${work}/control.tar.xz"
(cd "${work}" && ar rD "${deb}" control.tar.xz)
