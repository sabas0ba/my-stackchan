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

### 表示シミュレーション

実機や USB 接続なしで、firmware と同じ `Controller`・`renderer` を PC 上で実行する。
リポジトリのルートから既存の Nix コンテナを使う。

```bash
scripts/container.sh run make simulate
```

`.work/simulation/index.html` をブラウザーで開くと、以下の 6 状態を並べて確認できる。
対応する 320×240 の BMP 画像も同じディレクトリに保存する。

1. 起動確認画面 (`01-startup.bmp`)
2. 上下の帯と顔 (`02-banners.bmp`)
3. Overlay 表示直後 (`03-overlay.bmp`)
4. TTL 満了時に復帰した上下の帯と顔 (`04-expired.bmp`)
5. Card のタイトルと比率バー (`05-card.bmp`)
6. Card 内の 16×16 px RGB565 画像 (`06-image.bmp`)

文言・TTL・Card のタイトルと比率・出力先は指定できる。`--ttl` は 1〜65535 秒、
Text の文言は UTF-8 で 512 byte まで、Card のタイトルは 48 byte まで、比率は 0〜100 とする。
各画像の時刻は実際の待機時間ではなく、
単調時計の入力を 0 ms と TTL 境界に設定して再現する。

```bash
scripts/container.sh run cargo run --manifest-path firmware/Cargo.toml \
  --example simulate --locked --offline -- \
  --out .work/simulation --top 'USB Text OK' --bottom 'BannerBottom OK' \
  --overlay 'Overlay test: 30s' --ttl 30 \
  --card-title CPU --card-ratio 75
```

BMP は描画処理が出力した RGB565 を RGB888 に展開した結果である。
LCD パネル固有の色順・反転・SPI 転送・USB 通信の成否は再現しないため、
実機の表示・通信確認は従来の手順で行う。

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

### Text / Clear の実機確認

Text / Clear 対応 firmware を書き込んでから、既存コンテナで host を build する。

```bash
scripts/container.sh run cargo build --locked --offline -p my-stackchan-host
scripts/container.sh device target/debug/stackchan text --slot top --ttl 0 'USB Text OK'
scripts/container.sh device target/debug/stackchan text --slot bottom --ttl 0 'BannerBottom'
scripts/container.sh device target/debug/stackchan text --slot overlay --ttl 5 'Overlay: expires in 5s'
scripts/container.sh device target/debug/stackchan clear
```

Overlay 中は顔と帯が隠れ、5 秒後に両方の帯と顔が復帰することを確認する。
Clear はこの復帰を確認してから実行し、顔のみの表示になることを確認する。
各コマンドは送信前に Ping で版を照合し、描画後の Ack を表示する。`--port` 省略時は
VID:PID から自動検出する。明示する場合は各サブコマンドへ `--port /dev/ttyACM0` を加える。

通信用の実機回帰テストは次のコマンドで実行する。最大 512 byte の Text、3 Slot、
Ack の通し番号、TTL 待機後の通信継続、Clear を確認する。テストは最後に Clear する。
Ack の受信だけでは画面や期限満了の目視確認は代替できない。

```bash
scripts/container.sh device env STACKCHAN_TEST_PORT=/dev/ttyACM0 \
  cargo test --locked --offline -p my-stackchan-host \
  -- --ignored --exact tests::hardware_text_and_clear
```

Slot ごとの期限・上書き・描画失敗・領域外描画・Overlay 解除時の復帰は通常の単体テストでも検証する。
電源再投入時は表示状態を保持せず、表示確認画面へ戻る。

### Card の実機確認

Card 対応 firmware を書き込んだ後、次の例で上帯にタイトル・比率バーを表示する。

```bash
scripts/container.sh device target/debug/stackchan card \
  --slot top --title CPU --detail '75%' --ratio 75 --label Usage
scripts/container.sh device target/debug/stackchan card \
  --slot overlay --ttl 5 --title Status --detail 'Card OK' --ratio 50 --space 8
```

Overlay の期限満了後、上帯の Card が再表示される。表示を消す場合は `clear` を実行する。
Card は送信前に host で行数・要素数・合計高さ・文字列長・比率を検証し、firmware も
同じ制約を再検証する。通信用の実機回帰テストは次で実行する。無効な比率の拒否、
Ack の通し番号、Overlay の TTL、Clear 後の通信を確認する。終了時に Clear する。

