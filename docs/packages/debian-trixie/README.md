# Pinned Sway packages for Debian trixie

These Debian packages provide Sway 1.11 on trixie without adding a testing or
unstable APT source. wlroots 0.19 is the first release whose headless backend
keeps the cursor separate from the captured output. wlroots is pinned to 0.19.1
because that Debian build uses trixie's `libdisplay-info2`; starting with the
0.19.2 Debian package would also require importing `libdisplay-info3`.

The packages are unmodified Debian binaries mirrored from Debian Snapshot so
deployments do not depend on a package remaining in the rolling Debian pool.
`SHA256SUMS` beside each architecture's packages pins the exact bytes.
