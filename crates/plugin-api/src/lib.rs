//! plugin と host daemon の間のメッセージ定義。
//!
//! plugin は daemon の子プロセスとして起動され、stdin / stdout 上で postcard を COBS で
//! 符号化したフレーム (0x00 終端) を交換する。stderr は daemon がログとして記録する。
//!
//! デバイスプロトコル (`protocol` crate) とは独立に版を持つ。daemon が本 crate の型を
//! デバイスプロトコルへ変換するため、firmware 側の変更は plugin に波及しない。
//! 本 crate の検証は大きさの一般的な上限だけを扱い、表示領域に収まるか等のデバイス
//! 固有の検証は daemon が行う。設計は docs/plugin.md を参照する。

#![forbid(unsafe_code)]

pub mod client;
mod frame;

pub use frame::{FrameError, FrameReader, write_frame};

use serde::{Deserialize, Serialize};

/// plugin API の版。互換性の無い変更を行った場合に増やす。
pub const API_VERSION: u16 = 1;

/// COBS 符号化後のフレームの最大バイト数。受信側はこれを超える入力を確保せずに破棄する。
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

pub const MAX_NAME_BYTES: usize = 32;
pub const MAX_VERSION_BYTES: usize = 32;
pub const MAX_LOG_BYTES: usize = 1024;
pub const MAX_REASON_BYTES: usize = 256;
/// Card や通知の文字列の一般的な上限。表示可能な長さは `Limits` で別に通知する。
pub const MAX_TEXT_BYTES: usize = 256;
pub const MAX_CARD_ROWS: usize = 8;
pub const MAX_ROW_ELEMENTS: usize = 4;
/// 1 plugin が同時に持てる Card の数の上限。実際の許可数は利用者設定で決まる。
pub const MAX_CARDS: u8 = 8;
pub const MAX_PARAMS: usize = 32;
pub const MAX_PARAM_KEY_BYTES: usize = 64;
pub const MAX_PARAM_VALUE_BYTES: usize = 1024;

/// plugin が要求し、利用者設定が許可する権限。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// 同時に持てる Card の数。
    pub cards: u8,
    /// 通知による割込み。
    pub notify: bool,
    /// 表情・活動状態 (Presence と、画面上だけの Emote)。
    pub presence: bool,
    /// Emote による機構部 (サーボ、LED) の駆動。`presence` と併せて許可する。
    pub motion: bool,
}

