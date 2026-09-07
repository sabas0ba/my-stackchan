//! host と firmware が共有するプロトコル定義。
//!
//! 本 crate は no_std で動作し、動的確保を行わない。すべての可変長データは
//! 上限付きの `heapless` 型で表す。上限の値は firmware 側の固定長バッファの
//! 根拠であり、変更する場合は両側の影響を確認する。
//!
//! フレーミングは COBS を用い、0x00 をフレーム終端とする。受信側はフレーム境界で
//! 同期を取り直せるため、任意のバイト列が混入しても復帰できる。
//! 詳細は docs/protocol.md を参照する。

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// プロトコルの版。互換性の無い変更を行った場合に増やす。
pub const VERSION: u8 = 0;

/// 1 メッセージ中のテキストの最大バイト数 (UTF-8)。
pub const MAX_TEXT_BYTES: usize = 512;

/// COBS 符号化後のフレームの最大バイト数。firmware 側の受信バッファの大きさ。
/// postcard の出力に対し、COBS は 254 バイトごとに 1 バイトのオーバーヘッドを持つ。
pub const MAX_FRAME_BYTES: usize = 1024;

/// 表示位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Slot {
    /// 画面上部の帯。
    BannerTop,
    /// 画面下部の帯。
    BannerBottom,
    /// 全画面。顔を隠す。
    Overlay,
}

/// host から firmware へ送るメッセージ。
///
/// firmware は本 enum の variant に対応する描画以外の動作を行わない。
///
/// variant 間の大きさの差は意図したものである。動的確保を行わない方針のため、
/// 最大の variant に合わせた固定長の領域に受信する。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// 疎通確認。firmware は同じ nonce を `Reply::Pong` で返す。
    Ping { nonce: u32 },
    /// すべての slot の内容を消し、顔のみの表示に戻す。
    Clear,
    /// テキストを表示する。`ttl_s` 秒後に自動的に消える (0 は消えない)。
    Text {
        slot: Slot,
        ttl_s: u16,
        text: heapless::String<MAX_TEXT_BYTES>,
    },
}

/// firmware から host へ返す応答。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    Pong {
        nonce: u32,
        version: u8,
    },
    /// 受理したメッセージの通し番号。
    Ack {
        seq: u32,
    },
    /// 復号または検証に失敗した回数の累計。診断用。
    Rejected {
        count: u32,
    },
}

/// encode/decode の失敗。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// 出力バッファが不足した。
    BufferTooSmall,
    /// 入力がフレームとして解釈できない。
    Malformed,
}

impl From<postcard::Error> for Error {
    fn from(e: postcard::Error) -> Self {
        match e {
            postcard::Error::SerializeBufferFull => Error::BufferTooSmall,
            _ => Error::Malformed,
        }
    }
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::BufferTooSmall => f.write_str("buffer too small"),
            Error::Malformed => f.write_str("malformed frame"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

/// 値を COBS フレームとして `buf` に書き込み、書き込んだ部分を返す。
/// 返り値の末尾は 0x00 (フレーム終端) である。
pub fn encode<'a, T: Serialize>(value: &T, buf: &'a mut [u8]) -> Result<&'a [u8], Error> {
    Ok(postcard::to_slice_cobs(value, buf)?)
}

/// 1 フレーム分のバイト列 (終端の 0x00 を含んでよい) を復号する。
/// 復号は入力を書き換えるため `&mut` を取る。
pub fn decode<'a, T: Deserialize<'a>>(frame: &'a mut [u8]) -> Result<T, Error> {
    Ok(postcard::from_bytes_cobs(frame)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_roundtrip() {
        let mut buf = [0u8; 32];
        let msg = Message::Ping { nonce: 0xDEAD_BEEF };
        let frame = encode(&msg, &mut buf).unwrap();
        assert_eq!(*frame.last().unwrap(), 0);
        let mut copy = [0u8; 32];
        copy[..frame.len()].copy_from_slice(frame);
        let decoded: Message = decode(&mut copy[..frame.len()]).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn text_at_limit_fits_in_frame() {
        let mut text = heapless::String::<MAX_TEXT_BYTES>::new();
        for _ in 0..MAX_TEXT_BYTES {
            text.push('a').unwrap();
        }
        let msg = Message::Text {
            slot: Slot::Overlay,
            ttl_s: 30,
            text,
        };
        let mut buf = [0u8; MAX_FRAME_BYTES];
        let frame = encode(&msg, &mut buf).unwrap();
        assert!(frame.len() <= MAX_FRAME_BYTES);
    }

    #[test]
    fn garbage_is_rejected_not_panicking() {
        let mut junk = [0xFFu8, 0x01, 0x02, 0x00];
        let result: Result<Message, Error> = decode(&mut junk);
        assert!(result.is_err());
    }

    #[test]
    fn oversized_text_is_rejected() {
        // MAX_TEXT_BYTES を超えるテキストを host 側で偽装して送った場合、
        // heapless::String の Deserialize が容量超過として失敗すること。
        let mut long = [b'b'; MAX_TEXT_BYTES + 1];
        long[MAX_TEXT_BYTES] = b'c';
        #[derive(Serialize)]
        enum Forged<'a> {
            #[allow(dead_code)]
            Ping { nonce: u32 },
            #[allow(dead_code)]
            Clear,
            Text {
                slot: Slot,
                ttl_s: u16,
                text: &'a str,
            },
        }
        let forged = Forged::Text {
            slot: Slot::Overlay,
            ttl_s: 1,
            text: core::str::from_utf8(&long).unwrap(),
        };
        let mut buf = [0u8; MAX_FRAME_BYTES];
        let frame = postcard::to_slice_cobs(&forged, &mut buf).unwrap();
        let mut copy = [0u8; MAX_FRAME_BYTES];
        copy[..frame.len()].copy_from_slice(frame);
        let result: Result<Message, Error> = decode(&mut copy[..frame.len()]);
        assert_eq!(result, Err(Error::Malformed));
    }
}
