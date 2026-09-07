# Repository instructions

- Two crates: `swayrx-rfb` (protocol, platform-independent, tested by a bare
  `cargo test`) and `swayrx` (the daemon, Linux + Wayland only). After Rust
  changes run `cargo test`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo clippy -p swayrx --all-targets --target aarch64-unknown-linux-gnu -- -D warnings`
  (the daemon type-checks for Linux from any host once the target's std is installed).
- Every protocol byte comes from `swayrx-rfb`; the daemon never writes one itself.
  Every encoder gets an independent decoder in its tests.
- Build and run the daemon on a Linux host with a sway session; packages are built
  only in Docker by `scripts/build-debs.sh`, never on the host.
- Do not run `cargo fmt`. Use `anyhow` for application errors and `thiserror` for
  typed protocol errors.
- Design and wire details live in `docs/architecture.md`, not here.