impl Capabilities {
    /// `requested` のうち `self` (許可) の範囲に収まる部分を返す。
    pub fn intersect(self, requested: Self) -> Self {
        Self {
            cards: self.cards.min(requested.cards),
            notify: self.notify && requested.notify,
            presence: self.presence && requested.presence,
            motion: self.presence && requested.presence && self.motion && requested.motion,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub api_version: u16,
    pub name: String,
    pub version: String,
    pub capabilities: Capabilities,
}

/// Card を置く領域の種別。どの帯に置くかは daemon が決める。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Placement {
    Banner,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Element {
    Text { text: String },
    Bar { ratio: u8, label: String },
    Spacer { height: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub elements: Vec<Element>,
    /// タップ時に `HostMessage::Action` で返す値。None の行はタップを plugin に送らない。
    pub action: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardPut {
    /// plugin 内で一意な Card の識別子。同じ値の CardPut は内容を置き換える。
    pub card: u8,
    pub placement: Placement,
    pub priority: Priority,
    /// 表示を保つ秒数。0 は CardRemove まで保つ。
    pub ttl_s: u16,
    pub rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notify {
    pub text: String,
    pub priority: Priority,
    pub ttl_s: u16,
}

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
    /// -100..100 の二軸。
    Point {
        x: i8,
        y: i8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EyeStyle {
    Auto,
    Open,
    Wide,
    Closed,
    HalfLidded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presence {
    pub activity: Option<Activity>,
    pub detail: String,
    pub expression: Expression,
    pub gaze: Gaze,
    pub eyes: EyeStyle,
    pub ttl_s: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Emote {
    pub expression: Expression,
    pub gaze: Gaze,
    pub eyes: EyeStyle,
    /// 0..100。capability `motion` が無い場合、daemon は 0 に置き換える。
    pub intensity: u8,
    pub duration_ms: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

/// plugin から daemon へ送るメッセージ。variant の追加は末尾に限る (postcard の
/// variant 番号を維持するため)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginMessage {
    /// 最初のメッセージでなければならない。
    Hello(Hello),
    CardPut(CardPut),
    CardRemove {
        card: u8,
    },
    Notify(Notify),
    Presence(Presence),
    Emote(Emote),
    Log {
        level: LogLevel,
        text: String,
    },
}

/// デバイスが表示できる大きさ。plugin はこの範囲で Card を組み立てる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    /// 帯に置ける 20 px 行の数。
    pub banner_rows: u8,
    /// Overlay に置ける 20 px 行の数。
    pub overlay_rows: u8,
    pub row_elements: u8,
    /// Card の Text 要素の最大バイト数。表示できるのは ASCII のみ。
    pub text_bytes: u16,
    pub bar_label_bytes: u16,
    pub notify_bytes: u16,
    pub presence_detail_bytes: u16,
}

/// Debug は params の値を出さない (下の impl)。secret を含み得るため、plugin 作者が
/// `{:?}` でログに出しても漏れないようにする。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Init {
    pub api_version: u16,
    /// 要求と利用者設定の許可の共通部分。これを超えるメッセージは拒否される。
    pub granted: Capabilities,
    /// 利用者設定の `param.*` (secret を含む)。path 等の環境依存の値はここから受け取る。
    pub params: Vec<(String, String)>,
    pub limits: Limits,
}

impl std::fmt::Debug for Init {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.params.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("Init")
            .field("api_version", &self.api_version)
            .field("granted", &self.granted)
            .field("params", &format_args!("{names:?} (値は伏せる)"))
            .field("limits", &self.limits)
            .finish()
    }
}

/// daemon から plugin へ送るメッセージ。variant の追加は末尾に限る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostMessage {
    Init(Init),
    Visibility {
        card: u8,
        visible: bool,
    },
    Action {
        card: u8,
        action: u8,
    },
    Rejected {
        reason: String,
    },
    /// 終了要求。猶予時間内に終了しない plugin は停止される。
    Shutdown,
}

fn check_len(value: &str, max: usize, what: &'static str) -> Result<(), &'static str> {
    if value.len() > max { Err(what) } else { Ok(()) }
}

fn check_gaze(gaze: Gaze) -> Result<(), &'static str> {
    match gaze {
        Gaze::Point { x, y } if !(-100..=100).contains(&x) || !(-100..=100).contains(&y) => {
            Err("視線の座標は -100..100 にしてください")
        }
        _ => Ok(()),
    }
}

impl PluginMessage {
    /// 大きさと値域の一般的な検証。表示領域に収まるかは daemon が別に検証する。
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Hello(hello) => {
                check_len(&hello.name, MAX_NAME_BYTES, "名前が長すぎます")?;
                if hello.name.is_empty() {
                    return Err("名前が空です");
                }
                check_len(&hello.version, MAX_VERSION_BYTES, "版の文字列が長すぎます")?;
                if hello.capabilities.cards > MAX_CARDS {
                    return Err("Card 数の要求が上限を超えます");
                }
            }
            Self::CardPut(card) => {
                if card.card >= MAX_CARDS {
                    return Err("Card の識別子が範囲外です");
                }
                if card.rows.is_empty() || card.rows.len() > MAX_CARD_ROWS {
                    return Err("Card の行数が範囲外です");
                }
                for row in &card.rows {
                    if row.elements.is_empty() || row.elements.len() > MAX_ROW_ELEMENTS {
                        return Err("行の要素数が範囲外です");
                    }
                    for element in &row.elements {
                        match element {
                            Element::Text { text } => {
                                check_len(text, MAX_TEXT_BYTES, "Text が長すぎます")?
                            }
                            Element::Bar { ratio, label } => {
                                if *ratio > 100 {
                                    return Err("バーの比率は 0..100 にしてください");
                                }
                                check_len(label, MAX_TEXT_BYTES, "ラベルが長すぎます")?;
                            }
                            Element::Spacer { height } => {
                                if !(1..=32).contains(height) {
                                    return Err("余白の高さは 1..32 px にしてください");
                                }
                            }
                        }
                    }
                }
            }
            Self::CardRemove { card } => {
                if *card >= MAX_CARDS {
                    return Err("Card の識別子が範囲外です");
                }
            }
            Self::Notify(notify) => {
                check_len(&notify.text, MAX_TEXT_BYTES, "通知が長すぎます")?;
                if notify.text.is_empty() {
                    return Err("通知が空です");
                }
            }
            Self::Presence(presence) => {
                check_len(&presence.detail, MAX_TEXT_BYTES, "詳細が長すぎます")?;
                check_gaze(presence.gaze)?;
            }
            Self::Emote(emote) => {
                check_gaze(emote.gaze)?;
                if emote.intensity > 100 {
                    return Err("Emote の強度は 0..100 にしてください");
                }
                if !(100..=10_000).contains(&emote.duration_ms) {
                    return Err("Emote の継続時間は 100..10000 ms にしてください");
                }
            }
            Self::Log { text, .. } => check_len(text, MAX_LOG_BYTES, "ログが長すぎます")?,
        }
        Ok(())
    }
}

impl HostMessage {
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Init(init) => {
                if init.params.len() > MAX_PARAMS {
                    return Err("設定値の数が上限を超えます");
                }
                for (key, value) in &init.params {
                    check_len(key, MAX_PARAM_KEY_BYTES, "設定値の名前が長すぎます")?;
                    check_len(value, MAX_PARAM_VALUE_BYTES, "設定値が長すぎます")?;
                }
            }
            Self::Rejected { reason } => {
                check_len(reason, MAX_REASON_BYTES, "拒否理由が長すぎます")?
            }
            Self::Visibility { .. } | Self::Action { .. } | Self::Shutdown => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_card(text: &str) -> PluginMessage {
        PluginMessage::CardPut(CardPut {
            card: 0,
            placement: Placement::Banner,
            priority: Priority::Normal,
            ttl_s: 0,
            rows: vec![Row {
                elements: vec![Element::Text { text: text.into() }],
                action: None,
            }],
        })
    }

    #[test]
    fn granted_capabilities_never_exceed_request_or_allowance() {
        let allowed = Capabilities {
            cards: 2,
            notify: true,
            presence: false,
            motion: true,
        };
        let requested = Capabilities {
            cards: 8,
            notify: false,
            presence: true,
            motion: true,
        };
        assert_eq!(
            allowed.intersect(requested),
            Capabilities {
                cards: 2,
                notify: false,
                presence: false,
                // motion は presence 無しでは意味を持たないため許可しない。
                motion: false,
            }
        );
    }

    #[test]
    fn text_at_limit_is_accepted_and_beyond_is_rejected() {
        assert_eq!(text_card(&"x".repeat(MAX_TEXT_BYTES)).validate(), Ok(()));
        assert!(
            text_card(&"x".repeat(MAX_TEXT_BYTES + 1))
                .validate()
                .is_err()
        );
    }

    #[test]
    fn empty_and_oversized_structures_are_rejected() {
        let PluginMessage::CardPut(mut card) = text_card("x") else {
            unreachable!()
        };
        card.rows.clear();
        assert!(PluginMessage::CardPut(card.clone()).validate().is_err());
        card.rows = vec![
            Row {
                elements: vec![Element::Spacer { height: 1 }],
                action: None,
            };
            MAX_CARD_ROWS + 1
        ];
        assert!(PluginMessage::CardPut(card.clone()).validate().is_err());
        card.rows.truncate(1);
        card.card = MAX_CARDS;
        assert!(PluginMessage::CardPut(card).validate().is_err());
    }

    #[test]
    fn emote_and_gaze_ranges_follow_device_protocol() {
        let emote = Emote {
            expression: Expression::Happy,
            gaze: Gaze::Point { x: -100, y: 100 },
            eyes: EyeStyle::Auto,
            intensity: 100,
            duration_ms: 100,
        };
        assert_eq!(PluginMessage::Emote(emote).validate(), Ok(()));
        for invalid in [
            Emote {
                gaze: Gaze::Point { x: -101, y: 0 },
                ..emote
            },
            Emote {
                intensity: 101,
                ..emote
            },
            Emote {
                duration_ms: 10_001,
                ..emote
            },
        ] {
            assert!(PluginMessage::Emote(invalid).validate().is_err());
        }
    }

    #[test]
    fn init_debug_does_not_show_parameter_values() {
        let init = Init {
            api_version: API_VERSION,
            granted: Capabilities::default(),
            params: vec![("access_code".into(), "s3cr3t-value".into())],
            limits: Limits {
                banner_rows: 2,
                overlay_rows: 4,
                row_elements: 2,
                text_bytes: 48,
                bar_label_bytes: 12,
                notify_bytes: 512,
                presence_detail_bytes: 20,
            },
        };
        for text in [
            format!("{init:?}"),
            format!("{:?}", HostMessage::Init(init.clone())),
        ] {
            assert!(!text.contains("s3cr3t-value"), "{text}");
            assert!(text.contains("access_code"), "{text}");
        }
    }

    #[test]
    fn hello_requires_name_and_bounded_card_request() {
        let hello = Hello {
            api_version: API_VERSION,
            name: "clock".into(),
            version: "0.1.0".into(),
            capabilities: Capabilities {
                cards: MAX_CARDS,
                ..Capabilities::default()
            },
        };
        assert_eq!(PluginMessage::Hello(hello.clone()).validate(), Ok(()));
        let mut unnamed = hello.clone();
        unnamed.name.clear();
        assert!(PluginMessage::Hello(unnamed).validate().is_err());
        let mut greedy = hello;
        greedy.capabilities.cards = MAX_CARDS + 1;
        assert!(PluginMessage::Hello(greedy).validate().is_err());
    }
}
