# wlshare

[English](README.md) | 简体中文 | [繁體中文](README.zh-TW.md)

> **不保证向后兼容。** 任何版本都可能更改或移除某个配置项、某个私有 RFB 扩展的线路格式或某项功能，且不再读取旧的写法。升级时请对照 [`wlshare.example.toml`](wlshare.example.toml) 检查你的配置；如果 `wlshare --version` 给出的协议版本有变化，请同时升级 zDesk 网关。

一个面向基于 wlroots 的 Wayland 合成器的 VNC 服务器，为 [zDesk](https://github.com/andrewtheguy/zdesk/blob/main/README.zh-CN.md) 网关而构建。它通过 wlr-screencopy 捕获一个输出，以 RFB 3.8 的 Raw 或 ZRLE 提供；或者，对于提出请求的客户端（zDesk 就为它的浏览器这样请求），以一路质量随链路变化的 VP9 流提供；在链路带宽充裕时，则以精确的 PNG 图块提供，并为每个运动区域提供一路视频流，即由主机硬件或 libx264 编码的 H.264。它通过虚拟键盘和虚拟指针协议注入输入，通过 wlr-data-control 共享剪贴板，把桌面来自 PipeWire 的声音经同一连接传输；它还会告诉客户端帧缓冲是以什么像素密度绘制的，而这是标准 RFB 做不到的，因此 `scale 2` 的输出会以 2x 清晰显示，客户端自己的密度也会成为输出的密度。有多个显示器的桌面会把列表发给客户端，由客户端选择共享哪一个；该客户端的第二个连接会在旁边显示另一个输出，从而同时看到两个。合成器的指针不包含在捕获的帧中，因此 wlshare 自行捕获光标图像（即指针下方的应用所选择的任何形状，按输出的像素密度），并通过 RFB 的 Cursor 伪编码发送；对于列出 Cursor With Alpha 的客户端，还会带上透明度。客户端移动光标时无需等待帧缓冲更新。

在 zDesk 的演示中观看：
[Mac -> Linux（Sway 上的 wlshare），Retina/HiDPI 密度并带声音](https://zdesk.dev/zh-CN/demo#mac-to-linux)。

在这组协议范围内，它与具体合成器无关：任何提供所需协议的基于 wlroots 的合成器都是同一类对端。需要 `wlr-screencopy` 版本 2 或更高，以及带输出捕获源的 `ext-image-copy-capture`，光标图像就来自后者。调整输出尺寸和缩放还需要 `wlr-output-management`；合成器提供虚拟键盘、虚拟指针和 `wlr-data-control` 协议时，输入和剪贴板会使用它们。在无头输出上让光标保持独立并捕获它，需要 wlroots 0.19 或更新版本。

在帧缓冲层面，任何声明了标准 Cursor 伪编码并能绘制 Raw 矩形的 RFB 3.8 客户端都可以连接。客户端列出 ZRLE 时会使用它，除非客户端还请求了 wlshare 私有的 VP9 流或它的图块。配置了登录时，还需要 RSA-AES。之所以要求支持光标，是因为 wlshare 从不把指针画进帧缓冲像素。密度和输出扩展由客户端请求，否则保持静默；在设置了 `subtype = "wlshare"` 的 `vnc` 目标上，zDesk 会请求这两者以及 VP9 流（选择器中选了 *Tiles with motion* 时则请求图块），在未设置的目标上则按任何 VNC 服务器的方式读取 wlshare。音频是一个私有扩展，zDesk 在这类目标上以声音启动的会话中会请求它：在客户端收听期间，wlshare 自己的输出设备（sink）是默认设备，因此所有跟随默认设备的流都在那里播放，而不是在主机的扬声器上；该设备的监听源会编码为无损 FLAC 发送，或在客户端改为请求 Opus 时，以客户端指定的比特率编码为 Opus 发送。固定到主机某个输出设备的应用会留在那里，在主机上播放。没有列出该扩展的客户端听不到任何声音。
声音、摄像头和麦克风始终开启，无需任何配置来打开：软件包依赖它们所经由的 PipeWire、WirePlumber 和 pipewire-pulse。列出摄像头扩展的客户端（zDesk 在勾选了选择器中 Camera 后启动的会话中会这样做）可以把它的摄像头借给桌面：它发送的 H.264 由系统的 libavcodec 解码到一个 PipeWire 视频源 "wlshare remote camera" 中，通过 PipeWire 访问摄像头的应用都可以打开它，并且只有在有应用打开它时才会向客户端请求帧。Chrome 只有在启用 `chrome://flags/#enable-webrtc-pipewire-camera` 并有实现了 Access 的 `xdg-desktop-portal` 后端（例如 `xdg-desktop-portal-gtk`）时才是这样的应用；仅有 `xdg-desktop-portal-wlr` 不行，没有这样的后端时门户不提供 Camera 接口，否则 Chrome 只查看 `/dev/video*`，列不出任何摄像头。列出麦克风扩展的客户端可以用同样的方式把它的麦克风借给桌面：一个 PipeWire 音频源 "wlshare remote microphone"，在有应用从它录音时，由客户端发送的 16 位 PCM 提供数据。
桌面同一时间只有一个客户端：完成握手的连接会从当前持有者手中接管桌面，与 Windows 远程桌面的做法一样，RFB 的 shared 标志不起任何作用。它的工作原理以及刻意不做的内容，见 [`docs/architecture.zh-CN.md`](docs/architecture.zh-CN.md)。

## 适用于 Debian trixie 的 Sway、labwc 和 wlroots

Trixie 自带的 Sway 1.10 和 labwc 0.8.3 基于 wlroots 0.18，其无头后端会把光标画进捕获画面。GitHub Pages 提供一个签名的 APT 软件源，其中有针对 trixie 自身的库重新构建的 Sway 1.11、labwc 0.9.7 和 wlroots 0.19。wlshare 本身不在其中。

```sh
sudo mkdir -p /etc/apt/keyrings
sudo curl -fsSL -o /etc/apt/keyrings/wlshare.gpg https://andrewtheguy.github.io/wlshare/wlshare.gpg
sudo tee /etc/apt/sources.list.d/wlshare.sources <<'EOF'
Types: deb
URIs: https://andrewtheguy.github.io/wlshare
Suites: trixie
Components: main
Signed-By: /etc/apt/keyrings/wlshare.gpg
EOF
sudo apt update && sudo apt install sway  # 或 labwc
```

这些软件包是 Debian 自己的源码包，由 `packaging/apt/sources.env` 中 snapshot.debian.org 上的 `.dsc` 固定版本，`packaging/apt/patches/<source>/` 中的补丁系列在 Debian 的补丁之后应用。wlroots 保持在 0.19，这是 trixie 的 libdrm 和 wayland-protocols 能构建的最新系列；labwc 保持在 0.9，这是它基于 wlroots 0.19 构建的最后一个系列。它们的版本号为 `<upstream>+<YYYYMMDD>-<N>~trixie`，高于 trixie 的软件包以及 Debian 对同一版本的构建。`scripts/build-sway-debs.sh` 在 Docker 中把它们构建到 `dist/sway/<arch>/`。

**Release Sway packages** 工作流构建两种架构，并将它们发布为预发布版本 `sway-<YYYYMMDD>-<N>`，它只用于存放软件包。**Publish APT repository** 在其之后运行：它为最近的三个 `sway-*` 发布建立索引，用 `packaging/apt/pubkey.asc` 中的密钥签名索引，在 trixie 容器中从结果安装 Sway 和 labwc，并把它部署为整个 Pages 站点。它需要以 `GPG_PRIVATE_KEY` 机密提供私钥。该密钥与 podman-package 的软件源使用的是同一个，其私钥部分保存在被 gitignore 的 `keys/` 中。要在本地运行组装过程，先导入该密钥：

```sh
gpg --import keys/apt-signing-key.private.asc
./scripts/apt-repo-build.sh dist/sway site https://andrewtheguy.github.io/wlshare
./scripts/apt-repo-smoke.sh site
```

## 运行

wlshare 在它捕获的 Wayland 会话中运行，以拥有该会话的用户身份：

```sh
WAYLAND_DISPLAY=wayland-1 wlshare --config ~/.config/wlshare/config.toml
```

或者作为软件包安装的 systemd 用户单元，在图形会话的环境中包含 `WAYLAND_DISPLAY` 之后随之启动：

```sh
systemctl --user enable --now wlshare.service
```

配置是一个 TOML 文件，默认为 `$XDG_CONFIG_HOME/wlshare/config.toml`。空文件使用默认值，[`wlshare.example.toml`](wlshare.example.toml) 列出了所有设置。

谁可以连接由一个设置决定，有三种选择。什么都不设置：任何能访问到端口的人都能进入，会话以明文传输，因此 `listen` 应保持在回环地址上，或位于 VPN 或 SSH 隧道之后。`[pam]`：RSA-AES，即 TigerVNC 和 zDesk 都支持的 RealVNC 安全类型。客户端发送运行 wlshare 的账户的用户名和密码，PAM 在服务名 `wlshare` 下检查它们（软件包会安装 `/etc/pam.d/wlshare`），密钥交换之后的所有内容都经过加密。不接受其他账户，因为端口背后的桌面属于这一个用户。`[password]`：同样的 RSA-AES，但只要求一个密码，与配置文件中的 Argon2 哈希比对，适用于不应以任何系统账户的密码作为入口的主机。用以下命令输出哈希：

```sh
wlshare hash-password
```

并把它粘贴到该表中；密码本身从不写下来。

三种方式都刻意不提供经典的 VncAuth：它不标识任何人，把密码截断为八个字符，只证明知道某台机器的秘密，并且会话以明文传输。服务器的 RSA 密钥在首次启动时生成到 `rsa_key_file`，并记录其指纹，以便与客户端显示的指纹比对。在 `[pam]` 下，由于 PAM 验证的密码就是账户的密码，PAM 栈可以把它传递下去：在其中加一行 `pam_exec ... expose_authtok`，就是无头会话在 VNC 登录时解锁其密钥环的方式。

`state_socket` 指定一个 Unix 套接字，守护进程在其上告知是否有客户端在桌面上；`wlshare watch --held COMMAND --free COMMAND` 在一个独立的进程中跟随它，这样 sway 会话就可以在桌面被别处观看时，把窗口移到客户端看到的无头输出上并关闭显示器，之后再恢复原样；守护进程在有客户端在桌面上时被杀死也一样，监视进程会将其视为桌面已被离开。无头输出本身在无人观看时可以关闭：`output` 指定的是首选项而不是要求，`output_wait_secs` 让接管桌面的客户端在监视进程启用该输出期间等待它。示例配置给出了两行 `swaymsg`，每一行何时运行见 [`docs/architecture.zh-CN.md`](docs/architecture.zh-CN.md#状态套接字)。

客户端可以请求它想要的虚拟显示器数量，就像 RDP 客户端向 Windows 主机请求显示器、Mac 的查看器请求虚拟显示器一样：一个或两个。在 sway 上，守护进程通过 sway 的 IPC（在 `SWAYSOCK` 中找到）创建和拔除无头输出，直到数量符合，无需任何配置：以一个无头输出启动的会话，在 zDesk 网关的目标请求两个（`virtual_displays = 2`）时会得到第二个。在其他任何合成器上，输出就是合成器已有的那些，列表也会如实说明（[`docs/architecture.zh-CN.md`](docs/architecture.zh-CN.md#虚拟显示器)）。

自定义键盘布局（用于会话中唯一的键盘必须承载的修饰键重映射）放在 `[xkb]` 下，并在单元的环境中用 `XKB_CONFIG_EXTRA_PATH` 指定存放它的目录。每个按键必须保留其 keysym：服务器通过它上传的同一个键盘映射来解析客户端的 keysym。

## 构建

工作区有三个 crate：守护进程 `wlshare`，它所使用的 RFB 协议就在其中；其下的 VP9 编码 `wlshare-vp9`；以及 `wlshare-proto`，即 wlshare 私有扩展的字节，zDesk 网关也依赖它，按它的 `proto-v<N>` 标签固定版本，即带来该协议版本的那次发布所打的标签。后两者不使用任何操作系统 API，可在预构建 libvpx 归档覆盖的目标上构建和测试：macOS arm64、Linux x86_64 和 aarch64，以及 Windows x86_64 MSVC。守护进程需要 libxkbcommon、libpam、libpipewire 和 libavcodec（Wayland 协议用 Rust 实现，不需要 libwayland），并且只能在基于 wlroots 的 Wayland 合成器下运行；它的音频测试还需要 `ffmpeg`，即把 Opus 数据包读回来的独立解码器。FLAC 编码是 [sound-flac](https://github.com/andrewtheguy/sound-flac)，Opus 编码是 [sound-opus](https://github.com/andrewtheguy/sound-opus)，两者都与 zDesk 网关共享，各自由其发布标签固定版本。对处理器的要求不超过其目标的基线。直接运行 `cargo test` 覆盖 `wlshare-proto` 和 `wlshare-vp9`，不包括守护进程；守护进程在装有 `libxkbcommon-dev`、`libpam0g-dev`、`libpipewire-0.3-dev`、`libspa-0.2-dev`、`libavcodec-dev`、`libclang-dev` 和 `pkg-config` 的 Linux 主机上用 `cargo build --release -p wlshare` 构建。libclang 不参与链接：PipeWire 和 FFmpeg 的 `-sys` crate 用 bindgen 生成绑定，bindgen 在构建时加载它。

`scripts/capture-frames.sh` 用 docker 或 podman 构建并运行捕获容器（`docker/capture/`）：一个无头 sway 和以 `--capture-frames` 启动的守护进程，在每种尺寸下播放一个脚本化的桌面，使交给 VP9 编码器的内容（每一帧的像素和变化区域（damage），完全精确）落在 `dist/captures/` 中；视频场景全屏播放 `--video` 指定的文件。`scripts/vp9-samples.sh` 把这些捕获再次送入编码器，并把 vp9-wasm 基准测试解码的流写入 `dist/vp9-bench/`，每种尺寸一路安静的、一路繁忙的，附带每一帧的 MD5：在修改 `wlshare-vp9` 或升级其 libvpx 后再次运行，样本就是新编码器的，无需重新播放任何桌面。它需要 cargo、`zstd` 以及路径上带 libvpx 的 `ffmpeg`。`scripts/capture-samples.sh` 把每个捕获截取为 120 帧，以各自的名称放入 `dist/capture-samples/`，用于在每个捕获的一部分上测量编码器。`--capture-frames` 用于手动运行的守护进程，由 systemd 启动的守护进程会拒绝它（[`docs/architecture.zh-CN.md`](docs/architecture.zh-CN.md#捕获交给编码器的内容)）。

适用于 amd64 和 arm64 上 Debian trixie 的软件包由 `scripts/build-debs.sh` 在 Docker 中构建，输出到 `dist/<arch>/wlshare-trixie-<arch>.deb`。**Release wlshare** 工作流构建同样的软件包，并将它们发布为 GitHub 发布 `v<version>`，版本号为工作区 `Cargo.toml` 中的版本；运行工作流前先提升它。软件包版本即 crate 的版本，发行版只体现在文件名中。软件包依赖 `libwlroots-0.19 (>= 0.19.0)`，即第一个把光标排除在无头捕获之外的 wlroots，以及 `pipewire`、`wireplumber` 和 `pipewire-pulse`：扬声器、摄像头和麦克风所在的 PipeWire 服务器，把扬声器设为默认并把流连接到它的会话管理器，以及大多数应用通过它播放声音的 PulseAudio 服务器。缺少它们，桌面的声音永远到不了扬声器，客户端什么也听不到。
