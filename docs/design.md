# 設計

M5Stack CoreS3 (Stack-chan) に顔・テキスト・画像を表示し、PC から USB 経由で情報を流し込む。firmware と host CLI をともに Rust で実装し、プロトコル定義を両者で共有する。

## 目標と制約

- 依存は最小限とし、すべて一意に固定する ([dependencies.md](dependencies.md))
- firmware は host からの入力を「描画対象のデータ」としてのみ扱い、コマンド実行、ファイルシステムへの書込、設定変更、再起動等の操作系を持たない
- 任意のバイト列、任意長のテキスト、不正な画像ヘッダが流入しても firmware が停止・破損しない
- Wi-Fi と BT は許可するまで有効化しない。有効化する場合も transport 層の差し替えとして行い、描画側のコードに影響させない
- 「顔」は自前の図形描画とし、第三者のアセットを取り込まない

## 構成

```
PC (host)                              CoreS3 (firmware)
+-------------------------+   USB CDC   +------------------------------+
| collector (使用量等の取得) |  COBS frame | transport (USB Serial/JTAG)  |  <- 将来 Wi-Fi/BT に差替
| card builder (レイアウト) | ----------> | codec: decode + validate     |  <- protocol crate (共有)
| sender (CLI)            | <---------- | model: slot ごとの表示内容    |  <- 検証済みデータのみ保持
+-------------------------+  Reply      | renderer: face / text / image|
                                        +------------------------------+
```

| crate | 役割 | 環境 |
| --- | --- | --- |
| `crates/protocol` | メッセージ型、上限値、encode/decode、検証。fuzz と単体テストの対象 | no_std (host では `std` feature) |
| `crates/host` | CLI。port の検出、Card の組み立て、送信、collector | std |
| `crates/fontgen` | GNU Unifont の `.hex` から埋め込み用ビットマップを生成する | std (build 時のみ) |
| `firmware/` | esp-hal ベースの firmware。独立した workspace | no_std, `xtensa-esp32s3-none-elf` |

firmware を別 workspace にしているのは、target と `build-std` の設定が host と異なり、同一 workspace では `cargo build` の既定動作が衝突するためである。

## Firmware

### 選択: esp-hal (no_std)

`esp-idf-hal` (std) ではなく `esp-hal` (no_std) を採用する。

- ESP-IDF の C コードとビルド時ダウンロードを持たず、依存が Cargo.lock と Nix の固定で閉じる
- Wi-Fi/BT のコードは `esp-radio` を依存に入れない限りリンクされない。「許可まで動かさない」を build flag ではなく依存の有無で担保する
- 代償として、電源 IC (AXP2101)、GPIO 拡張 (AW9523)、LCD (ILI9342C) の初期化を自前で記述する。レジスタの値は各データシートと M5Stack の回路図を一次情報とする

### ペリフェラル (CoreS3)

| 部品 | 接続 | 用途 |
| --- | --- | --- |
| ILI9342C (320x240) | SPI | 表示。`mipidsi` crate の ILI9342C model を使用 |
| AXP2101 | I2C | 電源。LCD バックライトの電源制御を含む |
| AW9523 | I2C | GPIO 拡張。LCD リセット等 |
| USB Serial/JTAG | ESP32-S3 内蔵 | host との通信。書込と共用 |
| UART0 | GPIO | ログ出力 (panic、backtrace)。USB とは分離する |

### CoreS3 の表示系接続

2026-09-07 に M5Stack の公式資料と公式実装を照合した。参照した版は次のとおり。

