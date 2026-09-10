#!/usr/bin/env bash
# Assemble and sign the APT repository GitHub Pages serves: one suite, trixie,
# with one component, main, for amd64 and arm64.
#
#   ./scripts/apt-repo-build.sh <debs-dir> <output-dir> <repo-url>
#
#   <debs-dir>    Searched recursively for .debs, normally those of the most recent
#                 few sway-* releases. Every version found is indexed, so apt
#                 installs the newest and the older ones stay installable. The
#                 same file twice is kept once; the same name with different
#                 content is an error.
#   <output-dir>  Created; must not exist or be empty.
#   <repo-url>    The public base URL, shown on index.html.
#
# The key that signs is the one in packaging/apt/pubkey.asc, and its secret half
# must already be in the GnuPG keyring ($GNUPGHOME): the script signs with
# nothing else.
#
# Needs dpkg-dev, apt-utils and gnupg.
set -euo pipefail

[[ $# -eq 3 ]] || { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//' >&2; exit 2; }
debs_dir="$(cd "$1" && pwd)"
out="$2"
repo_url="${3%/}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pubkey="${here}/packaging/apt/pubkey.asc"
suite=trixie
component=main
arches=(amd64 arm64)
name=wlshare

die() { echo "ERROR: $*" >&2; exit 1; }

if [[ -e ${out} ]] && [[ -n $(ls -A "${out}") ]]; then
	die "output directory exists and is not empty: ${out}"
fi
mkdir -p "${out}"
out="$(cd "${out}" && pwd)"

fpr="$(gpg --batch --with-colons --import-options show-only --import "${pubkey}" |
	awk -F: '$1 == "fpr" { print $10; exit }')"
[[ -n ${fpr} ]] || die "no key fingerprint in ${pubkey}"
gpg --batch --list-secret-keys "${fpr}" >/dev/null 2>&1 ||
	die "the secret key for ${fpr} (packaging/apt/pubkey.asc) is not in the GnuPG keyring; import keys/apt-signing-key.private.asc, or in Actions set the GPG_PRIVATE_KEY secret"
echo ">>> signing key ${fpr}"

cd "${out}"

# Pool: pool/<component>/<prefix>/<package>/<package>_<version>_<arch>.deb
count=0
while IFS= read -r -d '' deb; do
	pkg="$(dpkg-deb -f "${deb}" Package)"
	ver="$(dpkg-deb -f "${deb}" Version)"
	arch="$(dpkg-deb -f "${deb}" Architecture)"
	[[ ${ver} == *"~${suite}" ]] || die "$(basename "${deb}"): version ${ver} is not a ${suite} build"
	if [[ ${pkg} == lib?* ]]; then prefix="${pkg:0:4}"; else prefix="${pkg:0:1}"; fi
	dest="pool/${component}/${prefix}/${pkg}/${pkg}_${ver#*:}_${arch}.deb"
	if [[ -e ${dest} ]]; then
		cmp -s "${deb}" "${dest}" || die "${dest} is already in the pool with different content (from ${deb})"
		continue
	fi
	mkdir -p "$(dirname "${dest}")"
	cp "${deb}" "${dest}"
	echo "  ${dest}"
	count=$((count + 1))
done < <(find "${debs_dir}" -type f -name '*.deb' -print0 | sort -z)
[[ ${count} -gt 0 ]] || die "no .deb files under ${debs_dir}"

gpg --batch --yes --dearmor < "${pubkey}" > "${name}.gpg"
cp "${pubkey}" "${name}.asc"
touch .nojekyll

for arch in "${arches[@]}"; do
	dir="dists/${suite}/${component}/binary-${arch}"
	mkdir -p "${dir}"
	dpkg-scanpackages --multiversion --arch "${arch}" "pool/${component}" > "${dir}/Packages"
	grep -q '^Package:' "${dir}/Packages" || die "no ${arch} packages"
	gzip -9n < "${dir}/Packages" > "${dir}/Packages.gz"
	printf 'Archive: %s\nOrigin: %s\nLabel: %s\nComponent: %s\nArchitecture: %s\n' \
		"${suite}" "${name}" "${name}" "${component}" "${arch}" > "${dir}/Release"
done

conf="$(mktemp)"
cat > "${conf}" <<CONF
APT::FTPArchive::Release {
  Origin "${name}";
  Label "${name}";
  Suite "${suite}";
  Codename "${suite}";
  Architectures "${arches[*]}";
  Components "${component}";
  Description "Sway and wlroots rebuilt for Debian ${suite} - ${repo_url}";
  Acquire-By-Hash "yes";
};
CONF
apt-ftparchive -c "${conf}" release "dists/${suite}" > "${conf}.release"
mv "${conf}.release" "dists/${suite}/Release"
rm -f "${conf}"

# by-hash copies of every index: GitHub Pages' CDN can serve a stale Packages
# beside a fresh InRelease mid-deploy, and apt fetching by hash never sees that.
for algo in SHA256 SHA512; do
	awk -v a="${algo}:" '$0 == a { on = 1; next } /^[A-Za-z0-9-]+:/ { on = 0 } on { print $1, $3 }' "dists/${suite}/Release" |
		while read -r hash rel; do
			src="dists/${suite}/${rel}"
			mkdir -p "$(dirname "${src}")/by-hash/${algo}"
			cp "${src}" "$(dirname "${src}")/by-hash/${algo}/${hash}"
		done
done

gpg --batch --yes --local-user "${fpr}" --digest-algo SHA512 \
	--clearsign --output "dists/${suite}/InRelease" "dists/${suite}/Release"
gpg --batch --yes --local-user "${fpr}" --digest-algo SHA512 \
	--armor --detach-sign --output "dists/${suite}/Release.gpg" "dists/${suite}/Release"

rows="$(for arch in "${arches[@]}"; do
	awk '/^Package:/ { p = $2 } /^Version:/ { v = $2 } /^Architecture:/ { a = $2 }
		/^$/ { print p "\t" v "\t" a }' "dists/${suite}/${component}/binary-${arch}/Packages"
done | sort -t "$(printf '\t')" -k1,1 -k2,2Vr -k3,3 |
	awk -F'\t' '{ printf "<tr><td>%s</td><td>%s</td><td>%s</td></tr>\n", $1, $2, $3 }')"