```bash
scripts/container.sh device env STACKCHAN_TEST_PORT=/dev/ttyACM0 \
  cargo test --locked --offline -p my-stackchan-host \
  -- --ignored --exact tests::hardware_card_and_invalid_ratio
```

Ack と単体テストだけでは Card の画面表示と期限満了の目視確認は代替できない。

### RGB565 画像の実機確認

`card --image-bmp` は無圧縮 24-bit BMP (40 byte の BITMAPINFOHEADER) の指定範囲を
RGB565 に変換する。画像は Card あたり 1 枚、縦横とも 1〜16 px。`--image-x` と
`--image-y` は元 BMP の左上を原点とする。幅・高さの省略値は各 16 px。
シミュレーターの `06-image.bmp` の上帯からテスト画像を切り出す例:

```bash
scripts/container.sh run cargo build --locked --offline -p my-stackchan-host
scripts/container.sh device target/debug/stackchan card \
  --slot top --title RGB565 --image-bmp .work/simulation/06-image.bmp \
  --image-x 160 --image-y 6
```

上帯に RGB565 の文字と赤・緑・青・白の 16×16 px 画像が表示される。
比率バーと画像を併用する場合は、合計行高が帯の 48 px を超えるため `--slot overlay` を使う。
画像長の不一致による拒否と、512 byte の画像に最大長のテキスト 7 個を組み合わせた
フレームの受信を含む実機回帰テストは次で確認する。テストは最後に Clear する。

```bash
scripts/container.sh device env STACKCHAN_TEST_PORT=/dev/ttyACM0 \
  cargo test --locked --offline -p my-stackchan-host \
  -- --ignored --exact tests::hardware_inline_image_and_invalid_length
```

### 表情・視線と PC 状態の実機確認

版 3 の firmware を書き込んでから、次の例で表情・視線と活動状態を送る。
`status` は PC のジョブやスクリプトから呼び出せる。詳細は ASCII で表示する。
`status` の既定 TTL は 30 秒、`face` の既定 TTL は無期限である。

```bash
scripts/container.sh run cargo build --locked --offline -p my-stackchan-host
scripts/container.sh device target/debug/stackchan status \
  --activity working --detail BUILD --gaze right --eyes half-lidded --ttl 10
scripts/container.sh device target/debug/stackchan face \
  --expression surprised --gaze left --eyes wide --ttl 5
scripts/container.sh device target/debug/stackchan clear
```

`status` と `face` は現在の顔を上書きし、帯とは独立する。期限満了後は既定の顔に戻る。
`clear` は顔と帯をすべて消す。描画例は `.work/simulation/index.html` の
`07-working.bmp` から `24-eyes-half-lidded.bmp` に含まれる。

この版は画面上の視線のみを制御する。物理的な首振りは駆動機構、接続端子、可動域を
確認した後に実装する。

### 静的検査・単体テスト

```bash
make check        # nix flake check + 環境 + Rust の fmt/clippy/test
make lint         # 静的解析のみ
make fmt          # 整形
make audit        # cargo-deny による advisory と license の検査 (ネットワークを使用)
```

CI (`.github/workflows/ci.yml`) はイメージを構築し、`--network none` のコンテナ内で `make check` を実行する。

`make check` は firmware の電源制御と描画処理も host 上でテストする。対象外レジスタビットの保持、I2C エラー伝播、描画範囲、RGB565 の色順、描画エラー伝播を検証する。単独実行は開発シェル内のリポジトリルートから `cargo test --manifest-path firmware/Cargo.toml --lib --locked --offline` を使う。シミュレーターの BMP ヘッダー・色・TTL 復帰は `cargo test --manifest-path firmware/Cargo.toml --example simulate --locked --offline` で確認する。`firmware/` 内から実行すると Xtensa 用の Cargo 設定が適用されるため、host テストではルートから実行する。

`make check` は protocol の固定 seed 変異検査を 20,000 件実行する。正常フレーム、最大長 Card、
境界長のバイト列を変異し、Message / Reply の復号と Card の検証に panic がないことを確認する。
長時間検査は次のように件数と seed を指定する。失敗時には同じ seed とケース番号が出力される。

```bash
scripts/container.sh run cargo run --locked --offline -p protocol \
  --example decoder_stress -- --cases 1000000 --seed 0x7c3a4d92b615ef08
```

firmware の単体テストは任意バイト列の後のフレーム再同期も検査する。この検査は決定的で
coverage-guided ではない。`cargo fuzz` 用の固定済み環境による検証は後続作業とする。

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
