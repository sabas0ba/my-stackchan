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
pub const VERSION: u8 = 7;

/// 1 メッセージ中のテキストの最大バイト数 (UTF-8)。
pub const MAX_TEXT_BYTES: usize = 512;

/// COBS 符号化後のフレームの最大バイト数。firmware 側の受信バッファの大きさ。
/// postcard の出力に対し、COBS は 254 バイトごとに 1 バイトのオーバーヘッドを持つ。
pub const MAX_FRAME_BYTES: usize = 1024;

/// Card の行数と、各行に並べる要素数の上限。
pub const MAX_CARD_ROWS: usize = 4;
pub const MAX_ROW_ELEMENTS: usize = 2;
pub const MAX_CARD_TEXT_BYTES: usize = 48;
pub const MAX_BAR_LABEL_BYTES: usize = 12;
/// 画像は Card あたり 1 枚に限定し、RGB565 の 2 byte/pixel で送る。
pub const MAX_IMAGE_SIDE: u8 = 16;
pub const MAX_IMAGE_BYTES: usize = 512;
pub const MAX_STATUS_DETAIL_BYTES: usize = 20;

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

/// PC が共有する活動状態。取得元は host 側で選ぶ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Activity {
    Idle,
    Working,
    Waiting,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expression {
    Happy,
    Focused,
    Sleepy,
    Worried,
    Surprised,
    Grin,
    Calm,
    Curious,
    Playful,
    Wink,
    Sad,
    Determined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gaze {
    Center,
    Left,
    Right,
    Up,
    Down,
    /// 画面上の視線を -100..100 の二軸で指定する。
    Point {
        x: i8,
        y: i8,
    },
}

impl Gaze {
    pub fn validate(self) -> Result<(), &'static str> {
        if let Self::Point { x, y } = self
            && (!(-100..=100).contains(&x) || !(-100..=100).contains(&y))
        {
            return Err("視線の座標は -100..100 にしてください");
        }
        Ok(())
    }
}

/// 目の開き方。Auto は表情ごとの既定形を使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EyeStyle {
    Auto,
    Open,
    Wide,
    Closed,
    HalfLidded,
}

/// 顔と PC 状態をまとめて更新する。TTL が満了すると既定の顔に戻る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub activity: Option<Activity>,
    pub detail: heapless::String<MAX_STATUS_DETAIL_BYTES>,
    pub expression: Expression,
    pub gaze: Gaze,
    pub eyes: EyeStyle,
    pub ttl_s: u16,
}

/// 一時的な表情。期限満了後は直前の Presence に戻る。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Emote {
    pub expression: Expression,
    pub gaze: Gaze,
    pub eyes: EyeStyle,
    /// 身振りと発光の強さ。0 は無効、100 は上限。
    pub intensity: u8,
    pub duration_ms: u16,
}

impl Emote {
    pub fn validate(self) -> Result<(), &'static str> {
        self.gaze.validate()?;
        if self.intensity > 100 {
            return Err("Emote の強度は 0..100 にしてください");
        }
        if !(100..=10_000).contains(&self.duration_ms) {
            return Err("Emote の継続時間は 100..10000 ms にしてください");
        }
        Ok(())
    }
}

/// 実機固有のピッチ補正。1 step は約 0.3125 度。ESP の RAM にのみ保持する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PitchTrim {
    pub raw_steps: i16,
}

impl PitchTrim {
    pub fn validate(self) -> Result<(), &'static str> {
        if !(-96..=64).contains(&self.raw_steps) {
            return Err("ピッチ補正は -96..64 step にしてください");
        }
        Ok(())
    }
}

/// Column -> Row -> Element の 2 段に限定した表示内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Card {
    pub slot: Slot,
    pub ttl_s: u16,
    pub rows: heapless::Vec<Row, MAX_CARD_ROWS>,
    pub image: Option<ImageData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageData {
    pub width: u8,
    pub height: u8,
    /// RGB565 の big-endian byte 列。
    pub pixels: heapless::Vec<u8, MAX_IMAGE_BYTES>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub elements: heapless::Vec<Element, MAX_ROW_ELEMENTS>,
}

