# プロトコル仕様

host から firmware へ表示内容を送るための、シリアル上のフレーム形式。定義の実体は `crates/protocol` であり、本書はその設計意図と不変条件を記す。

## 版

`protocol::VERSION` (現在 0)。互換性の無い変更で増やす。firmware は `Pong` で自身の版を返し、host は不一致なら送信しない。

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

`protocol::Reply` (firmware -> host):

| variant | 内容 |
| --- | --- |
| `Pong { nonce, version }` | `Ping` への応答 |
| `Ack { seq }` | 受理したメッセージの通し番号 |
| `Rejected { count }` | 破棄したフレームの累計。診断用 |

Card (レイアウト付きの表示内容) と画像は Phase 2 で追加する。

### 現在の実装範囲

firmware は USB Serial/JTAG から `Ping` を受信し、同じ nonce と
`protocol::VERSION` を含む `Pong` を返す。host の `stackchan ping` は nonce と版の
両方が一致した場合だけ終了コード 0 を返す。版不一致時は両者の版を含むエラーで終了する。

受信処理は USB パケットの分割・連結に依存しない。空の区切りは同期用として無視する。
不正フレーム、上限超過フレーム、および未実装の `Clear` / `Text` は `Rejected` を返す。
上限超過時は次の区切りまでを 1 フレームとして破棄する。破棄回数は `u32::MAX` で飽和する。
応答 1 件分だけを保持し、USB FIFO に渡すまで次の要求の読み出しを待つ。
USB の送受信は非ブロッキング API を使用する。

host は要求の送信前に入力バッファを破棄し、起動直後に残る bootloader のログを除外する。
USB 側に残ったログとも区別できるよう、firmware は応答の前に同期用の 0x00 を送る。
host は空の区切り、不正・過長フレームを読み飛ばし、次の区切りから復帰する。
読み飛ばしを含む受信ループには 1 秒の期限を設ける。読み取り自体も 1 秒でタイムアウトする。

表示は起動時の確認画面を維持する。Card 表示と fuzz 検証を含む Phase 2 全体は未完了。

## 不変条件

- 可変長のフィールドはすべて `heapless` の上限付き型で表す。上限を超える入力は `Deserialize` の時点で失敗し、firmware のメモリに到達しない (`oversized_text_is_rejected` テスト)
- firmware は `Message` の variant に対応する描画以外の動作を行わない
- テキストは UTF-8 として妥当であることを `heapless::String` の `Deserialize` が保証する。制御文字は描画側で無視する
- 画像はヘッダで宣言した幅・高さ・オフセットが画面内に収まり、宣言長とデータ長が一致する場合のみ受理する (Phase 2)

## 検証

- 単体テスト: 往復、上限ちょうど、上限超過、不正バイト列
- fuzz: `cargo fuzz` で `decode` に任意のバイト列を与え、panic しないことを確認する (Phase 2 で追加。fuzz 用の toolchain は別途固定する)
