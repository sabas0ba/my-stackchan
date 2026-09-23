# プラグイン機構 (設計案)

PC 側の情報源と、それに伴う描画・操作を本リポジトリ外の git リポジトリから追加できるようにする。本書は設計案であり、未決事項は末尾に記す。現行の構成は [design.md](design.md)、デバイスとの通信は [protocol.md](protocol.md) を参照する。

## 目的と範囲

想定する拡張の例は次のとおり。

| 例 | 取得元 | 表示 | 操作 |
| --- | --- | --- | --- |
| 時計 | host の時刻 | Card (帯) | なし |
| 通知 | 外部からの push (Claude Code の hook 等) | 通知 (Overlay または帯、TTL 付き) | 既読化 |
| Claude / Codex 使用率 | ローカルのセッションログ | Card (Bar) | 表示期間の切替 |
| 3D プリンタの進捗とカメラ | LAN 上のプリンタ | Card (Bar)、画像領域 | カメラ表示の開始・終了 |

範囲に含むもの:

- plugin と host の間のプロトコル、plugin の宣言と利用者設定
- host の常駐プロセス (以下 daemon) による画面と入力の配分
- 上記を成立させるためのデバイス側プロトコルと firmware の拡張

範囲に含まないもの:

- firmware 上でのプラグイン実行。firmware は引き続き「描画対象のデータ」だけを受け取る ([design.md](design.md#目標と制約) の制約を維持する)
- Wi-Fi/BT の有効化

## 前提条件

| 条件 | 内容 |
| --- | --- |
| 実行環境 | daemon と plugin は Windows ネイティブと Linux (WSL、コンテナを含む) の両方で動作する。OS 固有の機能を必須としない |
| 依存 | Rust の crate は増やさない。既存の `serde`、`postcard`、`heapless`、`clap`、`serialport` と std で構成する |
| 隔離 | OS による隔離は必須としない。plugin の権限は daemon がプロトコル上で制限できる範囲 (表示、通知、表情) に限って強制する |
| plugin の作者 | 主に利用者自身。第三者製もあり得るが、導入前に利用者がソースを確認し、commit SHA で固定することを前提とする |

## 現状との差分

現行の実装を前提にすると、次の 4 点が不足している。

| 項目 | 現状 | 必要な変更 |
| --- | --- | --- |
| host の実行形態 | `stackchan` は 1 回の送信ごとに port を開いて終了する CLI | port を占有する daemon を置き、複数の plugin からの表示要求を調停する。daemon の動作中、既存 CLI の表示命令は daemon 経由で送る |
| デバイスからの入力 | `Reply` は Pong / Ack / Rejected のみ。タップは firmware 内の表情デモに閉じている | タップを host へ通知する `Event` を追加する |
| 画像 | Card あたり 16×16 px、フレーム 1024 byte 以内 | 矩形領域へ行単位で分割転送する画像メッセージを追加する |
| Card の識別 | Slot を上書きするだけで、表示中の Card の出所を持たない | Card に host が採番する `card_id` を持たせ、入力を発生元の plugin へ戻せるようにする |

## 構成

```
plugin (子プロセス)               host daemon                           CoreS3 (firmware)
+------------------+  stdio     +------------------------------+  USB CDC  +-------------------+
| 取得・整形        | postcard   | plugin 管理 (起動・監視・再起動) |  COBS     | codec / validate  |
|                  | + COBS     | 検証・流量制限                 | <-------> | model (slot)      |
+------------------+ <--------> | scheduler (slot の配分)       |           | renderer          |
                                | 入力の振分け                   |           | touch -> Event    |
stackchan CLI ---> spool dir -> | 受付 (通知、手動の表示命令)     |           +-------------------+
                                +------------------------------+
```

- plugin はデバイスに直接接続しない。port は daemon だけが開く
- plugin プロトコルとデバイスプロトコルは分離し、daemon が変換する。firmware 側の変更 (版の更新、上限値の変更) を plugin へ波及させないためである
- daemon は plugin から受けた内容を `protocol` crate の検証に通してから送る。不正な内容は plugin 側へエラーとして返し、デバイスには送らない
- daemon は USB の読取りを 1 スレッドに集約し、要求への応答 (Pong / Ack / Rejected) と非同期の `Event` を振り分ける
- daemon は接続を保持し続けるため、`Ping` の nonce を送信ごとに変え、再接続前の古い応答と区別する。現行 CLI の nonce は固定値であり、1 回の送信で終了する用途に限って成り立つ
- pitch trim は現行 CLI と同じ保存先 (`--pitch-trim-file`、`STACKCHAN_PITCH_TRIM_FILE`、既定は `.work/pitch-trim.txt`) から読み、接続時と、firmware の再起動を検出した時 (Ack の seq が巻き戻った時、または再接続時) に再適用する
- `HardwareProbe` と `PitchTrim` は保守用の命令であり、plugin には開放しない

### crate 構成

| crate | 役割 |
| --- | --- |
| `crates/protocol` | 既存。デバイスプロトコル |
| `crates/plugin-api` | plugin プロトコルの型、版、上限値、フレームの読み書き。`client` module は Rust で plugin を書くための接続補助 |
| `crates/host` | 既存 CLI に daemon (`stackchan daemon`)、利用者設定の読込み (`stackchan config`) を追加する |
| `plugins/clock` | 本リポジトリ内の参照実装。API の検証に用いる |

plugin を書くための補助は crate を分けず `plugin-api` に含める。crate 数を抑え、型と補助の版を一致させるためである。plugin の試験には、pipe で接続した daemon 側を模擬する方法を用いる (`crates/plugin-api/src/client.rs` と `crates/host/src/daemon/tests.rs` の試験を参照)。

非同期ランタイムは導入せず、plugin ごとの読み書きは std のスレッドとチャネルで扱う。依存を増やさないためである。想定する plugin 数 (10 未満) ではスレッド数が問題にならない。

## 信頼境界と権限

plugin は daemon と同じ利用者権限で動く子プロセスであり、ファイルやネットワークへのアクセスを daemon は制限しない。Windows と Linux に共通する隔離手段を依存なしで用意できないため、OS による隔離は必須としない。導入時のソース確認と commit SHA による固定を、第三者製 plugin に対する主な対策とする。

daemon が強制する範囲:

| 対象 | 方法 |
| --- | --- |
| 表示 | plugin が `hello` で宣言し、利用者設定で許可した capability (Card 数、通知、表情、画像、機構部) 以外のメッセージを拒否する |
| 機構部 | `Emote` の `intensity` はサーボと LED を駆動する。capability `motion` を許可していない plugin の Emote は、daemon が `intensity` を 0 に置き換えて送る (画面上の表情だけが変わる) |
| 内容 | 上限値と `protocol` の検証。違反は `Rejected` として plugin へ返す |
| 流量 | 1 メッセージの長さ (64 KiB) と秒間件数の上限。超過した plugin は停止する |
| 異常終了 | 指数的な待ち時間で再起動し、連続して失敗したものは無効化する |

### 任意の隔離

Linux では、利用者設定で plugin ごとに起動コマンドの前置き (例: `podman run --rm -i --read-only --cap-drop=all --network=none ...`) を指定できるようにする。daemon は stdio を用いるだけなので、前置きの内容には関知しない。隔離の構成は利用者の選択とし、本リポジトリでは例を示すにとどめる。

### secret

アクセスコードやトークンは利用者設定側 (他の利用者から読めないファイル) に置き、daemon が起動後の `init` メッセージで plugin の stdin へ渡す。環境変数はプロセス一覧や隔離基盤の設定から参照できる場合があるため用いない。

## plugin の配布と固定

plugin の取得と構築は、daemon ではなく手順と補助スクリプトで行う。daemon にネットワーク取得と構築の機能を持たせないためである。

1. 利用者が plugin のリポジトリを指定の commit SHA で取得し、ソースを確認する
2. plugin 側の `Cargo.lock` を用いて `cargo build --locked --release` で構築する
3. 利用者設定に実行ファイルの path と、取得した commit SHA を記録する

daemon は記録された commit SHA を `hello` の内容とともにログへ出力する。実行ファイルと commit SHA の対応の検証 (再現可能な構築とハッシュの照合) は後続の課題とする。

### 利用者設定

設定は行単位の簡易な形式とし、パーサは `crates/host/src/config.rs` に自前で実装する。TOML や JSON の crate を追加しないためである。受け付ける構文は、section 見出し、`key = 値` の行、`#` で始まるコメント行に限る。値は `"文字列"` (エスケープは `\"` と `\` のみ)、整数、`true` / `false`、`["文字列", ...]` のいずれかとする。未知の key、重複した key と section は誤りとし、ファイル名と行番号を示す。

```
# 例: <設定ディレクトリ>/stackchan.conf
[daemon]
port = "COM7"        # 省略時は VID:PID から自動検出する
rotate_s = 10        # 帯の巡回と送り直しの間隔 (1..3600 秒)

[plugin clock]
command = ["C:/Users/<user>/stackchan-plugins/stackchan-clock.exe"]
rev = "<40 桁の commit SHA>"
cards = 1            # 同時に持てる Card の数 (0..8)
notify = false
presence = false
motion = false
param.utc_offset_minutes = "540"
```

- `command` は argv である。Linux では隔離コマンドを前置きできる (例: `["podman", "run", "--rm", "-i", ...]`)
- `cards` / `notify` / `presence` / `motion` は許可の上限であり、plugin が `Hello` で要求したものとの共通部分を `Init` で通知する。省略時はすべて不許可とする
- `param.*` は `Init` で plugin に渡す値である。path 等の環境依存の値は plugin に既定値を持たせず、利用者設定から与える

secret は同じディレクトリの `secrets.conf` に、同じ構文の `[plugin <id>]` と `param.*` だけで書く。`stackchan.conf` を共有・版管理しても secret が混入しないようにするためである。`secrets.conf` から plugin や権限を追加することはできない。`stackchan config` は設定を検証して要約を表示し、`param` は名前だけを表示する。

### 設定ディレクトリ

利用者設定と spool は同じ設定ディレクトリに置く。既定値は次のとおりとし、CLI の全サブコマンド共通の `--config-dir <path>` で起動時に変更できる。daemon と、daemon へ要求を送る CLI は同じ設定ディレクトリを指定する必要がある。

| OS | 既定値 |
| --- | --- |
| Windows | `%APPDATA%\stackchan` |
| Linux | `$XDG_CONFIG_HOME/stackchan`。未設定なら `$HOME/.config/stackchan` |

std にはこれらを解決する API が無いため、環境変数から自前で求める。環境変数が無い、または絶対 path でない場合は既定値を持たないものとしてエラーにし、`--config-dir` の指定を求める。

## plugin プロトコル

stdio 上で、デバイスプロトコルと同じ postcard + COBS (0x00 終端) を用いる。既存の依存だけで実装でき、daemon 側の復号処理とその検査方法 (固定 seed の変異入力) をデバイスプロトコルと共用できるためである。stderr は daemon がログとして記録する。

postcard の wire format は公開された仕様があり、Rust 以外でも実装できる。ただし本リポジトリが提供する SDK は Rust のみとする。

plugin-api の型は host 側でのみ使うため、`String` / `Vec` を用いる。確保量は COBS フレームの上限 (64 KiB) で先に制限し、復号後に要素ごとの上限を検証する。

### plugin -> daemon

| type | 内容 |
| --- | --- |
| `Hello` | `api_version`、plugin の名前と版、要求する capability。最初のメッセージでなければならない |
| `CardPut` | 論理 Card ID (plugin 内で一意)、内容、希望する slot 種別、優先度、TTL、行ごとの action ID |
| `CardRemove` | 論理 Card ID |
| `Notify` | 短文、優先度、TTL。capability `notify` が必要 |
| `Presence` | 表情・活動状態の要求。capability `presence` が必要。採否は daemon が決める |
| `Emote` | 一時的な表情と視線の要求。capability `presence` が必要。機構部の駆動には加えて `motion` が必要 |
| `Log` | 診断用の文字列 |

### daemon -> plugin

| type | 内容 |
| --- | --- |
| `Init` | 許可された capability、利用者設定の `param.*` (secret を含む)、表示可能な行数と文字列長の上限 |
| `Visibility` | 論理 Card が表示中か否か。カメラ等は表示中だけ取得することで負荷を下げる |
| `Action` | 論理 Card ID と action ID。タップの結果 |
| `Rejected` | 検証に失敗したメッセージと理由 |
| `Shutdown` | 終了要求。一定時間内に終了しなければ停止する |

Card の識別子は plugin ごとに 0..7 とする。Card の内容はデバイスプロトコルの Card と同じ要素 (Text / Bar / Spacer) で表し、帯は 2 行、Overlay は 4 行までとする。Overlay の Card は TTL を必須とする。期限の無い Overlay は顔を隠し続けるためである。

画像 (`ImageFrame`) は P4 で variant の末尾に追加する。画像は RGB565 で受け取り、daemon に画像 codec を持たせないため、デコードと縮小は plugin 側で行う。

`plugin-api` の検証は大きさの一般的な上限 (文字列 256 byte、8 行 × 4 要素) だけを扱い、デバイスに収まるかは daemon が変換時に検証する。plugin API の版をデバイスの上限値の変更から切り離すためである。

### 通知と手動命令の受付

hook やスクリプトからの単発の通知は `stackchan notify` で送る。daemon の動作中、CLI は spool ディレクトリ (利用者の設定ディレクトリ配下) に 1 件 1 ファイルで要求を書き、daemon が短い間隔で取り込む。

- local socket ではなく spool ディレクトリとするのは、Windows と Linux の両方で std だけで実装でき、アクセス制御を OS のファイル権限に委ねられるためである
- CLI は一時ファイルに書いてから rename し、daemon が書きかけのファイルを読まないようにする
- 取込み時にファイルの大きさを制限し、処理後に削除する。受付からの通知は `push` という組込み plugin からの `Notify` として扱い、同じ検証と流量制限を適用する
- 既存の `text` / `card` / `face` / `status` / `clear` も、daemon の動作中は同じ経路で送る

受付は P3 で実装する。それまでは daemon が port を占有するため、daemon の動作中に CLI の表示命令を実行すると port を開けずに失敗する。

## 画面と入力の配分

### slot と scheduler

firmware の slot (BannerTop / BannerBottom / Overlay) はそのまま使い、daemon の scheduler が plugin の Card を割り当てる。

| 種別 | 割当て先 | 規則 |
| --- | --- | --- |
| 常設 Card (時計、使用率) | 帯 | 帯ごとに候補を一定間隔で巡回する。利用者設定で固定表示も可能 |
| 通知 | 帯または Overlay | 優先度が高いものは巡回に割り込む。TTL 満了または既読化で巡回に戻る |
| 画像 (カメラ) | 画像領域 (Overlay 内) | 利用者の操作で開始し、操作または一定時間で終了する |
| presence / emote | 顔と機構部 | 手動の `status` / `face` / `emote` が最優先。plugin からの要求は許可されたものだけを短い TTL で採用する。Emote は連続送信で上書きされるため、plugin ごとに最短間隔を設ける |

firmware 内の表示期限 (TTL) は保険として残し、daemon は表示を切り替えるたびに Card を送り直す。daemon が停止しても古い表示が残らないようにするためである。

- 帯の Card は 2 枚ずつ上下に置き、`rotate_s` ごとに次の組へ進める。優先度の高い順、同順位は設定順と Card 識別子の順に並べる
- 高優先度以外の通知がある間は、下の帯を通知に、上の帯を Card 1 枚ずつの巡回に使う。高優先度の通知は Overlay に置く
- firmware 側の TTL は「表示内容の残り時間 (切上げ)」と「`rotate_s` + 5 秒」の短い方とし、`rotate_s` ごとに送り直す
- 表示を消す場合、現行のデバイスプロトコルには slot 単位の消去が無いため、空の Text を TTL 1 秒で送る。1 秒以内に firmware 側の期限で消える表示には送らない。slot 単位の消去は版 8 で追加を検討する

### 入力

- Card の各行は任意で action ID を持つ。firmware はタップ位置を表示中の行と照合し、`Event::Tap { card_id, action }` を返す。行外のタップは `card_id` と `action` を持たない Tap とし、daemon の scheduler が巡回の送り等に使う
- 座標をそのまま plugin に渡さない。表示位置が変わっても plugin の実装が影響を受けないようにするためである

### タップの扱いの切替

firmware はタップの扱いとして `Demo` (現行の表情デモ) と `Forward` (host へ Event を送る) の 2 つの状態を持つ。切替は CLI (`stackchan input demo|forward`) で明示的に行い、daemon は自動では切り替えない。

- 起動時は `Demo` とする。host が接続されていなくても単体で動作を確認できる現行の挙動を保つためである
- 状態は RAM 上にのみ保持し、再起動で `Demo` に戻る。firmware に設定の永続化を持たせない方針 ([design.md](design.md#目標と制約)) に従う
- `Forward` の間は、タップによる firmware 内の動作 (表情デモの進行、Emote の解除) を行わず、Event の送信だけを行う。タップへの反応は daemon と plugin が決める
- `Forward` の間も Event の送信先が無ければ破棄するだけで、表示には影響しない

## デバイスプロトコルの拡張

版を 7 から 8 に上げる。既存 variant の番号は維持する。

| 追加 | 内容 | 上限の考え方 |
| --- | --- | --- |
| `Card.card_id: u16` と `Row.action: Option<u8>` | Card の識別と行単位の操作領域 | 既存の Card 検証を拡張する |
| `Message::InputMode(Demo \| Forward)` | タップの扱いの切替 | 揮発。Ack を返す |
| `Message::ImageBegin { id, x, y, w, h }` | 画像領域の開始。領域は Overlay 内 | 最大 320×240 |
| `Message::ImageRows { id, y, rows }` | RGB565 の行の組。1 フレーム 1024 byte 以内に収まる行数 | 320 px なら 1 フレーム 1 行 |
| `Message::ImageEnd { id }` | 画像の完了 | - |
| `Reply::Event(Event)` | タップ等の入力。host の要求とは非同期に送る | Event 用の送信待ちは 1 件とし、あふれた場合は古いものを捨てて破棄数を数える |

画像はフレームバッファを持たず、受信した行をそのまま LCD の対応位置へ書く。動的確保を行わない方針を維持するためである。フレーム間で画面が部分的に更新されることは許容する (低い更新頻度を前提とする)。160×120 px の画像は約 38 KB であり、1 fps 以下の更新を想定する。

## 検証

| 対象 | 方法 |
| --- | --- |
| plugin プロトコル | 型の往復、上限ちょうど・超過、不正フレーム、`Hello` 前のメッセージの拒否。`decoder_stress` と同様の固定 seed の変異入力 |
| 利用者設定のパーサ | 正常系、構文エラー、未知・重複 key、過長行、任意バイト列。固定 seed の変異入力を含める |
| daemon | mock plugin を用いた結合テスト。異常終了からの再起動、流量超過での停止、許可外 capability の拒否、spool の書きかけファイルの無視 |
| デバイスプロトコル | 既存の単体テストと `decoder_stress` に新 variant を追加する |
| 描画 | `firmware/examples/simulate.rs` に Card の巡回、通知の割込み、画像領域の状態を追加し、画像で比較する |
| 対象 OS | 単体・結合テストはコンテナ内 (Linux) で実行する。Windows 向けはコンテナ内の cross build が通ることを `make check` で確認し、実行時の動作は Windows 上で手動の確認手順により確かめる |
| plugin 作者向け | pipe で接続した daemon 側の模擬で、plugin 単体の出力を CI で検査できるようにする |

## 段階

| 段階 | 内容 | 受入条件 |
| --- | --- | --- |
| P0 | 本書の確定 | 未決事項の決定 |
| P1 | `plugin-api`、daemon、利用者設定、`plugins/clock` | 時計が帯に表示され続け、plugin の異常終了から復帰する。Windows と Linux で動作する |
| P2 | デバイスプロトコル版 8 (card_id、InputMode、Event)、scheduler、入力の振分け | タップが発生元の plugin に届く。複数 Card が巡回する |
| P3 | spool による通知の受付、別リポジトリの plugin の導入手順 | commit SHA で固定した外部 plugin が許可した capability の範囲で動作する |
| P4 | 画像領域 | 外部 plugin からの画像が Overlay に表示される |

Claude / Codex 使用率は P3 の最初の外部 plugin とする。design.md の collector (Phase 3) は本機構の plugin として実装し、host への組込みは行わない。取得元のログ形式に依存する部分をリポジトリ外へ分離するためである。

## plugin ごとの留意点

- Claude / Codex 使用率: ログ形式、およびサブスクリプションの残量を取得する公式手段の有無は、着手時に一次情報で確認する
- 3D プリンタ: LAN 経由の制御・映像取得には公式仕様ではなく第三者の解析に依拠するものがあり、プリンタの firmware 更新により第三者のアクセスが制限される場合もある。利用条件の確認を含め plugin 側の責務とし、本リポジトリにはプリンタ固有のコードを置かない
- 通知: OS の通知を横取りする方式は OS ごとの制約が大きいため、第一段階では spool 経由の受付で受ける

## 決定事項

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 隔離 | OS による隔離は必須としない。Linux では起動コマンドの前置きで任意に隔離できるようにする | Windows と Linux に共通する手段を依存なしで用意できない |
| 形式 | plugin プロトコルは postcard + COBS、利用者設定は自前パーサの簡易形式 | Rust の依存を増やさない |
| 実行環境 | Windows ネイティブを対象から外さない。local socket の代わりに spool ディレクトリを用いる | std だけで両 OS に対応できる |
| タップの切替 | CLI で明示的に切り替える。起動時は Demo | 単体での動作確認を保ち、daemon の暗黙の状態変更を避ける |
| 設定ディレクトリ | OS ごとの既定値を持ち、`--config-dir` で起動時に変更できる | 複数の構成の併用と試験での分離 |
| Windows 向けの構築 | コンテナ内で `x86_64-pc-windows-gnu` 向けに cross build する | ホストに toolchain を導入しない規約を保つ |

## Windows 向けの cross build

host 側の crate は Espressif fork の toolchain (nix/esp-rust.nix) で構築している。上流の `rust-std-x86_64-pc-windows-gnu` は compiler の commit が異なり、この toolchain では使えない。そのため次の構成とした (2026-09-23 に既存 CLI で成立を確認)。

- 標準ライブラリは同梱の rust-src から `-Z build-std` で構築する (firmware と同じ方式)
- linker と C runtime は nixpkgs の mingw-w64 cross toolchain を用いる。版は既存の nixpkgs の rev で固定される
- Windows 向けで新たにコンパイル対象となる crate (`windows-sys` 等) の調査は [dependencies.md](dependencies.md#windows-向け-host-cli-の追加調査-2026-09-23) に記録した

手順は [environment.md](environment.md#windows-ネイティブの-host-cli) を参照する。

## 未決事項

1. 実行ファイルと commit SHA の対応を検証する方法 (再現可能な構築、ハッシュの照合)。P3 で扱う
