# 依存の固定と調査記録

外部の成果物はすべて一意に固定する。固定は版を一意にするだけであり、その版が安全であることは示さないため、追加・更新時に以下を調査し、結果を本書に記録する。

- 既知の脆弱性: RustSec、GitHub Advisory Database
- 侵害の情報: maintainer アカウントの乗っ取り、悪意ある版の publish
- クールダウン: 公開から 7 日を経た版のみを採る
- 対象自体の状態: 更新頻度、maintainer

## エコシステムの状況 (2026-09-07 時点)

2026-08-20 に crates.io で maintainer アカウントの侵害による攻撃が発生した。`arrayref` 0.3.10、`internment` 0.8.7、`append-only-vec` 0.1.9 が、build script で外部から payload を取得する `proc-macro1` (proc-macro2 の typosquat) への依存を持って publish され、86〜107 分後に削除された。Rust Security Response Team は当該版の削除と、maintainer アカウントのロックを行い、対応済みとしている ([Rust Blog](https://blog.rust-lang.org/2026/08/20/supply-chain-attack-on-arrayref/))。

本プロジェクトの方針:

- 当該日以降に publish された版は、必要が無い限り採らない
- `Cargo.lock` に `arrayref`、`internment`、`append-only-vec`、`proc-macro1` が現れないことを `make audit` で確認する (cargo-deny の bans)
- build script を持つ crate の追加時は、その内容を確認する

## toolchain と配布物

| 成果物 | 版 | 固定値 (sha256) | 公開日 | 調査 |
| --- | --- | --- | --- | --- |
| nixpkgs | `597283ad8aa0b331c788e97c4c262d58877074ef` (nixos-26.05) | flake.lock の narHash | - | dotfiles と同一 |
| nixos/nix (ベースイメージ) | 2.35.1 | `377d4887aca98f0dfa12971c1ea6d6a625a435d8b610d4c95a436843da6fbfd1` | - | dotfiles と同一 |
| esp-rs/rust-build `rust-1.97.0.0-x86_64-unknown-linux-gnu.tar.xz` | 1.97.0.0 | `a99bfee69221e9ff6d86388f6811ee688cd405e6a0400a3cd1784e8d463e9d99` | 2026-07-08 | 本 asset のみ maintainer (MabezDev) の手動 upload。v1.95.0.0 でも同じ運用であることを確認 |
| esp-rs/rust-build `rust-src-1.97.0.0.tar.xz` | 1.97.0.0 | `568d688b9f8f332ec4d04657544fad23e99ce11e9e6cf5835979e68a28c68b73` | 2026-07-08 | GitHub Actions による upload |
| espressif/crosstool-NG `xtensa-esp-elf-15.2.0_20250920-x86_64-linux-gnu.tar.xz` | 15.2.0_20250920 | `e3d77ad14544814527bbe7a2d0f79ec4592a4e23392c51c7388c0e686b6a6977` | 2025-09-20 | espup 0.17.1 の既定値と同一。espressif organization の release に置かれた asset |
| espflash | 4.4.0 (nixpkgs) | nixpkgs 側で固定 | 2026-04-16 | RustSec / GHSA に advisory なし |
| usbipd-win (Windows ホスト) | 5.3.0 | `1c984914aec944de19b64eff232421439629699f8138e3ddc29301175bc6d938` | 2025-10-11 | Microsoft の WSL 文書が案内する方式 |
| actions/checkout | v4.2.2 | `11bd71901bbe5b1630ceea73d27597364c9af683` | - | dotfiles と同一 |

sha256 の取得元は GitHub Releases の asset digest (API の `digest` フィールド)。

## crate

| crate | 版 | 公開日 | 用途 | 調査 (2026-09-07) |
| --- | --- | --- | --- | --- |
| serde | 1.0.229 | 2026-07-18 | シリアライズ | - |
| postcard | 1.1.3 | 2025-07-24 | no_std シリアライズ、COBS | RustSec / GHSA なし |
| heapless | 0.9.3 | 2026-04-30 | 上限付きコンテナ | RUSTSEC-2020-0145 は <=0.6 のみ影響 |
| clap | 4.6.6 | 2026-08-06 | CLI | - |
| serialport | 4.9.0 | 2026-03-16 | シリアル通信 (MPL-2.0)。`libudev` feature は無効 (C ライブラリ依存を持ち込まないため。Linux では sysfs から VID/PID を列挙する) | RustSec / GHSA なし。4.10.0 (2026-08-26) は侵害事案の直後のため見送り |
| esp-hal | 1.1.2 | 2026-08-05 | HAL (MSRV 1.88) | RustSec / GHSA なし。1.2.0 (2026-09-02) はクールダウン中 |
| esp-backtrace | 0.19.0 | 2026-04-16 | panic / 例外の出力 | esp-hal 1.1 系に対応 |
| esp-println | 0.17.0 | 2026-04-16 | ログ出力 (UART0) | 同上 |
| embedded-hal | 1.0.0 | - | HAL trait | - |
| embedded-hal-bus | 0.3.0 | 2025-01-21 | SPI device の共有 | - |
| embedded-graphics | 0.8.2 | 2026-02-15 | 2D 描画 | GHSA なし |
| mipidsi | 0.10.0 | 2026-02-17 | ILI9342C driver | GHSA なし |
| static_cell | 2.1.1 | 2025-06-22 | 静的確保 | - |

推移的な依存は `Cargo.lock` の checksum で固定する。`make audit` (cargo-deny) が advisory と license を検査する。

### `make audit` の結果 (2026-09-07)

- host workspace: advisories / bans / licenses / sources すべて ok
- firmware workspace: RUSTSEC-2024-0436 (`paste` が unmaintained) のみ。`paste` は esp-hal 1.1.x の直接依存で本プロジェクト側では差し替えられず、脆弱性ではないため `firmware/deny.toml` で理由付きで ignore する。esp-hal の更新時に解消を確認する
- 同一 crate の複数版 (bitflags, embedded-hal, heapless 等) は警告として検出される。esp-hal エコシステム内の版差によるもので、現時点では許容する

## 未調査・保留

- GNU Unifont の版と sha256 (Phase 4 で固定する)
- `cargo fuzz` 用の toolchain (Phase 2)
- 推移的依存の個別調査は `make audit` の結果に基づいて行う
