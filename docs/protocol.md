# プロトコル仕様

host から firmware へ表示内容を送るための、シリアル上のフレーム形式。定義の実体は `crates/protocol` であり、本書はその設計意図と不変条件を記す。

## 版

`protocol::VERSION` (現在 2)。互換性の無い変更で増やす。firmware は `Pong` で自身の版を返し、host は不一致なら送信しない。

## 物理層

- USB Serial/JTAG (ESP32-S3 内蔵)。host からは CDC-ACM のシリアル port として見える (VID 0x303A, PID 0x1001)
- ボーレートは CDC では意味を持たないが、host 側の API 上の値として 115200 を用いる
- firmware は同じ port をログに使用しない (esp-println は UART0)

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

`protocol::Reply` (firmware -> host):

| variant | 内容 |
| --- | --- |
| `Pong { nonce, version }` | `Ping` への応答 |
| `Ack { seq }` | 描画に成功した Text / Card / Clear の通し番号 |
| `Rejected { count }` | 破棄したフレームの累計。診断用 |

Card への画像追加で版を 1 から 2 に上げた。既存 Message variant の番号は維持する。

### 現在の実装範囲

firmware は USB Serial/JTAG から `Ping` を受信し、同じ nonce と
`protocol::VERSION` を含む `Pong` を返す。host の `stackchan ping` は nonce と版の
両方が一致した場合だけ終了コード 0 を返す。版不一致時は両者の版を含むエラーで終了する。

受信処理は USB パケットの分割・連結に依存しない。空の区切りは同期用として無視する。
不正・上限超過フレーム、無効な Card、描画に失敗した `Clear` / `Text` / `Card` は `Rejected` を返す。
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

Ack は描画完了後に返す。seq は起動時 0、Text / Card / Clear の成功ごとに加算し、最初の Ack は 1。
`u32::MAX` の次は 0 に戻る。Ping、拒否、TTL 満了では加算せず、TTL 満了の自発的応答も送らない。
描画エラー時は Slot の状態と seq を確定せず Rejected を返す。ただし途中まで書かれた画素は
元に戻せないため、表示装置の障害が解消した後に表示命令を再送する。

decoder の fuzz 検証を含む Phase 2 全体は未完了。

## 不変条件

- 可変長のフィールドはすべて `heapless` の上限付き型で表す。上限を超える入力は `Deserialize` の時点で失敗し、firmware のメモリに到達しない (`oversized_text_is_rejected` テスト)
- firmware は `Message` の variant に対応する描画以外の動作を行わない
- テキストは UTF-8 として妥当であることを `heapless::String` の `Deserialize` が保証する。改行以外の制御文字は描画側で無視する
- 画像は縦横の上限と `width × height × 2` byte の長さを検証し、行の位置で決まるセル内に描画する

## 検証

- 単体テスト: 往復、上限ちょうど、上限超過、不正バイト列
- fuzz: `cargo fuzz` で `decode` に任意のバイト列を与え、panic しないことを確認する (Phase 2 で追加。fuzz 用の toolchain は別途固定する)
