# 開発環境

Nix で定義した環境をコンテナ内に構築して使う。構成は [sabas0ba/dotfiles](https://github.com/sabas0ba/dotfiles) に従う。

## 構成

```
flake.nix / flake.lock   nixpkgs を rev で固定 (dotfiles と同一 rev)
nix/packages.nix         ツールの一覧 (単一情報源)
nix/esp-rust.nix         Xtensa 対応の Rust toolchain (Espressif fork) を sha256 固定で取得
nix/xtensa-gcc.nix       Xtensa 向け GCC (リンクに使用) を sha256 固定で取得
nix/cargo-vendor.nix     Cargo.lock の依存を store に取り込む (ネットワーク無しの build 用)
nix/devshell.nix         開発シェルの定義
nix/checks.nix           nix flake check の検査
Dockerfile               同一の flake からコンテナを構築する
Makefile                 コンテナ内 (開発シェル内) の操作の入り口
scripts/container.sh     ホスト側 (Windows / Linux) からコンテナを操作する
.work/                   一時ファイル置き場 (git 管理外)。CARGO_HOME もここに置く
```

Rust の toolchain は nixpkgs のものではなく、Espressif fork の sysroot を 1 本だけ使う。x86_64-linux の std も含むため、host 側の crate も同じ toolchain で build する。rustup と espup は使用しない。

## Windows ホストからの使い方

前提: Podman (WSL machine)、Git Bash。nix と make はホストに不要。

```bash
scripts/container.sh lock     # flake.lock を生成する (初回、または flake.nix の変更時)
scripts/container.sh build    # イメージを構築する。toolchain の取得を含み初回は数分かかる
scripts/container.sh shell    # 開発シェルに入る
scripts/container.sh check    # --network none で make check を実行する
```

`.git` が worktree のファイルである場合、`container.sh` は main checkout の `.git` を併せてマウントする。

### USB デバイスを渡す

Windows の USB デバイスを Podman machine (WSL) に渡すには usbipd-win を使う。

```powershell
usbipd list                                  # BUSID を確認する (CoreS3 は 303a:1001)
usbipd bind --busid <BUSID>                  # 管理者権限。初回のみ
usbipd attach --wsl podman-machine-default --busid <BUSID>
```

attach 後、machine 内に `/dev/ttyACM0` が現れる。コンテナには `device` サブコマンドで渡す。

```bash
scripts/container.sh device make flash
scripts/container.sh device stackchan ping
```

取り外す場合は `usbipd detach --busid <BUSID>`。

## Linux ホストからの使い方

nix を持つ場合は `nix develop` または `direnv allow` で開発シェルに入り、`make help` で操作を一覧する。コンテナを使う場合は `make docker-build` / `make docker-shell` / `make docker-check`。

## Phase 1 の実機確認

実機への書込は利用者の許可を得てから行う。対象は ILI9342C 搭載の CoreS3。microSD カードは取り外す。

1. `scripts/container.sh check` で静的検査・テスト・firmware の release build を通す。
2. 前節の手順で対象 CoreS3 の USB を Podman machine に接続する。
3. `scripts/container.sh device make flash` で書き込む。
4. 起動後、外枠の四辺が欠けず、上部の `my-stackchan / CoreS3` が正しい向きで読めることを確認する。
5. 中央の顔、下部の `ASCII 0123456789 !?` と `RGB565`、左から赤・緑・青・白のカラーバーを確認する。
6. リセット後と電源再投入後の両方で同じ画面になることを確認する。

成功時は UART0 に `Phase 1 display ready: face / ASCII / RGB565` を出力する。USB Serial/JTAG へログは出さないため、USB monitor でこのログは観測できない。表示が点灯しない場合は UART0 のエラーと内部 I2C の応答を確認する。ビルド成功だけでは実機の表示確認を代替できない。

## 検査

### Ping/Pong の実機確認

前節の USB 接続と書き込みを済ませてから実行する。書き込みには利用者の許可が必要。

```bash
scripts/container.sh device make flash
scripts/container.sh device bash scripts/check-ping-device.sh /dev/ttyACM0
```

`check-ping-device.sh` は通常の `stackchan ping`、分割・連結フレーム、不正・過長フレームを
拒否した後の復帰を検証する。続いて `.work/ping-device.*` に host ソースを複製し、
検証用コピーの `protocol::VERSION` だけを変更して build する。実機の `Pong` に対して
版不一致エラーと終了コード 1 を確認し、最後に通常の host でもう一度 Ping を確認する。
検証用コピーと結果の `mismatch.log` は `.work/` に残る。firmware の版は変更しない。

通常のテスト実行では実機テストをスキップする。単独実行する場合は次のように明示する。

```bash
scripts/container.sh device env STACKCHAN_TEST_PORT=/dev/ttyACM0 \
  cargo test --locked --offline -p my-stackchan-host \
  -- --ignored --exact tests::hardware_ping_and_frame_recovery
```

### 静的検査・単体テスト

```bash
make check        # nix flake check + 環境 + Rust の fmt/clippy/test
make lint         # 静的解析のみ
make fmt          # 整形
make audit        # cargo-deny による advisory と license の検査 (ネットワークを使用)
```

CI (`.github/workflows/ci.yml`) はイメージを構築し、`--network none` のコンテナ内で `make check` を実行する。

`make check` は firmware の電源制御と描画処理も host 上でテストする。対象外レジスタビットの保持、I2C エラー伝播、描画範囲、RGB565 の色順、描画エラー伝播を検証する。単独実行は開発シェル内のリポジトリルートから `cargo test --manifest-path firmware/Cargo.toml --lib --locked --offline` を使う。`firmware/` 内から実行すると Xtensa 用の Cargo 設定が適用されるため、host テストではルートから実行する。

## cargo の依存とネットワーク

開発シェルでは crates.io を Nix の store 内の vendor (`nix/cargo-vendor.nix`) で置き換える。`Cargo.lock` に記録された crate と、build-std が要求する rust-src 同梱の crate がイメージの構築時に取り込まれるため、build と検査はネットワーク無しで完結する。

依存を追加・更新する (`Cargo.lock` が変わる) 場合のみ、置き換えを外して crates.io を参照する。

```bash
scripts/container.sh run env CARGO_HOME=/workspace/.work/cargo-online cargo update -p <crate> --precise <version>
scripts/container.sh run env CARGO_HOME=/workspace/.work/cargo-online cargo generate-lockfile   # 初回
scripts/container.sh build                                                   # 新しい lock を vendor に取り込む
```

`.work/cargo` に registry の cache が残っても、置き換えが有効な間は参照されない。`make audit` (cargo-deny) は yank の確認に crates.io の index を要するため、置き換えの無い `.work/cargo-online` を `CARGO_HOME` として実行する。

## 固定の更新

- nixpkgs: `flake.nix` の rev を変更し、`scripts/container.sh lock` で `flake.lock` を再生成する
- toolchain: `nix/esp-rust.nix` と `nix/xtensa-gcc.nix` の version と sha256 を同時に変更する
- crate: `Cargo.toml` の版を変更し、前節の手順で `Cargo.lock` を更新してイメージを再構築する
- いずれも [dependencies.md](dependencies.md) の調査を行い、記録を更新してから採る
