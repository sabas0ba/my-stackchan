# プラグイン機構

PC 側の情報源と、それに伴う描画・操作を本リポジトリ外の git リポジトリから追加できるようにする。本書には決定した設計を記す。実装の状況は [段階](#段階) の表に、決まっていない点は末尾の [未決事項](#未決事項) にだけ記す。現行の構成は [design.md](design.md)、デバイスとの通信は [protocol.md](protocol.md) を参照する。

## 目的と範囲

想定する拡張の例は次のとおり。

| 例 | 取得元 | 表示 | 操作 |
| --- | --- | --- | --- |
| 時計 | host の時刻 | Card (帯) | なし |
| 通知 | 外部からの push (Claude Code の hook 等) | 通知 (Overlay または帯、TTL 付き) | 既読化 |
| Claude / Codex 使用率 | ローカルのセッションログ | Card (Bar) | 表示期間の切替 |
| 3D プリンタの進捗とカメラ | LAN 上のプリンタ | Card (Bar)、画像領域 | カメラ表示の開始・終了 |
| タイマー | デバイスの単調時計 | Card (Bar)、操作画面 | 時間の設定、開始、取消 |

plugin には実行場所の異なる 2 種がある。両者は連携できる。

| 種別 | 実行場所 | 追加の方法 | 信頼の水準 |
| --- | --- | --- | --- |
| Host 側 | PC (daemon の子プロセス、または plugin ごとのコンテナ) | 実行ファイルを構築し、利用者設定に書く | 信頼しない。daemon が権限を制限し、コンテナで隔離する |
| Client 側 | デバイス (firmware に構築時に組み込む) | firmware に登録して再構築し、書き込む | firmware の一部として信頼する。隔離できない |

本書の大半は Host 側を扱う。Client 側は [Client 側の plugin](#client-側の-plugin) に記す。

範囲に含むもの:

- plugin と host の間のプロトコル、plugin の宣言と利用者設定
- host の常駐プロセス (以下 daemon) による画面と入力の配分
- 上記を成立させるためのデバイス側プロトコルと firmware の拡張
- firmware に構築時に組み込む Client 側の plugin の枠組み

範囲に含まないもの:

- firmware が実行時にコードを読み込むこと (インタプリタ、動的な読込み)。実行基盤の依存とメモリを要し、host からの入力でデバイス上の動作を任意に変えられる経路になるためである
- Wi-Fi/BT の有効化

## 前提条件

| 条件 | 内容 |
| --- | --- |
| 実行環境 | daemon と plugin は Windows ネイティブと Linux (WSL、コンテナを含む) の両方で動作するよう実装し、OS 固有の機能を必須としない。運用は Linux 側 (コンテナ) で行う (決定事項を参照) |
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
| `plugins/image-demo` | 画像 (`ImageFrame`) の参照実装。試験模様を送る |

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
| 環境変数 | daemon の環境変数は許可したもの (`PATH`、`HOME`、`LANG`、`TZ`、一時ディレクトリ等) だけを渡す。利用者のシェルにあるトークン等を引き継がないため。その他は利用者設定の `env.*` で plugin ごとに明示する |

### コンテナでの運用時の配置 (`scripts/container.sh daemon`)

コンテナでの運用時は、plugin ごとにコンテナを分ける。plugin のコンテナには設定ディレクトリ、ログ、spool、リポジトリを渡さない。構成は [plugin ごとのコンテナ分離](#plugin-ごとのコンテナ分離) に記す。

daemon のコンテナのマウントは次のとおりである。

| マウント | 権限 | 理由 |
| --- | --- | --- |
| `/workspace` (リポジトリ) | 読取り専用。git のディレクトリは渡さない | ピッチ補正の既定の保存先を読むため。書込みは不要である |
| `/config` (設定ディレクトリ) | 読取り専用 | daemon は設定を書き換えない |
| `/config/logs`、`/config/spool` | 書込み可能 | daemon がログを書き、spool の要求を削除するため |
| `/run/stackchan/<plugin id>` (plugin ごとの volume) | 書込み可能 | plugin との接続に用いる socket を置くため |

build は書込み可能なマウントで先に行い、各コンテナは開発シェルを通さず build 済みのバイナリを直接起動する。

### 子プロセス方式で残るリスク

`--socket-dir` を指定しない場合 (Windows ネイティブ、開発時の確認)、plugin は daemon と同じ利用者権限の子プロセスとして動き、次は防げない。2026-10-05 より前はコンテナでの運用もこの方式であり、secret を扱う plugin の導入を検討する中でコンテナ分離へ改めた。

- plugin は設定ディレクトリの `secrets.conf` を読める。自分宛て以外の plugin の secret も読める
- plugin はログと spool に書ける (ログの偽装、通知の注入)
- plugin は daemon のプロセスの情報を読める場合がある (カーネルの ptrace の制限の設定に依存する)

子プロセス方式では、`command` に隔離コマンドを前置きできる (例: `podman run --rm -i --read-only --cap-drop=all --network=none ...`)。daemon は stdio を用いるだけなので、前置きの内容には関知しない。

### secret

アクセスコードやトークンは利用者設定側 (他の利用者から読めないファイル) に置き、daemon が起動後の `init` メッセージで plugin の stdin へ渡す。環境変数はプロセス一覧や隔離基盤の設定から参照できる場合があるため用いない。

- secret は `secrets.conf` の `param.*` で渡す。`command` (argv) と `env.*` には書かない。argv はプロセス一覧と `stackchan config` の表示に、環境変数はプロセスの情報に現れるため。`secrets.conf` には `env.*` を書けない
- daemon は param と env の値をログにも `stackchan config` の表示にも出さない (名前だけを出す)。`Init` の Debug 表示も値を伏せる
- spool の通知の本文と status の詳細は、hook から個人的な内容が入り得るため、ログの INFO では種類と長さだけを出し、内容は `--trace` の時だけ記録する
- plugin の標準エラーは、制御文字を無害な表記にしてからログへ転送する

plugin の作者の規約:

- param の値を標準エラー、`Log` メッセージ、Card に出さない
- 失敗の理由に、受け取った値や外部の応答の本文をそのまま含めない

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
image = false
param.utc_offset_minutes = "540"
```

- `command` は argv である。Linux では隔離コマンドを前置きできる (例: `["podman", "run", "--rm", "-i", ...]`)
- `cards` / `notify` / `presence` / `motion` / `image` は許可の上限であり、plugin が `Hello` で要求したものとの共通部分を `Init` で通知する。省略時はすべて不許可とする
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
| `ImageFrame` | 画像 1 枚 (幅、高さ、TTL、RGB565 big-endian の画素)。capability `image` が必要 |

### daemon -> plugin

| type | 内容 |
| --- | --- |
| `Init` | 許可された capability、利用者設定の `param.*` (secret を含む)、表示可能な行数・文字列長・画像の大きさの上限、中継を介して接続できる宛先 (`endpoints`) |
| `Visibility` | 論理 Card が表示中か否か。カメラ等は表示中だけ取得することで負荷を下げる |
| `Action` | 論理 Card ID と action ID。タップの結果 |
| `Rejected` | 検証に失敗したメッセージと理由 |
| `Shutdown` | 終了要求。一定時間内に終了しなければ停止する |

Card の識別子は plugin ごとに 0..7 とする。Card の内容はデバイスプロトコルの Card と同じ要素 (Text / Bar / Spacer) で表し、帯は 2 行、Overlay は 4 行までとする。Overlay の Card は TTL を必須とする。期限の無い Overlay は顔を隠し続けるためである。

画像 (`ImageFrame`) は plugin API 版 2 で追加した。画像は RGB565 で受け取り、daemon に画像 codec を持たせないため、デコードと縮小は plugin 側で行う。

- 大きさは `Init` の `limits.image_width` / `image_height` (現在 160×120 px) 以下とする。daemon は画面の中央に置く
- TTL は 0 で 10 秒とし、期限の無い画像は受け付けない。Overlay の Card と同じく顔を隠し続けないためである。連続した映像は、TTL を数秒とした画像を送り続けることで表す
- daemon が保持する画像は最新の 1 枚だけで、別の plugin の画像でも置き換える。plugin の停止時にはその plugin の画像を取り除く
- 1 枚はデバイスへ約 40 フレーム (約 40 KB) になり、転送中は daemon のイベントループが止まる (実機で約 0.3 秒)。daemon は新しい画像をデバイスへ送る間隔を 500 ms 以上とし、間隔内に届いた画像は最新の 1 枚だけを残して間隔の経過後に送る。受信時に拒否しないのは、daemon の処理待ちで溜まった画像がまとめて届いた場合に、古い画像が採られて新しい画像が拒否されるためである
- 版 1 から版 2 への変更は `Capabilities` と `Limits` のフィールド追加を含み、postcard の符号化が変わる。daemon は版の異なる plugin の `Hello` を拒否するため、外部 plugin は plugin-api の rev を更新して再構築する

版 3 では `Init` に `endpoints` を追加した ([接続先の許可](#接続先の許可))。版 2 と同じく符号化が変わるため、外部 plugin は再構築を要する。

`plugin-api` の検証は大きさの一般的な上限 (文字列 256 byte、8 行 × 4 要素) だけを扱い、デバイスに収まるかは daemon が変換時に検証する。plugin API の版をデバイスの上限値の変更から切り離すためである。

### 通知と手動命令の受付 (spool)

hook やスクリプトからの単発の要求は、設定ディレクトリの `spool/` に 1 件 1 ファイルで書き、daemon が短い間隔で取り込む。daemon が port を占有している間も、通知や活動状態を送れるようにするためである。

- local socket ではなく spool ディレクトリとするのは、Windows と Linux の両方で std だけで実装でき、アクセス制御を OS のファイル権限に委ねられるためである。daemon はコンテナで動かし、設定ディレクトリは Windows の `%APPDATA%\stackchan` をマウントするため (決定事項の「運用環境」)、Windows 側の hook もこのディレクトリに書ける
- Windows では利用者が build した exe を実行できない場合があるため、要求の形式は exe を使わずに書ける行単位のテキストとする。書式は利用者設定と同じで、section 見出しが要求の種類を表す。UTF-8 の BOM と CRLF を受け付ける (PowerShell 5.1 の `Set-Content -Encoding utf8` の出力)
- 書き手は `*.tmp` に書いてから `*.req` へ rename する。daemon は `*.req` だけを読み、書きかけのファイルを読まない
- daemon は 200 ms ごとに最大 8 件を、名前の順に取り込む。1 ファイルは 4 KiB までとする。処理した後は、不正なものも含めて削除し、不正なものは理由をログに残す
- 通知は plugin の `Notify` と同じ検証 (文字列の長さ) を経て scheduler に入る。待ち行列は 16 件で、溢れた分は古いものから捨てる
- デバイスへ送る要求 (status、input) は、切断中や再接続待ちの間は daemon が保持し、送れるまで送り直す。spool のファイルは取込み時に削除するため、ここで保持しないと失われる。どちらも最新の 1 件だけが意味を持つため新しい要求で置き換え、status は要求された期限を過ぎたら捨てる。遅れて送る場合は、期限までの残り時間を TTL とする

| 見出し | key | 内容 |
| --- | --- | --- |
| `[notify]` | `text` (必須)、`priority` (`low` / `normal` / `high`、既定 `normal`)、`ttl_s` (既定 0 = 10 秒) | 通知を表示する。`high` は Overlay、それ以外は下の帯に置く |
| `[status]` | `activity` (`idle` / `working` / `waiting` / `done` / `error`、必須)、`detail` (既定 空)、`ttl_s` (既定 30) | 活動状態と、それに対応する表情を表示する (`stackchan status` と同じ対応) |
| `[input]` | `mode` (`demo` / `forward`、必須) | タップの扱いを切り替える |

```
# 例: <設定ディレクトリ>/spool/20260926-120000-001.req
[notify]
text = "Claude Code: 入力を待っています"
priority = "normal"
ttl_s = 10
```

CLI では `stackchan notify --text ...` が spool に書く。`stackchan status` と `stackchan input` は `--via-daemon` を付けた場合に spool に書く。どれも daemon が取り込むまで待たない。

## 画面と入力の配分

### slot と scheduler

firmware の slot (BannerTop / BannerBottom / Overlay) はそのまま使い、daemon の scheduler が plugin の Card を割り当てる。

| 種別 | 割当て先 | 規則 |
| --- | --- | --- |
| 常設 Card (時計、使用率) | 帯 | 帯ごとに候補を一定間隔で巡回する。利用者設定で固定表示も可能 |
| 通知 | 帯または Overlay | 優先度が高いものは巡回に割り込む。TTL 満了または既読化で巡回に戻る |
| 画像 (カメラ) | 画像領域 (Overlay 内) | 利用者の操作で開始し、操作または一定時間で終了する。通常優先度の Overlay Card と同順位とし、同順位では新しい方を出す。高優先度の通知と Card は画像に勝つ |
| presence / emote | 顔と機構部 | 手動の `status` / `face` / `emote` が最優先。plugin からの要求は許可されたものだけを短い TTL で採用する。Emote は連続送信で上書きされるため、plugin ごとに最短間隔を設ける |

firmware 内の表示期限 (TTL) は保険として残し、daemon は表示を切り替えるたびに Card を送り直す。daemon が停止しても古い表示が残らないようにするためである。

- 帯の Card は 2 枚ずつ上下に置き、`rotate_s` ごとに次の組へ進める。優先度の高い順、同順位は設定順と Card 識別子の順に並べる
- 高優先度以外の通知がある間は、下の帯を通知に、上の帯を Card 1 枚ずつの巡回に使う。高優先度の通知は Overlay に置く
- firmware 側の TTL は「表示内容の残り時間 (切上げ)」と「`rotate_s` + 5 秒」の短い方とし、`rotate_s` ごとに送り直す
- 表示を消す場合は `ClearSlot` を送る。1 秒以内に firmware 側の期限で消える表示には送らない

### 入力

- Card の各行は任意で action ID を持つ。firmware はタップ位置を表示中の Slot と行に照合し、`Event::Tap { slot, card, action }` を返す。行外や Text のタップは `card` / `action` を持たない Tap とし、daemon の scheduler が巡回の送りに使う
- 座標をそのまま plugin に渡さない。表示位置が変わっても plugin の実装が影響を受けないようにするためである

### タップの扱いの切替

firmware はタップの扱いとして `Demo` (現行の表情デモ) と `Forward` (host へ Event を送る) の 2 つの状態を持つ。切替は CLI (`stackchan input demo|forward`) で明示的に行い、daemon は自動では切り替えない。

- 起動時は `Demo` とする。host が接続されていなくても単体で動作を確認できる現行の挙動を保つためである
- 状態は RAM 上にのみ保持し、再起動で `Demo` に戻る。firmware に設定の永続化を持たせない方針 ([design.md](design.md#目標と制約)) に従う
- `Forward` の間は、タップによる firmware 内の動作 (表情デモの進行、Emote の解除) を行わず、Event の送信だけを行う。タップへの反応は daemon と plugin が決める
- `Forward` の間も Event の送信先が無ければ破棄するだけで、表示には影響しない
- daemon の動作中は `stackchan input --via-daemon forward` (spool 経由) で切り替える。firmware が再起動した場合は `Demo` に戻るため、切り替え直す

## デバイスプロトコルの拡張

版 8 で次を追加した (P2)。詳細は [protocol.md](protocol.md#タップの通知-版-8) を参照する。

| 追加 | 内容 |
| --- | --- |
| `Card.id: u16` と `Row.action: Option<u8>` | Card の識別と行単位の操作領域。daemon は (plugin, Card 識別子) を 1 以上の値に符号化する。0 は識別なし (CLI) |
| `Message::ClearSlot(Slot)` | 1 つの Slot だけを消す。P1 で用いた「空の Text を TTL 1 秒で送る」方法を置き換えた |
| `Message::InputMode(Demo \| Forward)` | タップの扱いの切替。揮発。Ack を返す |
| `Reply::Event(Event::Tap { slot, card, action })` | タップの通知。host の要求とは非同期に送る。送信待ちは 1 件 |

daemon はタップを次のように振り分ける。

- 行に action を持つ Card のタップは、発生元の plugin へ `Action { card, action }` として返す。表示中でない Card への Action は送らない
- 帯のそれ以外の位置のタップは、帯の巡回を次の組へ送る
- 顔の領域と、Overlay の action の無い位置では何もしない

版 9 で画像領域の分割転送 (`ImageBegin` / `ImageRows` / `ImageEnd`) を追加した (P4)。詳細は [protocol.md](protocol.md#画像領域-版-9) を参照する。受信した行は firmware の画像用の static 領域に書き、`ImageEnd` の受信後にフレームバッファ ([design.md](design.md#画面の更新)) へ描いて LCD へ転送する。途中まで受信した画像が表示されないようにするためである。160×120 px の画像は約 38 KB であり、1 fps 以下の更新を想定する。

画像用の領域はフレームバッファとは別に持つ。表示状態 (Slot の内容) は描画のたびに複製されるため、画素を表示状態に含めると 38 KB の複製が描画ごとに生じるためである。

## plugin ごとのコンテナ分離

コンテナでの運用時 (`scripts/container.sh daemon`) の構成である。段階 P5 (コンテナの分離と socket での接続) と P6 (接続先の許可) で実装した。

### 目的

| 対象 | 子プロセス方式 | コンテナ分離 |
| --- | --- | --- |
| secret | 全 plugin が `secrets.conf` を読める | plugin のコンテナに設定ディレクトリを渡さない。secret は `Init` で自分宛てのものだけを受け取る |
| ログと spool | 全 plugin が書ける | plugin のコンテナに渡さない |
| 他のプロセス | daemon と他の plugin が見える | plugin ごとに PID 名前空間が分かれる |
| ネットワーク | daemon と同じ範囲に接続できる | plugin のコンテナはネットワークを持たない。利用者設定に列挙した接続先だけに、中継を介して接続できる |

### 構成

```
ホスト (scripts/container.sh daemon)
  |  起動計画 (stackchan config --launch-plan) を読み、次のコンテナを起動する
  |
  +-- daemon のコンテナ          --network none、USB デバイス、/config
  |     plugin ごとの Unix socket で待ち受ける
  |
  +-- plugin のコンテナ (plugin ごと)   --network none、読取り専用、capability なし
  |     stackchan plugin-run が socket へ接続し、plugin を子プロセスとして起動する
  |
  +-- 中継のコンテナ (接続先の許可がある場合のみ)   ネットワークあり
        stackchan relay が Unix socket への接続を、列挙された宛先へ TCP で転送する
```

- コンテナの起動は、daemon ではなくホスト側のスクリプトが行う。daemon にコンテナエンジンの操作権を渡すと、Podman machine 上の任意の path をマウントできる権限を、plugin からの入力を処理するプロセスに与えることになるためである
- daemon と plugin は、plugin ごとの named volume に置く Unix socket で接続する。volume は daemon と当該 plugin のコンテナだけにマウントする。plugin は自分の volume の socket にしか到達できないため、daemon は接続を受けた socket で plugin を識別する
- volume は bind mount ではなく named volume とする。設定ディレクトリは Windows 側のディレクトリであり、その上では Unix socket を作成できない場合があるためである
- 全コンテナで同じイメージを用いる。plugin の実行ファイルは Nix store の動的リンカを絶対 path で参照しており、開発用のイメージ内でしか動かないためである。plugin 用の最小イメージは後続の課題とする

### plugin との接続

daemon の待受けと、plugin 側の起動補助 (`stackchan plugin-run`) を追加する。plugin 自体は従来どおり stdin / stdout でフレームを交換するプログラムであり、変更を要しない。Rust 以外で書いた plugin も同じ方法で動く。

1. daemon は `--socket-dir <path>` を指定された場合、plugin を子プロセスとして起動せず、`<path>/<plugin id>/plugin.sock` で待ち受ける
2. `plugin-run` は socket へ接続する。接続できない場合は 1 秒後に再試行する
3. daemon は接続を受けると、起動情報 (利用者設定の `command` と `env.*`) を 1 フレームで送る。`plugin-run` に設定ディレクトリを渡さずに argv を伝えるためである。この型は `plugin-api` ではなく host の crate に置く (`crates/host/src/launch.rs`)。plugin の作者が扱うものではないためである
4. `plugin-run` は socket を子プロセスの stdin と stdout として渡し、plugin を起動する。以降のフレームは `plugin-run` を経由しない
5. plugin が終了すると、`plugin-run` は 1 秒後に 2 から繰り返す。daemon が接続を受け付けない間 (再起動までの待ち時間、無効化) は、起動情報を受け取る前に接続が閉じられ、plugin は起動されない

監視と停止は次のとおりとする。

- daemon は plugin を停止する場合 (手順違反、流量超過、`Shutdown` 後の時間切れ)、接続を閉じる。子プロセスを直接終了させる手段は持たない。接続を閉じられても終了しない plugin は、表示の経路を失ったまま自身のコンテナ内で動き続ける。資源の消費はコンテナの上限 (メモリ、プロセス数) で抑える
- 再起動までの待ち時間と、連続失敗による無効化は、従来どおり daemon が判断する。`plugin-run` は plugin と同じコンテナで動くため信頼しない。待ち時間内の接続と、無効化した plugin の接続は、daemon が受けた直後に閉じる
- 同じ plugin の接続は 1 本だけを保持し、2 本目以降は閉じる
- plugin の標準エラーは daemon に届かず、コンテナエンジンのログに残る (`podman logs stackchan-plugin-<id>`)。`Log` メッセージは従来どおり daemon のログに出る

`--socket-dir` を指定しない場合は、従来の子プロセス方式で動く。Windows ネイティブでの動作と、開発時の確認のために残す。この方式では分離も接続先の制限も働かないため、`net.allow` を持つ plugin があれば daemon は起動時に警告を出す。

### 接続先の許可

plugin のコンテナは常に `--network none` とし、許可は宛先を列挙する方式で与える。

- 利用者設定の `net.allow` に、IP アドレスと port の組を列挙する。名前解決は扱わない。plugin から解決先を変えられる経路を作らないためである
- 中継 (`stackchan relay`) は、列挙された宛先ごとに Unix socket (`<socket dir>/<plugin id>/net/<番号>.sock`) で待ち受け、接続ごとに宛先へ TCP で接続して双方向に転送する。宛先は起動時の引数で固定され、plugin からは変えられない
- 中継は daemon と別のコンテナで動かす。daemon のコンテナを `--network none` に保つためである
- 許可の強制は firewall の規則ではなく、plugin のコンテナにネットワークが存在しないことによる。rootless の Podman は宛先単位の送信制限を備えておらず、Podman machine に規則を入れる方法はリポジトリから再現できないためである
- TLS は plugin が socket の上で張る。中継は内容を解釈しない
- 中継は宛先ごとの同時接続数に上限を設ける (4 本)。超えた接続は閉じる
- 中継は接続のどちらかの向きが終わったら両方を閉じる。片方向だけを閉じた接続が残り、同時接続数の枠を占め続けないようにするためである。宛先へ接続できない場合も、plugin 側の接続を閉じる

plugin には、`Init` の `endpoints` (宛先と、plugin のコンテナから見た socket の path の組) で接続方法を伝える。plugin は `client::Connection::connect_to` (`plugin_api::net::connect`) に宛先を渡して接続する。`endpoints` に宛先があれば Unix socket に、無ければ TCP で直接接続する。plugin の実装を、分離の有無で分けないためである。宛先の一致は表記ではなく解析した値で判定する。

2026-10-05 に Windows 上の Podman machine で、plugin のコンテナから許可した宛先へ中継の socket を介して到達できること、同じ宛先へ直接は接続できないこと、許可を持たない plugin のコンテナに中継の socket が無いことを確認した。宛先には Podman machine 内の待受けを用いており、LAN 上の機器への到達は未確認である (未決事項 2)。

### 利用者設定の追加

```
[plugin bambu]
command = ["/plugin/stackchan-bambu"]   # plugin のコンテナ内の path
dir = "C:/Users/<user>/repos/stackchan-plugin-bambu/target/release"
rev = "<40 桁の commit SHA>"
cards = 1
notify = true
net.allow = ["192.168.1.50:8883"]
param.host = "192.168.1.50:8883"
```

| key | 内容 |
| --- | --- |
| `dir` | plugin の実行ファイルを置いたホスト側のディレクトリ。plugin のコンテナの `/plugin` に読取り専用でマウントする。省略時は本リポジトリの `target/release` (同梱の plugin) |
| `net.allow` | 接続を許可する宛先 (`"<IP アドレス>:<port>"`) の並び。最大 4 件。省略時は接続先を持たない。`secrets.conf` には書けない |

`command` はコンテナ分離時には plugin のコンテナ内の path を書く。子プロセス方式とは path が異なるため、同じ設定ファイルを両方式で共用することはできない。

### 起動計画

`stackchan config --launch-plan` は、スクリプトがコンテナを起動するために必要な項目だけを、1 行 1 件のタブ区切りで出力する。設定の構文の解釈をシェル側に重複させないためである。

```
plugin<TAB><id><TAB><dir>
relay<TAB><id><TAB><番号><TAB><IP アドレス>:<port>
```

`id` と宛先は使用できる文字が限られる。`dir` は行の最後の項目とし、制御文字 (タブと改行を含む) を含むものは設定の検証で拒否する。省略時は空とし、スクリプトが本リポジトリの `target/release` を用いる。argv、env、param は出力しない。`plugin` の行をすべて出力した後に、`relay` の行を plugin ごとにまとめて出力する。

### コンテナの起動条件

| コンテナ | 条件 |
| --- | --- |
| daemon | `--network none`。`/workspace` と `/config` は読取り専用、`logs` と `spool` は書込み可能。全 plugin の volume、USB デバイス |
| plugin | `--network none`、`--read-only`、`--cap-drop=all`、`--security-opt no-new-privileges`、メモリ 256 MiB とプロセス数 64 の上限。マウントは `/plugin` (読取り専用)、自身の volume、`stackchan` の実行ファイル (読取り専用) だけ |
| 中継 | ネットワークあり (エンジンの既定値。`STACKCHAN_RELAY_NETWORK` で変えられる)、`--read-only`、`--cap-drop=all`、`--security-opt no-new-privileges`、plugin と同じ資源の上限。マウントは `net.allow` を持つ plugin の volume と `stackchan` の実行ファイルだけ。`net.allow` を持つ plugin が無ければ起動しない |

`scripts/container.sh daemon` は、構築、起動計画の取得、volume の作成、中継と plugin のコンテナの起動を行ってから、daemon を前面で起動する。daemon の終了時には、起動したコンテナと volume を取り除く。

2026-10-05 に Windows 上の Podman machine (WSL) で、named volume 上の socket による接続、plugin のコンテナから設定ディレクトリ・ネットワーク・他の plugin の socket に到達できないこと、plugin の終了後の再接続、daemon の終了時の後始末を確認した。確認の手順は [environment.md](environment.md#daemon-と-plugin) に置く。

### 分離後に残るリスク

- コンテナはカーネルを共有する。カーネルやコンテナエンジンの欠陥による脱出は防げない
- `net.allow` を持つ plugin は、許可された宛先に対しては任意の内容を送れる。宛先の機器に対する操作を制限するものではない
- 全コンテナが開発用のイメージを共用するため、plugin のコンテナにも compiler 等と、イメージの構築時に複製されたリポジトリの内容 (`/workspace`) が含まれる。ネットワークが無く、ファイルシステムが読取り専用であることで影響を抑える
- 第三者製の plugin の導入前のソース確認と commit SHA による固定は、引き続き必要である

## Client 側の plugin

### 目的

host が無くても動く機能を、firmware の本体 (通信、表示状態、描画) と分けて追加できるようにする。最初の対象はタイマーである。3D プリンタの進捗を認証情報なしに取得できないため、所要時間を利用者が与えて残り時間を表示する。

firmware には既に、host なしで動く動作がある (タップで進む表情デモ)。Client 側の plugin は、この種の動作を追加するための枠組みである。

### 維持する制約

- host からの入力の扱いは変えない。C1 では、host からのメッセージが Client 側の plugin に届く経路は無い。[design.md](design.md#目標と制約) の「host からの入力を描画対象のデータとしてのみ扱う」は、そのまま成り立つ
- 動的確保を行わない。plugin の状態は固定長とする
- 設定を永続化しない。plugin の状態は再起動で消える
- Wi-Fi/BT を使わない
- 依存を増やさない

### 信頼の水準

Client 側の plugin は firmware と同じアドレス空間で動き、隔離できない。1 つの plugin の panic で firmware 全体が止まる。Host 側のような capability による制限は意味を持たないため設けず、firmware の一部として扱う。

- 追加はソースの確認と再構築による。外部のリポジトリの plugin は、firmware の依存として commit SHA で固定する
- 表示と入力の配分 (次項) は権限の制限ではなく、複数の plugin と host の表示が互いを壊さないための規約である

### 枠組み

plugin は画素を直接描かない。host が送るものと同じ宣言的な内容 (Card) と、操作画面 (Panel) を出力し、描画は firmware の renderer が行う。描画の経路を 1 本に保ち、host 上の試験 (`firmware/examples/simulate.rs`) で表示を確かめられるようにするためである。

| plugin が受け取るもの | 内容 |
| --- | --- |
| 周期的な呼出し | 単調時計の時刻。表示の更新と期限の判定に用いる |
| 帯の Card のタップ | 自身の Card がタップされたこと |
| Panel のボタン | 押されたボタンの番号 |
| 起動の要求 | 長押しから開かれたこと (下記) |

| plugin が出力するもの | 内容 |
| --- | --- |
| 帯の Card | 1 枚まで。内容は `protocol::Card` と同じ (Text / Bar / Spacer) |
| Panel | Overlay に出す操作画面。文字の行 (2 行まで) と、ボタン (6 個まで) からなる。開いている間は顔を隠すため、操作が無ければ一定時間で閉じる |
| 表情 | 一時的な表情 (Emote)。完了の通知等に用いる |

Panel を Card と別に設けるのは、Card の行 (高さ 20 px、画面上で約 2.5 mm) が指での操作に小さすぎるためである。ボタンは 3 列 × 2 行に置き、1 個を約 100 × 80 px とする。

登録は firmware 内の一覧 (`firmware/src/client/mod.rs` の `Builtin`) への追加で行い、構築時に確定する。登録できる数は 6 個までとする (一覧の Panel に 1 画面で並ぶ数)。plugin は `Plugin` trait を実装する。plugin の呼出しと表示の取込みは 100 ms ごとに行い、入力の直後にも行う。

### 画面の配分

| 対象 | 規則 |
| --- | --- |
| 帯の Card | Client 側の Card は上の帯に出し、その間は host が上の帯に置いた内容を隠す (内容と期限は保持する)。上の帯とするのは、host の通知が下の帯に出るためである |
| Panel | Overlay に出す。host の Overlay (通知、画像) より手前に出す。利用者の操作で開くものであり、操作中に隠れないようにするためである |
| 複数の plugin | Card を出す plugin が複数ある場合は、上の帯に 5 秒ごとに順に出す。Panel は同時に 1 つとし、Panel が開いている間の長押しは無視する |
| 表情 | host の Emote と同じ扱いとし、後から届いたものが置き換える |
| host の `Clear` | host が置いた内容だけを消す。Client 側の表示と plugin の状態は変えない |
| 起動確認画面 | Client 側の plugin が最初に表示を求めた時点で終える (host の最初の表示命令と同じ扱い) |

C1 では host は Client 側の表示を知らない。Client 側の Card が出ている間、daemon は上の帯に置いた Card が見えていると判断したままになる (`Visibility` が実際と食い違う)。C2 で、Client 側の Card の有無を host に通知し、daemon の scheduler が配分に含めるようにする。

### 入力の配分

firmware は接触をタップと長押しに判定し、表示中の内容に応じて振り分ける。

| 操作 | 扱い |
| --- | --- |
| Panel が開いている間のタップ | 位置をボタンに照合し、Panel を出した plugin に渡す。ボタンの外では何もしない (背後の表示にも host にも渡さない) |
| Client 側の Card が出ている上の帯のタップ | その plugin に渡す。host の Overlay が帯を隠している間は、host 側のタップとして扱う |
| それ以外のタップ | `Demo` では表情デモを進め、`Forward` では host へ Event を送る |
| 長押し (0.6 秒以上) | Client 側の plugin の起動。plugin が 1 つなら直接開き、複数なら一覧の Panel (plugin の名前と `CLOSE`) を出す。一覧は 30 秒操作が無ければ閉じる |

- 長押しを起動に用いるのは、何も表示していない状態から plugin を開く操作が要り、host 側の入力 (タップ) と重ならないようにするためである
- タップは指を離した時点で確定する。押した時点で確定すると、長押しの始まりと区別できないためである。位置は接触を始めた点とする
- 離した状態を 2 回 (40 ms) 続けて観測してから、接触の終わりとして扱う。1 回だけの欠落は同じ接触の続きとみなす
- 長押しは接触が続いている間に 1 回だけ判定し、指を離してもタップにはしない
- Client 側が受けたタップは host へ送らない

### タイマー

| 状態 | 表示 | 操作 |
| --- | --- | --- |
| 停止中 | なし | 長押しで設定の Panel を開く |
| 設定中 | Panel。設定中の時間と、`+1h` `+10m` `+1m` `CLEAR` `START` `CLOSE` のボタン | `START` で開始する。`CLOSE` は設定を捨てる。30 秒操作が無ければ閉じる |
| 動作中 | 上の帯の Card。経過の割合の Bar と残り時間 (時:分:秒)、合計時間 | Card のタップまたは長押しで `+10m` `-10m` `STOP` `CLOSE` の Panel を開く。30 秒操作が無ければ Panel を閉じる (動作は続く) |
| 完了 | Panel で完了を表示する。首を上へ向けて発光する (Emote、10 秒) | `OK` で閉じる。60 秒で閉じる |

- 時間は時・分で指定し、上限は 99 時間 59 分とする
- 0 分では開始しない。残りが 10 分以下の場合、`-10m` は無視する。押し間違いで直ちに完了させないためである
- 完了の表情は、Panel が顔を隠すため画面には現れず、首の動きと発光で知らせる役割になる
- 残り時間は単調時計から求める。host の時刻には依存しない。端数は切り上げ、表示が 0 になるのは完了時だけとする
- 動作中の Card は 1 秒ごとに内容が変わる。描画は変化した範囲だけを LCD へ送るため、表示全体の更新にはならない
- 名前 (印刷物の名称等) は扱わない。デバイス上で文字を入力する手段が無いためである。C2 で host から与えられるようにする

### host との連携 (C2)

C1 の範囲外であり、Host 側の対応する plugin を作る段階で実装する。必要な追加は次のとおり。

| 追加 | 内容 |
| --- | --- |
| デバイスプロトコル | plugin 宛ての不透明なデータを双方向に運ぶメッセージ (plugin の識別子と、上限付きの byte 列)。Client 側の Card の有無の通知と、host がそれを Slot に置く指示 |
| plugin API | Host 側の plugin が、対応する Client 側の plugin とデータを交換するメッセージ。利用者設定で、Host 側の plugin ごとに連携を許す Client 側の plugin を指定する |
| daemon | 上記の中継と、Client 側の Card を含めた配分 |

この段階で、host 由来の byte 列を Client 側の plugin が解釈することになる。上限付きの型で受け、plugin ごとに内容を検証する。[design.md](design.md#目標と制約) の制約の記述も、この段階で改める。

タイマーでは、Host 側から所要時間と名前を与える用途 (スライサーの出力からの設定等) を想定する。

### 検証

| 対象 | 方法 |
| --- | --- |
| タイマーの状態遷移 | 時刻を与える単体試験。設定、開始、補正、完了、時間切れでの Panel の終了、上限 |
| 入力の振分け | タップと長押しの判定 (接触の継続時間、離した後の再受付)、Panel のボタンと帯の照合 |
| 画面の配分 | Client 側の Card が上の帯を占める間、host の内容と期限が保持され、Card が消えると再表示されること。Panel が host の Overlay より手前に出ること |
| 描画 | `simulate.rs` に、設定中・動作中・完了の各状態を追加し、画像で確かめる |
| 実機 | 長押しの判定時間、ボタンの大きさと押しやすさ、1 秒ごとの更新時の表示のちらつきの有無 |

## 検証

| 対象 | 方法 |
| --- | --- |
| コンテナ分離 | socket での待受けと `plugin-run` は、一時ディレクトリの socket を用いた結合テストで確認する (待ち時間内の接続の拒否、2 本目の接続の拒否、起動情報の受渡し)。中継は loopback の宛先で確認する (socket ごとの宛先の固定、同時接続数の上限、到達できない宛先での切断)。起動計画は出力の形式と、不正な `dir` / `net.allow` の拒否を確認する。コンテナの起動条件は `make check` では実行できないため、手動の確認手順を environment.md に置く (plugin のコンテナから設定ディレクトリに到達できないこと、許可した宛先に中継を介してだけ到達できること) |
| plugin プロトコル | 型の往復、上限ちょうど・超過、不正フレーム、`Hello` 前のメッセージの拒否。`decoder_stress` と同様の固定 seed の変異入力 |
| 利用者設定のパーサ | 正常系、構文エラー、未知・重複 key、過長行、任意バイト列。固定 seed の変異入力を含める |
| daemon | mock plugin を用いた結合テスト。異常終了からの再起動、流量超過での停止、許可外 capability の拒否、spool の書きかけファイルの無視 |
| デバイスプロトコル | 既存の単体テストと `decoder_stress` に新 variant を追加する |
| 描画 | `firmware/examples/simulate.rs` に Card の巡回、通知の割込み、画像領域の状態を追加し、画像で比較する |
| 対象 OS | 単体・結合テストはコンテナ内 (Linux) で実行する。Windows 向けはコンテナ内の cross build が通ることを `make check` で確認し、実行時の動作は Windows 上で手動の確認手順により確かめる |
| plugin 作者向け | pipe で接続した daemon 側の模擬で、plugin 単体の出力を CI で検査できるようにする |

## 段階

| 段階 | 内容 | 受入条件 | 状況 |
| --- | --- | --- | --- |
| P0 | 本書の確定 | 未決事項の決定 | 完了 |
| P1 | `plugin-api`、daemon、利用者設定、`plugins/clock` | 時計が帯に表示され続け、plugin の異常終了から復帰する。Windows と Linux で動作する | 実装済み |
| P2 | デバイスプロトコル版 8 (card_id、InputMode、Event)、scheduler、入力の振分け | タップが発生元の plugin に届く。複数 Card が巡回する | 実装済み |
| P3 | spool による通知の受付、別リポジトリの plugin の導入手順 | commit SHA で固定した外部 plugin が許可した capability の範囲で動作する | spool は実装済み。外部 plugin の導入手順は未作成 |
| P4 | 画像領域 | 外部 plugin からの画像が Overlay に表示される | 実装済み |
| P5 | plugin ごとのコンテナ分離 (socket での接続、`plugin-run`、起動計画、`container.sh`) | 同梱の plugin が plugin ごとのコンテナで動作する。plugin のコンテナから設定ディレクトリと外部への通信に到達できない | 実装済み |
| P6 | 接続先の許可 (`net.allow`、中継、plugin API 版 3 の `endpoints`) | 列挙した宛先にだけ plugin から接続できる | 実装済み |
| C1 | Client 側の plugin の枠組み (登録、Panel、画面と入力の配分、長押し) とタイマー | host を接続せずに、タイマーの設定・開始・補正・完了の表示ができる。host の表示と同時に使っても、互いの内容を壊さない | 実装済み。host を接続しない状態での動作を実機で確認した (2026-10-06)。daemon との同時使用は未確認 |
| C2 | host との連携 (plugin 宛てのデータ、Client 側の Card を含めた配分) | Host 側の plugin からタイマーを設定できる。daemon の `Visibility` が実際の表示と一致する | 未着手 |

Claude / Codex 使用率は P3 の最初の外部 plugin とする。design.md の collector (Phase 3) は本機構の plugin として実装し、host への組込みは行わない。取得元のログ形式に依存する部分をリポジトリ外へ分離するためである。

## plugin ごとの留意点

- Claude / Codex 使用率: ログ形式、およびサブスクリプションの残量を取得する公式手段の有無は、着手時に一次情報で確認する
- 3D プリンタ: LAN 経由の制御・映像取得には公式仕様ではなく第三者の解析に依拠するものがあり、プリンタの firmware 更新により第三者のアクセスが制限される場合もある。利用条件の確認を含め plugin 側の責務とし、本リポジトリにはプリンタ固有のコードを置かない。状態 (進捗、残り時間、温度) の取得にはプリンタのアクセスコードが要る見込みである。このコードは LAN 内から公式ソフトでプリンタを操作できる認証情報でもあり、閲覧専用の認証情報は確認できていない。表示だけの用途で操作可能な認証情報を PC に保存しないため、認証を要する情報は扱わないことにし、進捗表示の plugin は見送った (2026-10-05)。同日に Bambu Lab A2L (クラウドモード、印刷中) で、LAN 内の MQTT (TLS、port 8883) が待ち受けていること、認証情報なしの接続要求が CONNACK の戻り値 5 (認可されていない) で拒否されることを確認した。認証情報を付けた接続は試していない。代わりに、プリンタと通信しないタイマーを Client 側の plugin として設ける
- 通知: OS の通知を横取りする方式は OS ごとの制約が大きいため、第一段階では spool 経由の受付で受ける

## 決定事項

| 項目 | 決定 | 理由 |
| --- | --- | --- |
| 隔離 | OS による隔離は必須としない。Linux では起動コマンドの前置きで任意に隔離できるようにする | Windows と Linux に共通する手段を依存なしで用意できない |
| 隔離 (改定) | コンテナでの運用時は plugin ごとにコンテナを分ける。コンテナはホスト側のスクリプトが起動し、daemon とは Unix socket で接続する (2026-10-05) | secret を扱う plugin を導入するため。daemon にコンテナエンジンの操作権を渡さない |
| 接続先の許可 | plugin のコンテナはネットワークを持たず、利用者設定に列挙した宛先 (IP アドレスと port) にだけ中継を介して接続できる (2026-10-05) | 許可を列挙方式とし、firewall の規則に依らずに強制する |
| plugin の種別 | Host 側と、firmware に構築時に組み込む Client 側の 2 種とし、連携できるものとする。実行時のコードの読込みは行わない (2026-10-05) | host が無くても動く機能を追加できるようにする。構築時の組込みは既存の依存だけで成立する |
| 形式 | plugin プロトコルは postcard + COBS、利用者設定は自前パーサの簡易形式 | Rust の依存を増やさない |
| 実行環境 | 実装は Windows ネイティブを対象から外さない。local socket の代わりに spool ディレクトリを用いる | std だけで両 OS に対応できる |
| 運用環境 | daemon と plugin は Linux 側 (コンテナ、`scripts/container.sh daemon`) で動かす。設定ディレクトリは Windows の `%APPDATA%\stackchan` をマウントし、Windows 側の hook からも spool に書けるようにする (2026-09-26) | Windows の Smart App Control が、利用者の build した署名の無い exe を実行させない。その設定の変更は難しく不可逆である |
| タップの切替 | CLI で明示的に切り替える。起動時は Demo | 単体での動作確認を保ち、daemon の暗黙の状態変更を避ける |
| 設定ディレクトリ | OS ごとの既定値を持ち、`--config-dir` で起動時に変更できる | 複数の構成の併用と試験での分離 |
| Windows 向けの構築 | コンテナ内で `x86_64-pc-windows-gnu` 向けに cross build する | ホストに toolchain を導入しない規約を保つ |

## Windows 向けの cross build

host 側の crate は Espressif fork の toolchain (nix/esp-rust.nix) で構築している。上流の `rust-std-x86_64-pc-windows-gnu` は compiler の commit が異なり、この toolchain では使えない。そのため次の構成とした (2026-09-23 に既存 CLI で成立を確認)。

- 標準ライブラリは同梱の rust-src から `-Z build-std` で構築する (firmware と同じ方式)
- linker と C runtime は nixpkgs の mingw-w64 cross toolchain を用いる。版は既存の nixpkgs の rev で固定される
- Windows 向けで新たにコンパイル対象となる crate (`windows-sys` 等) の調査は [dependencies.md](dependencies.md#windows-向け-host-cli-の追加調査-2026-09-23) に記録した

手順は [environment.md](environment.md#windows-ネイティブの-host-cli) を参照する。

## 解決した不具合

### Card の描画中のゼロ除算 (2026-09-23 発見、2026-09-26 修正)

P2 の実機確認で、Forward 状態のタップ後に daemon が送る Card で firmware が停止した。

- 停止は `renderer::draw_card` での `IntegerDivideByZero` 例外による panic だった。逆アセンブルすると、除数は行の要素数ではなく `card.image` の幅だった。`ImageRaw::new` の高さの計算 (画素数 / (幅 × 2)) が、`card.image` の判定より前にループの外へ先行して実行されていた
- 画像を持たない Card ではこの幅が未初期化で、0 のときだけ Xtensa の除算命令が例外を起こす。値はそれまでの処理の経過で変わるため、送り方やタイミングで発生が変わり、host の試験では再現しなかった。版 8 で Card の配置が変わり表面化したと考えられる。タップと Forward 状態は発生の条件ではなかった
- 画像の描画を `#[inline(never)]` の関数に分け、先行実行されないようにした。逆アセンブルで、`draw_card` に残る除算が 0 の判定を経た行の要素数だけであることを確認した。修正前に毎回停止した手順 (リセット直後に .NET の SerialPort から Card を送る) を 12 回繰り返し、すべて応答することを確認した

調査の過程で、daemon の書込みに待機中の読取りの待ち時間 (1 ms) が残る誤りも見つけて修正した。

## 未決事項

1. 実行ファイルと commit SHA の対応を検証する方法 (再現可能な構築、ハッシュの照合)。P3 で扱う
2. 中継のコンテナから LAN 上の機器へ到達できるネットワークの構成 (Podman machine の既定のネットワークで足りるか)。Podman machine 内の宛先への到達は確認済みで、LAN 上の機器は、`net.allow` を使う最初の plugin の導入時に確認する
3. plugin 用の最小イメージ。開発用のイメージの共用をやめるには、plugin の実行ファイルを Nix store に依存しない形で構築する必要がある
4. 接続を閉じられても終了しない plugin のコンテナを、daemon の外から停止する方法 (スクリプトによる監視等)
5. Client 側の plugin の表示と daemon の配分の関係。C1 では Client 側の Card が上の帯を占め、daemon はそれを知らない。C2 で通知と配分を定める
6. 長押しの判定時間 (0.6 秒) と Panel のボタンの大きさ。実機で操作して決める
7. USB の給電が止まった場合 (PC のスリープ等) のタイマーの扱い。CoreS3 の内蔵電池での継続時間と、復帰後の表示を実機で確かめる
