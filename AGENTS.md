# Repository instructions

- Strict no backward-compatibility or legacy paths no matter what.

- Three crates: `wlshare-proto` (the private extensions' bytes) and
  `wlshare-vp9` (the VP9 coding), platform-independent and tested by a bare
  `cargo test`, and `wlshare` (the daemon, Linux + wlroots-based Wayland only),
  whose RFB protocol is its own module (`src/rfb/`), not a crate. Its one
  client is the zDesk gateway; there is no native client. `wlshare-proto` is
  the one crate anything outside this repository depends on: the gateway pins
  it by this repository's release tag, behind its `wlshare` feature, and it
  depends on `thiserror` alone. Every private extension's bytes, both
  directions, are built and parsed there, never in the daemon or the gateway.
  `wlshare-vp9` is libvpx alone, from a
  prebuilt static archive, set up for a desktop, with the planes in front of
  it, its decoder, the quality walk and the capture file; nothing about a wire
  is decided there. Everything is built for its target's baseline, with no CPU floor.
  libvpx's decoder is also what the daemon's VP9 tests read the
  encoder's frames back with. A measurement of the encoder is run on an idle machine,
  alone: no build, test, profile or second measurement beside it. After Rust
  changes run `cargo test` and `cargo clippy --all-targets -- -D warnings`.
  The daemon has no cross-target check to run: nothing in it is
  architecture-specific, and the release builds each architecture in Docker on
  its own native runner.
  Linking the daemon and its tests needs `libpam0g-dev` and
  `libxkbcommon-dev`, and building it needs
  `libpipewire-0.3-dev`, `libspa-0.2-dev` and `libclang-dev` for the audio
  capture and the camera — the last for bindgen, which PipeWire's and FFmpeg's
  `-sys` crates run — and `libavcodec-dev` for the camera's H.264 decoder and the tiles'
  H.264 encoder (`h264.rs`), the system's libavcodec linked dynamically.
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
  H.264 encoder (`h264.rs`) is tested against libavcodec's software decoder:
  libx264 on every machine, and the hardware, libavcodec's `h264_vaapi`, where
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
- Translate Simplified (`zh-CN`) and Traditional (`zh-TW`) Chinese each from the
  English, never one by converting the other's characters: their technical terms
  differ (网关/閘道, 配置/設定, 软件包/套件, 会话/工作階段, 文件/檔案), so each
  keeps its own vocabulary.
- Design and wire details live in `docs/architecture.md`, not here.
