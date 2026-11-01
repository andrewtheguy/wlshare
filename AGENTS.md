# Repository instructions

- Strict no backward-compatibility or legacy paths no matter what.

- Two crates: `wlshare` (the daemon, Linux + wlroots-based Wayland only),
  the package at the repository's root, whose RFB protocol is its own module
  (`src/rfb/`), not a crate; and under `crates/`, `wlshare-proto` (the
  private extensions' bytes), platform-independent and tested elsewhere with
  `cargo test -p wlshare-proto`. Both are default members: a bare
  `cargo test` tests the workspace. The daemon's one
  client is the zDesk gateway; there is no native client. `wlshare-proto` is
  the one crate anything outside this repository depends on: the gateway pins
  it by its `proto-v<N>` tag, behind its `wlshare` feature, and it
  depends on `thiserror` alone. Its version is the protocol version,
  `<N>.0.0`: any change to what it does bumps `PROTOCOL_VERSION` and the
  crate's version together, and the release that brings it tags `proto-v<N>`;
  a change to its comments or its tests alone bumps nothing, and the release
  checks that nothing else differs from the tag. Its tests are each module's
  in a file of its own, `src/<module>/tests.rs` (`src/tests.rs` for the
  crate's root), which the check leaves out by its path. Every private extension's bytes, both
  directions, are built and parsed there, never in the daemon or the gateway.
  Nothing on the wire says how a picture is coded: the dial is the daemon's
  (`QUALITY` in `src/rfb/mod.rs`), and its one codec setting is
  `hardware_h264`. Everything is built for its target's baseline, with no CPU floor:
  a wider instruction set is used only where the processor says at run time it
  has it. A measurement of the encoder is run on an idle machine,
  alone: no build, test, profile or second measurement beside it. After Rust
  changes run `cargo test` and `cargo clippy --all-targets -- -D warnings`.
  The daemon has no cross-target check to run: nothing in it is
  architecture-specific but the progressive tiles' coder (`code_tile` in
  `src/rfb/progressive.rs`) and the lossless tiles' (`encode` in
  `src/rfb/planar.rs`), each compiled a second time for AVX2 on x86-64 and
  chosen at run time, the same source, which a test holds to the baseline's
  bytes; and the release builds each architecture in Docker on its own native
  runner.
  Linking the daemon and its tests needs `libpam0g-dev` and
  `libxkbcommon-dev`, and building it needs
  `libpipewire-0.3-dev`, `libspa-0.2-dev` and `libclang-dev` for the audio
  capture and the camera — the last for bindgen, which PipeWire's and FFmpeg's
  `-sys` crates and cros-libva run — `libavcodec-dev` for the camera's H.264
  decoder, and `libx264-dev` and `libva-dev` for the H.264 encoder of the
  video and the tiles' regions (`h264/`), libx264 through its own API and the
  GPU through libva itself, each linked dynamically from the system.
  The audio extension's FLAC coding is not here: it is the `sound-flac`
  repository, shared with the zDesk gateway and pinned by its release tag,
  its own encoder in plain Rust: nothing is needed to build or to run.
  Its Opus coding is the `sound-opus` repository, shared and pinned the same
  way, with libopus linked from a prebuilt static archive: nothing is needed
  to build or to run, and `ffmpeg` on the path to run the daemon's Opus
  tests, which decode with it.
- Every protocol byte comes from the daemon's `rfb` module or, for a private
  extension, from `wlshare-proto`; the rest of the daemon writes none itself.
  Every encoder gets an independent decoder in its tests. The daemon's
  H.264 encoder (`h264/`) is tested against libavcodec's software decoder:
  libx264 on every machine, and the hardware, through libva, where
  the machine has a VA-API encoder, its test passing with a word where it has
  none; testdesktop2
  (`../andrewserver`, the Proxmox host's Intel iGPU passed into it) is where
  the hardware's is run for real.
- Build and run the daemon on a Linux host inside the wlroots-based Wayland
  session it shares; packages are built only in Docker, by
  `scripts/build-debs.sh` for wlshare and `scripts/build-sway-debs.sh` for the
  wlroots, Sway and labwc the APT repository serves, never on the host.
- Do not run `cargo fmt`. Use `anyhow` for application errors and `thiserror` for
  typed protocol errors.
- Translate the READMEs' Simplified (`zh-CN`) and Traditional (`zh-TW`)
  Chinese each from the English, never one by converting the other's
  characters: their technical terms differ (网关/閘道, 配置/設定,
  软件包/套件, 会话/工作階段, 文件/檔案), so each keeps its own vocabulary.
  Everything under `docs/` is English alone, untranslated.
- wlshare's page (its demo) and its user guide, Setting up wlshare, live on
  [zdesk.dev](https://zdesk.dev), in
  [zdesk-website](https://github.com/andrewtheguy/zdesk-website), not in this
  repo. After a change to what a user sees, chooses or configures (a config
  key, a command or flag, a feature, a package or its install, a supported
  compositor), check whether the site needs updating too, and update it there
  in all three locales following that repository's own instructions.
- Design and wire details live in `docs/architecture.md`, not here.
  Measurements live in their own documents (`docs/tiles-performance.md`,
  `docs/video-performance.md`),
  which the architecture links to rather than repeats.