cat > index.html <<HTML
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>wlshare APT repository</title>
<style>
  body { font-family: system-ui, sans-serif; max-width: 60em; margin: 2em auto; padding: 0 1em; line-height: 1.5; }
  pre { background: #f4f4f4; padding: 1em; overflow-x: auto; }
  table { border-collapse: collapse; }
  td, th { border: 1px solid #ccc; padding: .2em .6em; text-align: left; font-family: ui-monospace, monospace; }
</style>
</head>
<body>
<h1>wlshare APT repository</h1>
<p>Sway and wlroots rebuilt for Debian ${suite} from Debian's own packaging, with
<a href="https://github.com/andrewtheguy/wlshare/tree/main/packaging/apt/patches">wlshare's patch series</a>
on top. It exists for its maintainer's machines: packages may change or disappear
without notice.</p>
<pre>sudo mkdir -p /etc/apt/keyrings
sudo curl -fsSL -o /etc/apt/keyrings/${name}.gpg ${repo_url}/${name}.gpg
sudo tee /etc/apt/sources.list.d/${name}.sources &lt;&lt;'EOF'
Types: deb
URIs: ${repo_url}
Suites: ${suite}
Components: ${component}
Signed-By: /etc/apt/keyrings/${name}.gpg
EOF
sudo apt update
sudo apt install sway</pre>
<p>Signing key <a href="${name}.gpg">${name}.gpg</a> (<a href="${name}.asc">armored</a>),
fingerprint <code>${fpr}</code>.</p>
<table>
<tr><th>Package</th><th>Version</th><th>Architecture</th></tr>
${rows}
</table>
<p>Source: <a href="https://github.com/andrewtheguy/wlshare">github.com/andrewtheguy/wlshare</a>.
Generated $(date -u +'%Y-%m-%d %H:%M UTC').</p>
</body>
</html>
HTML

echo ">>> assembled ${out}"
"${here}/scripts/apt-repo-verify.sh" "${out}"