impl Row {
    pub fn height(&self) -> u16 {
        self.elements
            .iter()
            .map(|element| match element {
                Element::Text { .. } | Element::Bar { .. } | Element::Image => 20,
                Element::Spacer { height } => u16::from(*height),
            })
            .max()
            .unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Element {
    Text {
        text: heapless::String<MAX_CARD_TEXT_BYTES>,
    },
    Bar {
        ratio: u8,
        label: heapless::String<MAX_BAR_LABEL_BYTES>,
    },
    Spacer {
        height: u8,
    },
    Image,
}

impl Card {
    /// 表示領域内へ全行を収め、無意味または範囲外の値を受理しない。
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.rows.is_empty() {
            return Err("Card に行がありません");
        }
        let mut used_height = 8u16;
        let mut image_count = 0;
        for row in &self.rows {
            if row.elements.is_empty() {
                return Err("Card に空の行があります");
            }
            for element in &row.elements {
                match element {
                    Element::Bar { ratio, .. } if *ratio > 100 => {
                        return Err("バーの比率は 0..100 にしてください");
                    }
                    Element::Spacer { height } if !(1..=32).contains(height) => {
                        return Err("余白の高さは 1..32 px にしてください");
                    }
                    Element::Image => image_count += 1,
                    _ => {}
                }
            }
            used_height += row.height();
        }
        let area_height = match self.slot {
            Slot::Overlay => 240,
            _ => 48,
        };
        if used_height > area_height {
            return Err("Card が表示領域の高さを超えます");
        }
        match (&self.image, image_count) {
            (None, 0) => {}
            (Some(image), 1)
                if (1..=MAX_IMAGE_SIDE).contains(&image.width)
                    && (1..=MAX_IMAGE_SIDE).contains(&image.height)
                    && image.pixels.len()
                        == usize::from(image.width) * usize::from(image.height) * 2 => {}
            _ => return Err("Card の画像は 16x16 px 以下を 1 枚、宣言長どおりに指定してください"),
        }
        Ok(())
    }
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
    /// 複数行の表示要素を 1 つの Slot に配置する。
    Card(Card),
    /// 顔の表情・視線と PC の活動状態を同時に更新する。
    Presence(Presence),
    /// 一時的な表情を重ね、期限満了時に Presence へ戻す。
    Emote(Emote),
    /// 本体側の I²C 拡張器の接続状態を読み取る。表示と機構部は変更しない。
    HardwareProbe,
    /// ピッチの中心位置を RAM 上だけで補正する。
    PitchTrim(PitchTrim),
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
    /// 拡張器の実測値。None は未応答または無効なバージョン値を表す。
    HardwareStatus {
        version_6f: Option<u8>,
        version_71: Option<u8>,
        vm_out: Option<u8>,
        led_config: Option<u8>,
        bus_out: Option<u8>,
        boost_out: Option<u8>,
        servo_x_position: Option<u16>,
        servo_y_position: Option<u16>,
        servo_x_goal: Option<u16>,
        servo_y_goal: Option<u16>,
        servo_x_min_limit: Option<u16>,
        servo_x_max_limit: Option<u16>,
        servo_y_min_limit: Option<u16>,
        servo_y_max_limit: Option<u16>,
        servo_x_voltage: Option<u8>,
        servo_y_voltage: Option<u8>,
        pitch_trim_raw_steps: i16,
        servo_x_torque: Option<bool>,
        servo_y_torque: Option<bool>,
        enabled: bool,
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
    fn hardware_probe_and_status_roundtrip() {
        let mut frame = [0; 64];
        let encoded = encode(&Message::HardwareProbe, &mut frame).unwrap();
        assert_eq!(
            decode::<Message>(&mut encoded.to_vec()),
            Ok(Message::HardwareProbe)
        );
        let status = Reply::HardwareStatus {
            version_6f: None,
            version_71: Some(2),
            vm_out: Some(1),
            led_config: Some(12),
            bus_out: Some(2),
            boost_out: Some(128),
            servo_x_position: Some(460),
            servo_y_position: Some(764),
            servo_x_goal: Some(460),
            servo_y_goal: Some(764),
            servo_x_min_limit: Some(0),
            servo_x_max_limit: Some(1000),
            servo_y_min_limit: Some(0),
            servo_y_max_limit: Some(1000),
            servo_x_voltage: Some(73),
            servo_y_voltage: Some(72),
            pitch_trim_raw_steps: -24,
            servo_x_torque: Some(false),
            servo_y_torque: Some(false),
            enabled: false,
        };
        let encoded = encode(&status, &mut frame).unwrap();
        assert_eq!(decode::<Reply>(&mut encoded.to_vec()), Ok(status));
    }

    #[test]
    fn pitch_trim_is_bounded_and_roundtrips() {
        assert_eq!(PitchTrim { raw_steps: -96 }.validate(), Ok(()));
        assert_eq!(PitchTrim { raw_steps: 64 }.validate(), Ok(()));
        assert!(PitchTrim { raw_steps: -97 }.validate().is_err());
        assert!(PitchTrim { raw_steps: 65 }.validate().is_err());
        let message = Message::PitchTrim(PitchTrim { raw_steps: -24 });
        let mut frame = [0; 32];
        let encoded = encode(&message, &mut frame).unwrap();
        assert_eq!(decode::<Message>(&mut encoded.to_vec()), Ok(message));
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

    fn sample_card(slot: Slot) -> Card {
        let mut rows = heapless::Vec::new();
        rows.push(Row {
            elements: heapless::Vec::from_slice(&[
                Element::Text {
                    text: "CPU".try_into().unwrap(),
                },
                Element::Text {
                    text: "Usage".try_into().unwrap(),
                },
            ])
            .unwrap(),
        })
        .unwrap();
        rows.push(Row {
            elements: heapless::Vec::from_slice(&[Element::Bar {
                ratio: 75,
                label: "75%".try_into().unwrap(),
            }])
            .unwrap(),
        })
        .unwrap();
        Card {
            slot,
            ttl_s: 30,
            rows,
            image: None,
        }
    }

    #[test]
    fn card_roundtrip_and_legacy_message_encoding() {
        let card = sample_card(Slot::BannerTop);
        assert_eq!(card.validate(), Ok(()));
        let message = Message::Card(card);
        let mut frame = [0; MAX_FRAME_BYTES];
        let len = encode(&message, &mut frame).unwrap().len();
        let mut copy = [0; MAX_FRAME_BYTES];
        copy[..len].copy_from_slice(&frame[..len]);
        assert_eq!(decode::<Message>(&mut copy[..len]), Ok(message));

        // 既存 variant の番号は変えず、版 1 の host と firmware の間でのみ Card を送る。
        for (message, variant) in [
            (Message::Ping { nonce: 1 }, 0),
            (Message::Clear, 1),
            (
                Message::Text {
                    slot: Slot::Overlay,
                    ttl_s: 0,
                    text: "x".try_into().unwrap(),
                },
                2,
            ),
        ] {
            let len = encode(&message, &mut frame).unwrap().len();
            copy[..len].copy_from_slice(&frame[..len]);
            let decoded: Message = decode(&mut copy[..len]).unwrap();
            assert_eq!(decoded, message);
            assert_eq!(copy[0], variant);
        }
    }

    #[test]
    fn card_validation_rejects_out_of_range_layout_and_bar() {
        let mut card = sample_card(Slot::BannerTop);
        card.rows[1].elements[0] = Element::Bar {
            ratio: 101,
            label: "bad".try_into().unwrap(),
        };
        assert!(card.validate().is_err());
        card.rows[1].elements[0] = Element::Spacer { height: 32 };
        assert!(card.validate().is_err());
        card.rows[1].elements[0] = Element::Spacer { height: 0 };
        assert!(card.validate().is_err());
        card.slot = Slot::Overlay;
        card.rows[1].elements[0] = Element::Spacer { height: 32 };
        assert_eq!(card.validate(), Ok(()));
        card.rows.clear();
        assert!(card.validate().is_err());
    }

    #[test]
    fn forged_card_with_too_many_rows_is_rejected_on_decode() {
        #[derive(Serialize)]
        struct ForgedCard {
            slot: Slot,
            ttl_s: u16,
            rows: heapless::Vec<Row, 5>,
        }
        #[derive(Serialize)]
        #[allow(clippy::large_enum_variant)]
        enum ForgedMessage {
            #[allow(dead_code)]
            Ping {
                nonce: u32,
            },
            #[allow(dead_code)]
            Clear,
            #[allow(dead_code)]
            Text {
                slot: Slot,
                ttl_s: u16,
                text: &'static str,
            },
            Card(ForgedCard),
        }
        let row = Row {
            elements: heapless::Vec::from_slice(&[Element::Text {
                text: "x".try_into().unwrap(),
            }])
            .unwrap(),
        };
        let mut rows = heapless::Vec::<Row, 5>::new();
        for _ in 0..5 {
            rows.push(row.clone()).unwrap();
        }
        let forged = ForgedMessage::Card(ForgedCard {
            slot: Slot::Overlay,
            ttl_s: 0,
            rows,
        });
        let mut frame = [0; MAX_FRAME_BYTES];
        let len = encode(&forged, &mut frame).unwrap().len();
        let mut copy = [0; MAX_FRAME_BYTES];
        copy[..len].copy_from_slice(&frame[..len]);
        assert_eq!(decode::<Message>(&mut copy[..len]), Err(Error::Malformed));
    }

    #[test]
    fn inline_image_requires_one_marker_and_exact_rgb565_length() {
        let mut card = sample_card(Slot::BannerTop);
        let pixels = heapless::Vec::from_slice(&[0xF8, 0x00, 0x07, 0xE0]).unwrap();
        card.image = Some(ImageData {
            width: 2,
            height: 1,
            pixels,
        });
        assert!(card.validate().is_err());
        card.rows[1].elements[0] = Element::Image;
        assert_eq!(card.validate(), Ok(()));
        card.rows[0].elements[1] = Element::Image;
        assert!(card.validate().is_err());
        card.rows[0].elements[1] = Element::Text {
            text: "Usage".try_into().unwrap(),
        };
        card.image.as_mut().unwrap().pixels.pop();
        assert!(card.validate().is_err());
        card.image.as_mut().unwrap().width = 17;
        assert!(card.validate().is_err());
        card.image = None;
        assert!(card.validate().is_err());
    }

    #[test]
    fn largest_inline_image_and_text_card_fits_in_frame() {
        let mut card = Card {
            slot: Slot::Overlay,
            ttl_s: 0,
            rows: heapless::Vec::new(),
            image: Some(ImageData {
                width: 16,
                height: 16,
                pixels: heapless::Vec::from_slice(&[0xFF; MAX_IMAGE_BYTES]).unwrap(),
            }),
        };
        let mut long_text = heapless::String::<MAX_CARD_TEXT_BYTES>::new();
        for _ in 0..MAX_CARD_TEXT_BYTES {
            long_text.push('x').unwrap();
        }
        for row_index in 0..MAX_CARD_ROWS {
            let mut row = Row {
                elements: heapless::Vec::new(),
            };
            for column_index in 0..MAX_ROW_ELEMENTS {
                row.elements
                    .push(if row_index == 0 && column_index == 0 {
                        Element::Image
                    } else {
                        Element::Text {
                            text: long_text.clone(),
                        }
                    })
                    .unwrap();
            }
            card.rows.push(row).unwrap();
        }
        assert_eq!(card.validate(), Ok(()));
        let message = Message::Card(card);
        let mut frame = [0; MAX_FRAME_BYTES];
        let len = encode(&message, &mut frame).unwrap().len();
        assert!(len <= MAX_FRAME_BYTES);
        let mut received = frame;
        assert_eq!(decode::<Message>(&mut received[..len]), Ok(message));
    }

    #[test]
    fn presence_roundtrip_preserves_existing_variant_numbers() {
        let message = Message::Presence(Presence {
            activity: Some(Activity::Working),
            detail: "Building".try_into().unwrap(),
            expression: Expression::Focused,
            gaze: Gaze::Right,
            eyes: EyeStyle::HalfLidded,
            ttl_s: 30,
        });
        let mut frame = [0; MAX_FRAME_BYTES];
        let len = encode(&message, &mut frame).unwrap().len();
        let mut received = frame;
        assert_eq!(decode::<Message>(&mut received[..len]), Ok(message));
        assert_eq!(received[0], 4);
        assert!(
            heapless::String::<MAX_STATUS_DETAIL_BYTES>::try_from("x".repeat(21).as_str()).is_err()
        );
    }

    #[test]
    fn emote_roundtrip_and_parameter_validation() {
        let emote = Emote {
            expression: Expression::Curious,
            gaze: Gaze::Point { x: -65, y: 40 },
            eyes: EyeStyle::Open,
            intensity: 75,
            duration_ms: 750,
        };
        let message = Message::Emote(emote);
        let mut frame = [0; MAX_FRAME_BYTES];
        let len = encode(&message, &mut frame).unwrap().len();
        let mut received = frame;
        assert_eq!(decode::<Message>(&mut received[..len]), Ok(message));
        assert_eq!(received[0], 5);
        assert_eq!(emote.validate(), Ok(()));
        assert!(
            Emote {
                duration_ms: 99,
                ..emote
            }
            .validate()
            .is_err()
        );
        assert!(
            Emote {
                gaze: Gaze::Point { x: 101, y: 0 },
                ..emote
            }
            .validate()
            .is_err()
        );
        assert!(
            Emote {
                intensity: 101,
                ..emote
            }
            .validate()
            .is_err()
        );
    }
}
