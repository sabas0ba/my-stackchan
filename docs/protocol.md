# プロトコル仕様

host から firmware へ表示内容を送るための、シリアル上のフレーム形式。定義の実体は `crates/protocol` であり、本書はその設計意図と不変条件を記す。

## 版

`protocol::VERSION` (現在 8)。互換性の無い変更で増やす。firmware は `Pong` で自身の版を返し、host は不一致なら送信しない。

## 物理層

- USB Serial/JTAG (ESP32-S3 内蔵)。host からは CDC-ACM のシリアル port として見える (VID 0x303A, PID 0x1001)
- ボーレートは CDC では意味を持たないが、host 側の API 上の値として 115200 を用いる
- firmware のログ (esp-println) は UART0 に出るが、ROM のコンソールを経由するため USB にも出力され得る。ログは 0x00 を含まず、firmware は応答の前に 0x00 を送るため、host はフレームとして復号できない区切り単位を読み飛ばせばよい ([design.md](design.md#ログと通信の分離))

## フレーミング

- 1 メッセージを `postcard` でシリアライズし、COBS で符号化して 0x00 を終端として送る
- 受信側は 0x00 までをフレームとして切り出し、COBS を復号してから `postcard` で復号する
- 受信バッファは固定長 `MAX_FRAME_BYTES` (1024)。終端が来る前に溢れた場合はバッファを捨て、次の 0x00 から再同期する
- 復号または検証に失敗したフレームは破棄し、`Rejected` のカウンタを増やす。firmware は停止しない

## メッセージ

`protocol::Message` (host -> firmware):

| variant | 内容 | 上限 |
| --- | --- | --- |
| `Ping { nonce }` | 疎通確認 | - |
| `Clear` | すべての slot を消す | - |
| `Text { slot, ttl_s, text }` | テキストを表示する | `text` は `MAX_TEXT_BYTES` (512) バイト |
| `Card(Card)` | 行と要素からなる表示内容を 1 Slot に配置する | 最大 4 行、各行最大 2 要素 |
| `Presence(Presence)` | 活動状態・表情・視線を一括更新する | 詳細は UTF-8 で最大 20 byte |
| `Emote(Emote)` | 表情と二軸視線を一時的に重ねる | 視線 -100..100、強度 0..100、継続 100..10000 ms |
| `HardwareProbe` | 機構部の状態を読む | - |
| `PitchTrim(PitchTrim)` | ピッチ中心の補正 | -96..64 step |
| `ClearSlot(Slot)` | 1 つの Slot だけを消す | - |
| `InputMode(InputMode)` | タップの扱いを `Demo` / `Forward` に切り替える | - |

`protocol::Reply` (firmware -> host):

| variant | 内容 |
| --- | --- |
| `Pong { nonce, version }` | `Ping` への応答 |
| `Ack { seq }` | 描画に成功した Text / Card / Presence / Emote / Clear の通し番号 |
| `Rejected { count }` | 破棄したフレームの累計。診断用 |
| `HardwareStatus { .. }` | `HardwareProbe` への応答 |
| `Event(Event)` | 要求と独立に送る入力。現在は `Tap { slot, card, action }` のみ |

版 1 は Card、版 2 は Card への画像追加、版 3 は Presence、版 6 は Emote・二軸視線・HardwareProbe、版 7 は PitchTrim と HardwareStatus の補正値追加、版 8 は Card の識別子・行の action・ClearSlot・InputMode・Event に対応する。版 4・5 は使用していない。既存 Message variant の番号は維持する。

### 現在の実装範囲

firmware は USB Serial/JTAG から `Ping` を受信し、同じ nonce と
`protocol::VERSION` を含む `Pong` を返す。host の `stackchan ping` は nonce と版の
両方が一致した場合だけ終了コード 0 を返す。版不一致時は両者の版を含むエラーで終了する。

受信処理は USB パケットの分割・連結に依存しない。空の区切りは同期用として無視する。
不正・上限超過フレーム、無効な Card / Presence / Emote、描画に失敗した `Clear` / `Text` / `Card` / `Presence` / `Emote` は `Rejected` を返す。
上限超過時は次の区切りまでを 1 フレームとして破棄する。破棄回数は `u32::MAX` で飽和する。
応答 1 件分だけを保持し、USB FIFO に渡すまで次の要求の読み出しを待つ。
USB の送受信は非ブロッキング API を使用する。

host は要求の送信前に入力バッファを破棄し、起動直後に残る bootloader のログを除外する。
USB 側に残ったログとも区別できるよう、firmware は応答の前に同期用の 0x00 を送る。
host は空の区切り、不正・過長フレームを読み飛ばし、次の区切りから復帰する。
読み飛ばしを含む受信ループには 1 秒の期限を設ける。読み取り自体も 1 秒でタイムアウトする。

### Text / Card / Clear と表示期限

`stackchan text`、`stackchan card`、`stackchan clear` は同じ port で先に Ping を送り、nonce と版が
一致することを確認してから表示命令を送信する。版不一致では表示命令を送信しない。
表示命令は Ack を受信した場合だけ成功とする。

起動直後は従来の表示確認画面を使う。最初の Text / Card / Clear 以降は顔と Slot の表示へ切り替わる。
Text と Card は指定 Slot だけを置き換え、他の Slot は保持する。Clear は全 Slot と期限を消し、顔のみへ戻す。

| Slot | 領域 (x, y, width, height) | 表示量 |
| --- | --- | --- |
| BannerTop | (0, 0, 320, 48) | 30 文字 × 2 行 |
| BannerBottom | (0, 192, 320, 48) | 30 文字 × 2 行 |
| Overlay | (0, 0, 320, 240) | 30 文字 × 11 行 |

フォントは既存の 10×20 px ASCII を使用する。改行と 30 文字で折り返し、行数を超える部分は
描画しない。改行以外の制御文字は無視し、非 ASCII 文字は Unicode 文字単位で `?` に置換する。
入力は UTF-8 の 512 byte までで、host は超過を送信前に拒否する。

Overlay は顔と帯を隠すが、帯の内容と期限は保持する。`ttl_s` は受信時の単調時計から数え、
0 は期限なし、1〜65535 は秒数を表す。同じ Slot を上書きすると期限も更新する。
期限は USB の送信待ち中も確認し、Overlay の裏側の期限切れも除去する。
Overlay が消えると、その時点で有効な帯と顔が再表示される。

Card は最大 4 行で、各行には最大 2 要素を左から等幅で配置する。要素は `Text { text }`
(UTF-8 で最大 48 byte)、`Bar { ratio, label }` (比率 0〜100、ラベル最大 12 byte)、
`Spacer { height }` (1〜32 px)、`Image` を使う。Text、Bar、Image の行高は 20 px、Spacer の行高は指定値とし、
複数要素の行高は最大値を使う。上下 4 px の余白を含む合計が Slot の高さ (帯 48 px、
Overlay 240 px) を超える Card、空の Card・行は拒否する。Bar はラベルと比率の白いバーを描く。
Card の文字も既存の 10×20 px ASCII フォントを用い、非 ASCII は `?` で表示する。

Image は Card あたり 1 枚のみとし、Card の `image` フィールドに幅・高さと RGB565 の
big-endian 画素列を同梱する。縦横とも 1〜16 px、画素列は `width × height × 2` byte
(最大 512 byte) と一致しなければならない。行には画像データを複製せず `Image` 要素を 1 個置く。
画像データと要素の片方だけがある場合や、要素が 2 個以上ある場合は拒否する。
画像は該当セルの左上から 2 px 下へ置き、セルでクリップする。host CLI は無圧縮の 24-bit BMP
から指定領域を切り出して RGB565 に変換する。Card 全体の COBS フレームは 1024 byte 以下に収める。

### 活動状態・表情・視線

`Presence` は Slot とは独立した現在の顔の状態である。`activity` は Idle / Working / Waiting /
Done / Error または未指定。`expression` は Happy / Focused / Sleepy / Worried / Surprised /
Grin / Calm / Curious / Playful / Wink / Sad / Determined を指定する。
`gaze` は Center / Left / Right / Up / Down、`eyes` は Auto / Open / Wide / Closed /
HalfLidded を指定する。Auto は表情ごとの目の形を使う。視線は白い目の位置で示し、
黒目は描かない。`detail` は最大 20 UTF-8 byte で、
画面中央下部に活動状態とともに表示する。表示フォントの制約から非 ASCII 文字は `?` になる。
`ttl_s` は受信からの秒数で、0 は期限なし。期限満了時は活動状態と詳細を消し、既定の顔に戻る。
Overlay 中も期限は進み、Overlay が消えると有効な Presence だけを再表示する。
`Clear` は Presence、Emote、Slot をすべて消す。

`HardwareProbe` は表示や出力を変更せず、PY32 I²C アドレス `0x6F` と `0x71` の
バージョン応答、選択した拡張器の VM 出力ラッチと LED 設定、
CoreS3 AW9523B の BUS_OUT / BOOST 出力ラッチ、両サーボの現在位置・目標位置・
角度制限・電圧の生値・トルク状態、出力有効状態を
`HardwareStatus` で返す。未応答や無効なバージョン値は `None` とする。
`stackchan hardware` で確認できる。

`PitchTrim { raw_steps }` は待機姿勢のピッチ中心を -96..64 step の範囲で補正する。
1 step は約 0.3125°、負値が下向き。ESP の RAM にのみ保持し、再起動すると 0 に戻る。
`Clear` は補正値を変更しない。`stackchan pitch-trim --raw-steps -96 --save` で設定し、
`stackchan hardware` の `pitch_trim_raw_steps` で確認できる。`--save` は実機への適用後、
ホストの `.work/pitch-trim.txt` に保存する。以降の表示コマンドは接続ごとに設定を
再適用する。別の設置場所では `--pitch-trim-file <path>` または
`STACKCHAN_PITCH_TRIM_FILE` で保存先を切り替える。EEPROM と工場校正値は変更しない。
負方向の補正を大きくすると下向き視線は機構の安全下限で飽和するため、補正値が決まったら
その実機で上下の可動範囲を確認する。

`Emote` は既存の Presence と Slot を保持したまま顔を一時的に上書きし、`duration_ms` の満了後に
その時点で有効な Presence の顔へ戻る。`gaze=Point { x, y }` は左右・上下それぞれ
`-100..100` の連続値とし、画面内の目の移動量へ変換する。`eyes=Auto` はまばたきを許可する。
`intensity` は機構部の目標値に反映し、0 では首振り・発光とも無効にする。
画面描画への影響はない。視線の指定は眼球位置に加え、首の X/Y 目標角度を変える。
画面座標では Y の正方向が下、M5 BSP のピッチ角では正方向が上のため、首の上下は符号を反転して対応する。
機構部の出力は X ±30°、補正なしの Y 30–60°、LED 各色成分 0–63 に制限する。
ピッチ補正を含めても Y は 15–80° に制限する。
LED は指定強度を上限として、状態が有効な間に 2 秒周期で 70–100% の輝度変化を付ける。
サーボのゴール位置とトルクだけを揮発性レジスタへ書き込み、ID・校正値・EEPROM は変更しない。
初回の動作前に両軸の現在位置を読み、読めない場合は動かさない。古いゴールへの急な移動を避けるため、
現在位置をゴールに設定してからトルクを有効にする。移動量は 100 ms ごとに最大 8 ステップ
（約 2.5°）に制限し、目標到達の約 800 ms 後にトルクを解除する。次の移動前には位置を読み直す。
ESP 側だけ再起動した場合にもトルクを残さないよう、起動時と待機中にも解除指令を送る。
期限満了時は有効な Presence の目標値へ戻り、Presence がなければ正面・消灯へ戻る。
通信方式と工場既定のゼロ位置は [M5 公式 BSP の固定コミット](https://github.com/m5stack/StackChan-BSP/tree/8d4d6fc3b7a6be379c6317c45a02a30bff8c492e) に合わせる。
Emote を続けて送ると、最新の Emote が前の Emote を置き換えて期限を更新する。
`Clear` と本体画面のタップは Emote も消す。

host CLI では `stackchan emote --expression curious --gaze-x -60 --gaze-y 25 --intensity 75 --duration-ms 800`
のように指定する。視線座標と継続時間は送信前にも検査する。

host CLI の `face` は表情・視線・目の開き方を明示し、`status` は活動状態から表情を選ぶ
(Idle/Done: Happy、Working: Focused、Waiting: Sleepy、Error: Worried)。
`status` の既定 TTL は 30 秒で、PC 側の更新が停止した状態を残さない。

firmware は `eyes=Auto` の顔を時刻に応じて短くまばたきさせる。明示した目の形と
Overlay は自動まばたきで変更しない。まばたきは Presence の TTL と Ack 番号を変更しない。

Ack は描画と LCD への転送の完了後に返す。USB 送信が詰まった場合、firmware は 2 秒後に未送信の応答を破棄して次の受信を再開する。
host は描画応答を最大 5 秒待つ。seq は起動時 0、Text / Card / Presence / Emote / Clear の成功ごとに加算し、最初の Ack は 1。
`u32::MAX` の次は 0 に戻る。Ping、拒否、TTL 満了では加算せず、TTL 満了の自発的応答も送らない。
firmware はフレームバッファに描画してから、変化した範囲だけを LCD へ転送する
([design.md](design.md#画面の更新))。LCD への転送に失敗した場合は Rejected を返す。この場合
Slot の状態と seq は確定済みであり、次の転送で画面全体を送り直す。

decoder の coverage-guided fuzz 検証は別途実施する。

### タップの通知 (版 8)

版 8 は、画面のタップを host 側の plugin で扱うために追加した。設計は [plugin.md](plugin.md) を参照する。

- `Card.id` は host が付ける識別子で、0 は識別なしを表す。CLI の `card` は 0 を送る。`Row.action` はタップ時に返す値で、None の行は行を区別しない
- `InputMode` は RAM にのみ保持し、起動時と再起動後は `Demo` である。`Demo` では従来どおり表情デモを進め、`Forward` では firmware 内で反応せずに `Event::Tap` を送る。表示を変えないため描画せず、Ack の通し番号だけを進める
- `Event::Tap` はタップの立ち上がりで 1 回送る。`slot` はタップ位置に表示中の Slot で、Overlay の表示中は画面全体が Overlay である。帯の領域 (上端から 48 px、下端から 48 px) に表示が無い位置と顔の領域では `slot` を None とする。`card` は表示中の Card の識別子 (0 の場合と Text の場合は None)、`action` は位置を含む行の値である。行は領域の上端 4 px から行高の順に並ぶ (描画と同じ配置)。横方向の位置は照合に使わない
- 送信待ちの Event は 1 件だけ保持し、新しいタップで置き換える。要求への応答を優先し、応答の送信が無い時に送る。host の port が開かれていない間は、応答と同じく 2 秒後に破棄する
- host は応答を待つ間に `Event` を受信し得る。CLI は Event を読み飛ばし、daemon は保持して待機中にも読み出す
- `ClearSlot` は指定した Slot の内容と期限だけを消し、他の Slot と Presence は保持する

タッチの座標は FT6336U の P1_XH/XL/YH/YL (0x03..0x06) を接触点数 (0x02) と同じ転送で読み、画面の範囲に丸める。

## 不変条件

- 可変長のフィールドはすべて `heapless` の上限付き型で表す。上限を超える入力は `Deserialize` の時点で失敗し、firmware のメモリに到達しない (`oversized_text_is_rejected` テスト)
- firmware は `Message` の variant に対応する描画以外の動作を行わない
- テキストは UTF-8 として妥当であることを `heapless::String` の `Deserialize` が保証する。改行以外の制御文字は描画側で無視する
- 画像は縦横の上限と `width × height × 2` byte の長さを検証し、行の位置で決まるセル内に描画する

## 検証

- 単体テスト: 往復、上限ちょうど、上限超過、不正バイト列
- 再現可能な変異入力: `decoder_stress` が正常な Message / Reply と最大長 Card を基準に、切り詰め・挿入・上書き・境界長の任意バイト列を生成する。`decode<Message>`、`decode<Reply>` と復号された Card の `validate` が panic しないことを確認する。固定 seed の 20,000 件を `make check` に含め、失敗時は seed とケース番号を出力する
- 受信器: firmware の単体テストで任意バイト列と次の区切りを処理した後、正常な Ping フレームへ再同期できることを確認する
- coverage-guided fuzz: `cargo fuzz` 用の toolchain と依存を固定して継続実行する作業は別途実施する。固定 seed の変異検査はその代替とは見なさない