- [CoreS3 公式資料](https://docs.m5stack.com/en/core/CoreS3)（製品ページおよび
  [回路図 v1.0](https://m5stack-doc.oss-cn-shenzhen.aliyuncs.com/490/Sch_M5_CoreS3_v1.0.pdf)）
- [M5GFX `d91077b9`](https://github.com/m5stack/M5GFX/blob/d91077b9a607b59404e4e4a49f775c792bfae382/src/M5GFX.cpp)
- [M5Unified `8530f537`](https://github.com/m5stack/M5Unified/blob/8530f5377d782e4a25a6c482de2e71c3f75ca8eb/src/utility/Power_Class.cpp)

| 信号 | 接続先 | 備考 |
| --- | --- | --- |
| 内部 I2C SCL / SDA | GPIO11 / GPIO12 | AXP2101 と AW9523B を 400 kHz で制御する |
| LCD SPI SCK / MOSI | GPIO36 / GPIO37 | LCD と microSD がバスを共有する |
| LCD CS | GPIO3 | active low |
| LCD D/C | GPIO35 | microSD の MISO と兼用するため、LCD 選択中だけ出力として扱う |
| LCD reset | AW9523B (0x58) P1_1 | AW9523B の Port 1 output register (0x03) で制御する |
| LCD backlight | AXP2101 (0x34) DLDO1 | enable は register 0x90 bit 7、電圧は register 0x99 で設定する |

Phase 1 では内部 I2C を先に初期化し、DLDO1 を消灯したまま電圧を設定し、LCD reset、SPI、初期描画の後に点灯する。
GPIO35 は LCD の D/C と microSD の MISO に共有されるため、将来 microSD を使用する場合も
同時に駆動しない。M5GFX は LCD の CS 操作に合わせて GPIO35 の入出力を切り替えている。

M5Unified の AXP2101 初期化にはカメラ、音声、microSD 等の電源設定も含まれる。本 firmware
では未使用回路を有効にせず、Phase 1 で表示に必要な DLDO1 だけを設定する。Wi-Fi/BT は
この初期化とは独立しており、引き続き依存も機能も追加しない。

### Phase 1 の表示確認画面

起動時に 320×240 の黒背景へ、白い外枠、自前の図形による顔、ASCII 文字列、96×16 の RGB565 画像を描画する。画像は赤・緑・青・白のカラーバーを big-endian の画素列としてコンパイル時に生成する。フレームバッファと動的確保は使わず、SPI 転送用に 512 byte のバッファを使う。

内部 I2C は 400 kHz、LCD SPI は mode 0 / 40 MHz とする。AXP2101 は 0x90 bit 7 と 0x99 bits 4:0 のみを更新し、DLDO1 を 2.8 V に設定する。AW9523B は Port 1 の出力ラッチ (0x03)、GPIO/LED mode (0x13)、方向 (0x05) の P1_1 のみを更新する。リセットは Low 20 ms、解除後 120 ms とし、その他のビットは読み出した値を保存する。

GPIO35 の D/C は出力ラッチを先に設定し、LCD CS が Low の期間だけ出力を有効にする。SPI の転送完了後に出力を無効化してから CS を High に戻す。microSD CS (GPIO4) は High に保持する。Phase 1 は microSD カードを取り外した状態で検証する。カードの利用には、SD mode から SPI mode への移行とバスの仲裁を別途実装する必要がある。

LCD は固定済み `mipidsi 0.10.0` の `ILI9342CRgb565` を使用し、BGR 順序・色反転ありで初期化する。実機で画面方向、色順、初期化の成立を確認するまで Phase 1 の受入完了とはしない。表示系の I2C/SPI エラーは UART0 の panic 出力で識別し、初期描画が失敗した場合は点灯処理へ進まない。

実装時の参照資料は [CoreS3 公式 PinMap](https://docs.m5stack.com/en/core/CoreS3)、前節で固定した M5GFX の `Light_M5StackCoreS3` と AW9523B 初期化、および [mipidsi 0.10.0 の Builder](https://docs.rs/mipidsi/0.10.0/mipidsi/struct.Builder.html)。実機確認手順は [environment.md](environment.md#phase-1-の実機確認) に記載する。

### ログと通信の分離

USB Serial/JTAG はプロトコル専用とし、`esp-println` の出力先は UART0 に固定する。ログがプロトコルのストリームに混入すると host 側の同期が乱れるためである。

## プロトコル

詳細は [protocol.md](protocol.md)。要点:

- シリアライズは `postcard` (serde 互換、no_std)。フレーミングは COBS で、0x00 をフレーム終端とする
- 可変長データは `heapless` の上限付き型で表す。上限を超える入力は復号の時点で失敗する
- firmware の受信は固定長バッファ (`MAX_FRAME_BYTES`) と状態機械で処理し、動的確保を行わない。不正なフレームは破棄し、カウンタに記録する
- firmware から host へは `Reply` (Pong / Ack / Rejected) のみを返す
- USB では物理接続を信頼境界とし、認証は行わない。Wi-Fi/BT 化する場合は transport 層で事前共有鍵と HMAC を追加する

## 表示モデル (Card)

HTML そのものを firmware で解釈することはしない。要素数、入れ子深さ、文字列長を固定した宣言的な Card を定義する。

- Slot: `BannerTop` / `BannerBottom` (帯) / `Overlay` (全画面)。顔は常駐し、Overlay の間だけ隠れる
- TTL: 各 Card は秒単位の TTL を持ち、経過後に自動的に消える
- 要素: 現行版は `Text` (str) / `Bar` (ratio, label) / `Spacer` / `Image` (Card あたり 1 枚の inline RGB565)。style / align と画像 ID は後続作業
- 構造: Column -> Row -> 要素 の 2 段に限定し、要素数に上限を設ける
- 振分け: 短文なら Banner、長文や画像なら Overlay、といった判断は host 側で行う。firmware は受け取った Card を検証して配置するだけとする

## フォント

日本語を表示するため、ビットマップフォントを firmware に埋め込む。

- 元データは GNU Unifont の `.hex` 形式 (OFL 1.1 と GPLv2+ (font embedding exception 付) の dual license)。行単位のテキストであり、解析に外部 crate を要しない
- `crates/fontgen` が ASCII と JIS X 0208 の部分集合を選び、16 px のビットマップ配列として出力する。約 7,000 字で 220 KB 程度
- 収録外の文字は host 側で置換文字に落とすか、host でラスタライズした画像として送る
- Unifont の配布物も sha256 で固定する

## 活動状態と表情

host の `status` は PC 側から Idle / Working / Waiting / Done / Error と短い詳細を送信する。
活動状態から表情を決め、`face` は表情・視線・目の開き方を直接指定する。いずれも一つの Presence
メッセージとして受理・描画され、表示期限が切れると既定の顔に戻る。現段階では
PC 側の状態取得は手動 CLI またはスクリプトからの呼び出しとし、特定のログ形式への
依存を持たせない。収集器は次段階で追加する。

表情は標準の白い目と口を基準に 12 種類実装する。画像生成で検討した
[表情案](assets/expression-concepts.png) は設計参考であり、実際の表示は追加画像を持たず
`embedded-graphics` の図形で構成する。生成時の条件は「標準の顔を参照し、黒背景に白い目と口だけを
配置した 4×3 の表情案。黒目・灰色・文字を使わず、320×240 の小型画面で再現できる線と円」とした。
視線は白い目全体を上下左右へ移動し、開閉状態は表情とは独立に指定できる。

画面上の視線は CoreS3 単体で動作する。物理的な首振りは、接続される機構の種類、
ピン割当て、電源、可動域を確認してから追加する。

## 使用量の取得 (collector)

認証を持たない。Claude Code と Codex がローカルに残すセッションログの読み取り専用集計を第一候補とし、取得できなければ表示しない。取得元は `trait Source` として差し替え可能にする。具体的な取得元は Phase 3 の着手時に確定する。

## フェーズと受入条件

現在は Phase 2 の Text / Card / Clear、16×16 px までの inline RGB565 画像、Slot ごとの TTL、
USB 経由の描画完了応答まで実装している。decoder は固定 seed の変異入力を `make check` で検査する。
coverage-guided fuzz は後続作業とする。操作手順は
[environment.md](environment.md#text--clear-の実機確認)、現行の表示仕様は [protocol.md](protocol.md) を参照する。

実機との表示比較には [表示シミュレーション](environment.md#表示シミュレーション)を利用できる。
`firmware` のモデルと描画器を host 上で直接実行し、起動時・帯表示・Overlay 中・TTL 満了後を
Card と RGB565 画像を含めて画像化する。シミュレーター側には表示配置を複製せず、画素を BMP へ保存する描画先だけを置く。

| Phase | 内容 | 受入条件 |
| --- | --- | --- |
| 0 | 環境整備 | `scripts/container.sh check` が `--network none` で成功する。firmware の最小構成が build できる |
| 1 | 表示 | CoreS3 で自前の顔、ASCII テキスト、RGB565 画像を表示できる |
| 2 | プロトコル + host CLI | USB 経由で Card を表示できる。protocol の decoder が fuzz テストを通過する |
| 3 | collector | ローカルログの集計を定期送信できる。取得失敗時に表示を汚さない |
| 4 | 日本語フォント | fontgen の出力で日本語の Card を表示できる |
