# wlshare

[English](README.md) | [简体中文](README.zh-CN.md) | 繁體中文

> **不保證向後相容。** 任何版本都可能變更或移除某個設定項目、某個私有 RFB 擴充的傳輸格式或某項功能，且不再讀取舊的寫法。升級時請對照 [`wlshare.example.toml`](wlshare.example.toml) 檢查你的設定；如果 `wlshare --version` 列出的協定版本有變化，請同時升級 zDesk 閘道。

一個針對以 wlroots 為基礎的 Wayland 合成器的 VNC 伺服器，為 [zDesk](https://github.com/andrewtheguy/zdesk/blob/main/README.zh-TW.md) 閘道而打造。它透過 wlr-screencopy 擷取一個輸出，以 RFB 3.8 的 Raw 或 ZRLE 提供；或者，對於提出要求的用戶端（zDesk 就為它的瀏覽器這樣要求），以一路品質隨連線變化的 VP9 串流提供；在連線頻寬充裕時，則以精確的 PNG 圖塊提供，並為每個移動區域提供一路視訊串流，即由主機硬體或 libx264 編碼的 H.264；或者隨連線調整，以 RemoteFX Progressive 圖塊提供，無論靜止還是移動（實驗性）。它透過虛擬鍵盤和虛擬指標協定注入輸入，透過 wlr-data-control 共用剪貼簿，把桌面來自 PipeWire 的聲音經由同一個連線傳輸；它還會告訴用戶端影格緩衝區是以什麼像素密度繪製的，而這是標準 RFB 做不到的，因此 `scale 2` 的輸出會以 2x 清晰顯示，用戶端自己的密度也會成為輸出的密度。有多個顯示器的桌面會把清單傳給用戶端，由用戶端選擇共用哪一個；該用戶端的第二個連線會在旁邊顯示另一個輸出，從而同時看到兩個。合成器的指標不包含在擷取的影格中，因此 wlshare 自行擷取游標圖像（即指標下方的應用程式所選擇的任何形狀，依輸出的像素密度），並透過 RFB 的 Cursor 虛擬編碼（pseudo-encoding）傳送；對於列出 Cursor With Alpha 的用戶端，還會附帶 Alpha 透明度。用戶端移動游標時不必等待影格緩衝區更新。

在 zDesk 的示範中觀看：
[Mac -> Linux（Sway 上的 wlshare），Retina/HiDPI 密度並帶聲音](https://zdesk.dev/zh-TW/demo#mac-to-linux)。

在這組協定範圍內，它與特定合成器無關：任何提供所需協定、以 wlroots 為基礎的合成器都是同一類對等端。需要 `wlr-screencopy` 版本 2 或更新，以及具備輸出擷取來源的 `ext-image-copy-capture`，游標圖像就來自後者。調整輸出尺寸和縮放還需要 `wlr-output-management`；合成器提供虛擬鍵盤、虛擬指標和 `wlr-data-control` 協定時，輸入和剪貼簿會使用它們。在無頭輸出上讓游標保持獨立並擷取它，需要 wlroots 0.19 或更新版本。

在影格緩衝區層面，任何宣告了標準 Cursor 虛擬編碼並能繪製 Raw 矩形的 RFB 3.8 用戶端都可以連線。用戶端列出 ZRLE 時會使用它，除非用戶端還要求了 wlshare 私有的 VP9 串流或它的圖塊。設定了登入時，還需要 RSA-AES。之所以要求支援游標，是因為 wlshare 從不把指標畫進影格緩衝區的像素。密度和輸出擴充由用戶端要求，否則保持靜默；在設定了 `subtype = "wlshare"` 的 `vnc` 目標上，zDesk 會要求這兩者以及 VP9 串流（選擇器中選了 *Tiles with motion* 或 *Progressive tiles (experimental)* 時則要求圖塊），在未設定的目標上則以任何 VNC 伺服器的方式讀取 wlshare。音訊是一個私有擴充，zDesk 在這類目標上以聲音啟動的工作階段中會要求它：在用戶端收聽期間，wlshare 自己的輸出裝置（sink）是預設裝置，因此所有跟隨預設裝置的串流都在那裡播放，而不是在主機的喇叭上；該裝置的監聽來源會編碼為無損 FLAC 傳送，或在用戶端改為要求 Opus 時，以用戶端指定的位元率編碼為 Opus 傳送。固定到主機某個輸出裝置的應用程式會留在那裡，在主機上播放。沒有列出該擴充的用戶端聽不到任何聲音。
聲音、攝影機和麥克風一律開啟，不需要任何設定來開啟：套件相依於它們所經由的 PipeWire、WirePlumber 和 pipewire-pulse。列出攝影機擴充的用戶端（zDesk 在勾選了選擇器中 Camera 後啟動的工作階段中會這樣做）可以把它的攝影機借給桌面：它傳送的 H.264 由系統的 libavcodec 解碼到一個 PipeWire 視訊來源 "wlshare remote camera" 中，透過 PipeWire 存取攝影機的應用程式都可以開啟它，並且只有在有應用程式開啟它時才會向用戶端要求影格。瀏覽器透過 `xdg-desktop-portal` 的 Camera 介面成為這樣的應用程式，而 portal 只有在有實作了 Access 的後端（例如 `xdg-desktop-portal-gtk`）時才提供該介面——只有 `xdg-desktop-portal-wlr` 不行。套件建議安裝（Recommends）這兩者，因此安裝了建議套件的系統就有該介面。Chrome 還需要啟用 `chrome://flags/#enable-webrtc-pipewire-camera`；沒有這個旗標或 portal 時，它只查看 `/dev/video*`，列不出任何攝影機。列出麥克風擴充的用戶端可以用同樣的方式把它的麥克風借給桌面：一個 PipeWire 音訊來源 "wlshare remote microphone"，在有應用程式從它錄音時，由用戶端傳送的 16 位元 PCM 提供資料。
桌面同一時間只有一個用戶端：完成交握的連線會從目前持有者手中接管桌面，與 Windows 遠端桌面的做法一樣，RFB 的 shared 旗標不起任何作用。它的運作方式以及刻意不做的內容，請見 [`docs/architecture.zh-TW.md`](docs/architecture.zh-TW.md)。

## 適用於 Debian trixie 的 Sway、labwc 和 wlroots

Trixie 內建的 Sway 1.10 和 labwc 0.8.3 以 wlroots 0.18 為基礎，其無頭後端會把游標畫進擷取畫面。GitHub Pages 提供一個經過簽署的 APT 套件庫，其中有針對 trixie 本身的函式庫重新建置的 Sway 1.11、labwc 0.9.7 和 wlroots 0.19。wlshare 本身不在其中。

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

這些套件是 Debian 自己的原始碼套件，由 `packaging/apt/sources.env` 中 snapshot.debian.org 上的 `.dsc` 固定版本，`packaging/apt/patches/<source>/` 中的修補檔系列在 Debian 的修補檔之後套用。wlroots 維持在 0.19，這是 trixie 的 libdrm 和 wayland-protocols 能建置的最新系列；labwc 維持在 0.9，這是它以 wlroots 0.19 建置的最後一個系列。它們的版本號為 `<upstream>+<YYYYMMDD>-<N>~trixie`，高於 trixie 的套件以及 Debian 對同一版本的建置。`scripts/build-sway-debs.sh` 在 Docker 中把它們建置到 `dist/sway/<arch>/`。

**Release Sway packages** 工作流程建置兩種架構，並將它們發行為預先發行版本 `sway-<YYYYMMDD>-<N>`，它只用來存放套件。**Publish APT repository** 在其之後執行：它為最近的三個 `sway-*` 發行版本建立索引，用 `packaging/apt/pubkey.asc` 中的金鑰簽署索引，在 trixie 容器中從結果安裝 Sway 和 labwc，並把它部署為整個 Pages 網站。它需要以 `GPG_PRIVATE_KEY` secret 提供私密金鑰。這把金鑰與 podman-package 的套件庫使用的是同一把，其私密金鑰部分保存在被 gitignore 的 `keys/` 中。要在本機執行組裝流程，請先匯入這把金鑰：

```sh
gpg --import keys/apt-signing-key.private.asc
./scripts/apt-repo-build.sh dist/sway site https://andrewtheguy.github.io/wlshare
./scripts/apt-repo-smoke.sh site
```

## 執行

wlshare 在它擷取的 Wayland 工作階段中執行，以擁有該工作階段的使用者身分：

```sh
WAYLAND_DISPLAY=wayland-1 wlshare --config ~/.config/wlshare/config.toml
```

或者作為套件安裝的 systemd 使用者單元，在圖形工作階段的環境中包含 `WAYLAND_DISPLAY` 之後隨之啟動：

```sh
systemctl --user enable --now wlshare.service
```

設定是一個 TOML 檔案，預設為 `$XDG_CONFIG_HOME/wlshare/config.toml`。空檔案使用預設值，[`wlshare.example.toml`](wlshare.example.toml) 列出了所有設定。

誰可以連線由一項設定決定，有三種選擇。什麼都不設定：任何能連到連接埠的人都能進入，工作階段以明文傳輸，因此 `listen` 應維持在 loopback 位址上，或位於 VPN 或 SSH 通道之後。`[pam]`：RSA-AES，即 TigerVNC 和 zDesk 都支援的 RealVNC 安全性類型。用戶端傳送執行 wlshare 的帳號的使用者名稱和密碼，PAM 在服務名稱 `wlshare` 下檢查它們（套件會安裝 `/etc/pam.d/wlshare`），金鑰交換之後的所有內容都經過加密。不接受其他帳號，因為連接埠背後的桌面屬於這一位使用者。`[password]`：同樣的 RSA-AES，但只要求一組密碼，與設定檔中的 Argon2 雜湊比對，適用於不應以任何系統帳號的密碼作為入口的主機。用以下指令印出雜湊：

```sh
wlshare hash-password
```

並把它貼到該表格中；密碼本身從不寫下來。

三種方式都刻意不提供傳統的 VncAuth：它不識別任何人，把密碼截斷為八個字元，只證明知道某台機器的秘密，而且工作階段以明文傳輸。伺服器的 RSA 金鑰在第一次啟動時產生到 `rsa_key_file`，並記錄其指紋，以便與用戶端顯示的指紋比對。在 `[pam]` 下，由於 PAM 驗證的密碼就是帳號的密碼，PAM 堆疊可以把它傳遞下去：在其中加一行 `pam_exec ... expose_authtok`，就是無頭工作階段在 VNC 登入時解鎖其金鑰圈的方式。

`state_socket` 指定一個 Unix 通訊端，常駐程式在其上告知是否有用戶端在桌面上；`wlshare watch --held COMMAND --free COMMAND` 在一個獨立的處理程序中跟隨它，這樣 sway 工作階段就可以在桌面被別處觀看時，把視窗移到用戶端看到的無頭輸出上並關閉顯示器，之後再恢復原狀；常駐程式在有用戶端在桌面上時被終止也一樣，監看程式會將其視為桌面已被離開。無頭輸出本身在無人觀看時可以關閉：`output` 指定的是偏好而不是要求，`output_wait_secs` 讓接管桌面的用戶端在監看程式啟用該輸出期間等待它。範例設定列出了兩行 `swaymsg`，每一行何時執行請見 [`docs/architecture.zh-TW.md`](docs/architecture.zh-TW.md#狀態通訊端)。

用戶端可以要求它想要的虛擬顯示器數量，就像 RDP 用戶端向 Windows 主機要求顯示器、Mac 的檢視器要求虛擬顯示器一樣：一個或兩個。在 sway 上，常駐程式透過 sway 的 IPC（在 `SWAYSOCK` 中找到）建立和拔除無頭輸出，直到數量相符，不需要任何設定：以一個無頭輸出啟動的工作階段，在 zDesk 閘道的目標要求兩個（`virtual_displays = 2`）時會得到第二個。在其他任何合成器上，輸出就是合成器已有的那些，清單也會如實說明（[`docs/architecture.zh-TW.md`](docs/architecture.zh-TW.md#虛擬顯示器)）。

自訂鍵盤配置（用於工作階段中唯一的鍵盤必須承載的輔助鍵重新對應）放在 `[xkb]` 下，並在單元的環境中用 `XKB_CONFIG_EXTRA_PATH` 指定存放它的目錄。每個按鍵必須保留其 keysym：伺服器透過它上傳的同一份鍵盤對應表來解析用戶端的 keysym。

## 建置

工作區有三個 crate：常駐程式 `wlshare`，它所使用的 RFB 協定就在其中；其下的 VP9 編碼 `wlshare-vp9`；以及 `wlshare-proto`，即 wlshare 私有擴充的位元組，zDesk 閘道也相依於它，依它的 `proto-v<N>` 標籤固定版本，即帶來該協定版本的那次發行所加上的標籤。後兩者不使用任何作業系統 API，可在預先建置的 libvpx 封存檔涵蓋的目標上建置和測試：macOS arm64、Linux x86_64 和 aarch64，以及 Windows x86_64 MSVC。常駐程式需要 libxkbcommon、libpam、libpipewire 和 libavcodec（Wayland 協定以 Rust 實作，不需要 libwayland），並且只能在以 wlroots 為基礎的 Wayland 合成器下執行；它的音訊測試還需要 `ffmpeg`，即把 Opus 封包讀回來的獨立解碼器。FLAC 編碼是 [sound-flac](https://github.com/andrewtheguy/sound-flac)，Opus 編碼是 [sound-opus](https://github.com/andrewtheguy/sound-opus)，兩者都與 zDesk 閘道共用，各自由其發行標籤固定版本。對處理器的要求不超過其目標的基準。常駐程式是儲存庫根目錄下的套件，另外兩者位於 `crates/` 之下。直接執行 `cargo test` 涵蓋全部三者；在其他平台上，`cargo test -p wlshare-proto -p wlshare-vp9` 涵蓋後兩者。常駐程式在裝有 `libxkbcommon-dev`、`libpam0g-dev`、`libpipewire-0.3-dev`、`libspa-0.2-dev`、`libavcodec-dev`、`libclang-dev` 和 `pkg-config` 的 Linux 主機上用 `cargo build --release` 建置。libclang 不參與連結：PipeWire 和 FFmpeg 的 `-sys` crate 用 bindgen 產生繫結，bindgen 在建置時載入它。

`scripts/capture-frames.sh` 用 docker 或 podman 建置並執行擷取容器（`docker/capture/`）：一個無頭 sway 和以 `--capture-frames` 啟動的常駐程式，在每種尺寸下播放一個指令碼化的桌面，讓交給 VP9 編碼器的內容（每一個影格的像素和變更區域（damage），完全精確）落在 `dist/captures/` 中；視訊情境以全螢幕播放 `--video` 指定的檔案。`scripts/vp9-samples.sh` 把這些擷取再次送入編碼器，並把 vp9-wasm 效能測試解碼的串流寫入 `dist/vp9-bench/`，每種尺寸一路安靜的、一路繁忙的，附帶每一個影格的 MD5：在修改 `wlshare-vp9` 或升級其 libvpx 後再次執行，樣本就是新編碼器的，不必重新播放任何桌面。它需要 cargo、`zstd` 以及路徑上具備 libvpx 的 `ffmpeg`。`scripts/capture-samples.sh` 把每個擷取截取為 120 個影格，以各自的名稱放入 `dist/capture-samples/`，用於在每個擷取的一部分上測量編碼器。`--capture-frames` 用於手動執行的常駐程式，由 systemd 啟動的常駐程式會拒絕它（[`docs/architecture.zh-TW.md`](docs/architecture.zh-TW.md#擷取交給編碼器的內容)）。

適用於 amd64 和 arm64 上 Debian trixie 的套件由 `scripts/build-debs.sh` 在 Docker 中建置，輸出到 `dist/<arch>/wlshare-trixie-<arch>.deb`。**Release wlshare** 工作流程建置同樣的套件，並將它們發行為 GitHub 發行版本 `v<version>`，版本號為工作區 `Cargo.toml` 中的版本；執行工作流程前請先調升它。套件版本即 crate 的版本，發行版只反映在檔案名稱中。套件相依於 `libwlroots-0.19 (>= 0.19.0)`，即第一個把游標排除在無頭擷取之外的 wlroots，以及 `pipewire`、`wireplumber` 和 `pipewire-pulse`：喇叭、攝影機和麥克風所在的 PipeWire 伺服器，把喇叭設為預設並把串流連接到它的工作階段管理員，以及大多數應用程式透過它播放聲音的 PulseAudio 伺服器。少了它們，桌面的聲音永遠到不了喇叭，用戶端什麼也聽不到。
