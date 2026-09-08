# Repository instructions

- Strict no backward-compatibility or legacy paths no matter what.

- Two crates: `swayrx-rfb` (protocol, platform-independent, tested by a bare
  `cargo test`) and `swayrx` (the daemon, Linux + Wayland only). After Rust
  changes run `cargo test` and `cargo clippy --all-targets -- -D warnings`.
  There is no cross-target check to run: nothing here is architecture-specific,
  and the release builds each architecture in Docker on its own native runner.
  Linking the daemon's tests needs `libpam0g-dev`.
- Every protocol byte comes from `swayrx-rfb`; the daemon never writes one itself.
  Every encoder gets an independent decoder in its tests.
- Build and run the daemon on a Linux host with a sway session; packages are built
  only in Docker by `scripts/build-debs.sh`, never on the host.
- Do not run `cargo fmt`. Use `anyhow` for application errors and `thiserror` for
  typed protocol errors.
- Design and wire details live in `docs/architecture.md`, not here.
