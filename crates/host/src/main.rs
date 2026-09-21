//! PC 側 CLI。
//!
//! port の列挙、疎通確認、テキストと Card の表示を提供する。
//! 使用量の取得 (collector) は docs/design.md のフェーズに従って追加する。

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand, ValueEnum};

mod bmp;

/// Espressif の USB Serial/JTAG が名乗る VID:PID。port の自動検出に使う。
const ESP_USB_SERIAL_JTAG: (u16, u16) = (0x303A, 0x1001);

#[derive(Parser)]
#[command(name = "stackchan", version, about = "my-stackchan host CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 表情と視線を表示する。
    Face {
        #[arg(long)]
        port: Option<String>,
        #[arg(long, value_enum, default_value = "happy")]
        expression: FaceExpression,
        #[arg(long, value_enum, default_value = "center")]
        gaze: FaceGaze,
        #[arg(long, value_enum, default_value = "auto")]
        eyes: HostEyeStyle,
        #[arg(long, default_value_t = 0)]
        ttl: u16,
    },
    /// PC の活動状態を表情とともに表示する。
    Status {
        #[arg(long)]
        port: Option<String>,
        #[arg(long, value_enum)]
        activity: HostActivity,
        #[arg(long, default_value = "")]
        detail: String,
        #[arg(long, value_enum, default_value = "center")]
        gaze: FaceGaze,
        #[arg(long, value_enum, default_value = "auto")]
        eyes: HostEyeStyle,
        #[arg(long, default_value_t = 30)]
        ttl: u16,
    },
    /// 接続されているシリアル port を列挙する
    ListPorts,
    /// 疎通確認を送り、応答を待つ
    Ping {
        /// port 名。省略時は VID:PID から自動検出する
        #[arg(long)]
        port: Option<String>,
    },
    /// 指定した領域へテキストを表示する（非 ASCII 文字は ? として表示）
    Text {
        #[arg(long)]
        port: Option<String>,
        #[arg(long, value_enum, default_value = "top")]
        slot: TextSlot,
        /// 表示秒数。0 は Clear または上書きまで保持する
        #[arg(long, default_value_t = 0)]
        ttl: u16,
        /// UTF-8 で最大 512 byte。改行と領域幅で折り返す
        text: String,
    },
    /// すべての表示領域を消し、顔のみの表示へ戻す
    Clear {
        #[arg(long)]
        port: Option<String>,
    },
    /// テキスト、比率バー、画像、余白を 4 行 × 各 2 要素以内で表示する
    Card {
        #[arg(long)]
        port: Option<String>,
        #[arg(long, value_enum, default_value = "overlay")]
        slot: TextSlot,
        #[arg(long, default_value_t = 0)]
        ttl: u16,
        #[arg(long)]
        title: String,
        #[arg(long)]
        detail: Option<String>,
        #[arg(long)]
        ratio: Option<u8>,
        #[arg(long, default_value = "Usage")]
        label: String,
        #[arg(long)]
        space: Option<u8>,
        /// 24-bit BMP から最大 16x16 px を切り出して表示する
        #[arg(long)]
        image_bmp: Option<PathBuf>,
        #[arg(long, requires = "image_bmp")]
        image_x: Option<u32>,
        #[arg(long, requires = "image_bmp")]
        image_y: Option<u32>,
        #[arg(long, requires = "image_bmp")]
        image_width: Option<u8>,
        #[arg(long, requires = "image_bmp")]
        image_height: Option<u8>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum TextSlot {
    Top,
    Bottom,
    Overlay,
}

#[derive(Clone, Copy, ValueEnum)]
enum FaceExpression {
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

impl From<FaceExpression> for protocol::Expression {
    fn from(value: FaceExpression) -> Self {
        match value {
            FaceExpression::Happy => Self::Happy,
            FaceExpression::Focused => Self::Focused,
            FaceExpression::Sleepy => Self::Sleepy,
            FaceExpression::Worried => Self::Worried,
            FaceExpression::Surprised => Self::Surprised,
            FaceExpression::Grin => Self::Grin,
            FaceExpression::Calm => Self::Calm,
            FaceExpression::Curious => Self::Curious,
            FaceExpression::Playful => Self::Playful,
            FaceExpression::Wink => Self::Wink,
            FaceExpression::Sad => Self::Sad,
            FaceExpression::Determined => Self::Determined,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum FaceGaze {
    Center,
    Left,
    Right,
    Up,
    Down,
}

impl From<FaceGaze> for protocol::Gaze {
    fn from(value: FaceGaze) -> Self {
        match value {
            FaceGaze::Center => Self::Center,
            FaceGaze::Left => Self::Left,
            FaceGaze::Right => Self::Right,
            FaceGaze::Up => Self::Up,
            FaceGaze::Down => Self::Down,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum HostEyeStyle {
    Auto,
    Open,
    Wide,
    Closed,
    HalfLidded,
}

impl From<HostEyeStyle> for protocol::EyeStyle {
    fn from(value: HostEyeStyle) -> Self {
        match value {
            HostEyeStyle::Auto => Self::Auto,
            HostEyeStyle::Open => Self::Open,
            HostEyeStyle::Wide => Self::Wide,
            HostEyeStyle::Closed => Self::Closed,
            HostEyeStyle::HalfLidded => Self::HalfLidded,
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum HostActivity {
    Idle,
    Working,
    Waiting,
    Done,
    Error,
}

impl From<HostActivity> for protocol::Activity {
    fn from(value: HostActivity) -> Self {
        match value {
            HostActivity::Idle => Self::Idle,
            HostActivity::Working => Self::Working,
            HostActivity::Waiting => Self::Waiting,
            HostActivity::Done => Self::Done,
            HostActivity::Error => Self::Error,
        }
    }
}

impl From<TextSlot> for protocol::Slot {
    fn from(slot: TextSlot) -> Self {
        match slot {
            TextSlot::Top => Self::BannerTop,
            TextSlot::Bottom => Self::BannerBottom,
            TextSlot::Overlay => Self::Overlay,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Face {
            port,
            expression,
            gaze,
            eyes,
            ttl,
        } => send_display(
            port,
            &presence_message(None, "", expression.into(), gaze.into(), eyes.into(), ttl)?,
        ),
        Command::Status {
            port,
            activity,
            detail,
            gaze,
            eyes,
            ttl,
        } => {
            let activity: protocol::Activity = activity.into();
            let expression = match activity {
                protocol::Activity::Idle | protocol::Activity::Done => protocol::Expression::Happy,
                protocol::Activity::Working => protocol::Expression::Focused,
                protocol::Activity::Waiting => protocol::Expression::Sleepy,
                protocol::Activity::Error => protocol::Expression::Worried,
            };
            send_display(
                port,
                &presence_message(
                    Some(activity),
                    &detail,
                    expression,
                    gaze.into(),
                    eyes.into(),
                    ttl,
                )?,
            )
        }
        Command::ListPorts => list_ports(),
        Command::Ping { port } => ping(port),
        Command::Text {
            port,
            slot,
            ttl,
            text,
        } => send_display(port, &text_message(slot, ttl, &text)?),
        Command::Clear { port } => send_display(port, &protocol::Message::Clear),
        Command::Card {
            port,
            slot,
            ttl,
            title,
            detail,
            ratio,
            label,
            space,
            image_bmp,
            image_x,
            image_y,
            image_width,
            image_height,
        } => send_display(
            port,
            &card_message(
                slot,
                ttl,
                CardContent {
                    title: &title,
                    detail: detail.as_deref(),
                    ratio,
                    label: &label,
                    space,
                    image: image_bmp
                        .as_deref()
                        .map(|path| {
                            bmp::load_icon(
                                path,
                                image_x.unwrap_or(0),
                                image_y.unwrap_or(0),
                                image_width.unwrap_or(protocol::MAX_IMAGE_SIDE),
                                image_height.unwrap_or(protocol::MAX_IMAGE_SIDE),
                            )
                        })
                        .transpose()?,
                },
            )?,
        ),
    }
}

fn list_ports() -> Result<(), Box<dyn std::error::Error>> {
    for p in serialport::available_ports()? {
        match p.port_type {
            serialport::SerialPortType::UsbPort(info) => {
                println!(
                    "{}\tusb {:04x}:{:04x}\t{}",
                    p.port_name,
                    info.vid,
                    info.pid,
                    info.product.unwrap_or_default()
                );
            }
            other => println!("{}\t{:?}", p.port_name, other),
        }
    }
    Ok(())
}

fn detect_port() -> Result<String, Box<dyn std::error::Error>> {
    let found = serialport::available_ports()?.into_iter().find(|p| {
        matches!(
            &p.port_type,
            serialport::SerialPortType::UsbPort(info)
                if (info.vid, info.pid) == ESP_USB_SERIAL_JTAG
        )
    });
    found
        .map(|p| p.port_name)
        .ok_or_else(|| "USB Serial/JTAG (303a:1001) の port が見つかりません".into())
}

fn open_port(
    port: Option<String>,
) -> Result<(String, Box<dyn serialport::SerialPort>), Box<dyn std::error::Error>> {
    let port_name = match port {
        Some(p) => p,
        None => detect_port()?,
    };
    let port = serialport::new(&port_name, 115_200)
        .timeout(Duration::from_millis(1000))
        .open()?;
    // 書き込み直後に残る bootloader のログを今回の応答に混入させない。
    port.clear(serialport::ClearBuffer::Input)?;
    Ok((port_name, port))
}

fn ping(port: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let (port_name, mut port) = open_port(port)?;
    let nonce = 0x5A5A_1234;
    let reply = exchange(&mut port, &protocol::Message::Ping { nonce })?;
    println!("{port_name}: {reply:?}");
    validate_pong(&reply, nonce).map_err(Into::into)
}

fn text_message(slot: TextSlot, ttl_s: u16, text: &str) -> Result<protocol::Message, String> {
    Ok(protocol::Message::Text {
        slot: slot.into(),
        ttl_s,
        text: text.try_into().map_err(|_| {
            format!(
                "テキストは UTF-8 で {} byte 以下にしてください",
                protocol::MAX_TEXT_BYTES
            )
        })?,
    })
}

fn presence_message(
    activity: Option<protocol::Activity>,
    detail: &str,
    expression: protocol::Expression,
    gaze: protocol::Gaze,
    eyes: protocol::EyeStyle,
    ttl_s: u16,
) -> Result<protocol::Message, String> {
    let detail = detail.try_into().map_err(|_| {
        format!(
            "状態の詳細は UTF-8 で {} byte 以下にしてください",
            protocol::MAX_STATUS_DETAIL_BYTES
        )
    })?;
    Ok(protocol::Message::Presence(protocol::Presence {
        activity,
        detail,
        expression,
        gaze,
        eyes,
        ttl_s,
    }))
}

struct CardContent<'a> {
    title: &'a str,
    detail: Option<&'a str>,
    ratio: Option<u8>,
    label: &'a str,
    space: Option<u8>,
    image: Option<protocol::ImageData>,
}

#[cfg(test)]
impl<'a> CardContent<'a> {
    fn new(title: &'a str) -> Self {
        Self {
            title,
            detail: None,
            ratio: None,
            label: "Usage",
            space: None,
            image: None,
        }
    }
}

fn card_message(
    slot: TextSlot,
    ttl_s: u16,
    content: CardContent<'_>,
) -> Result<protocol::Message, String> {
    let CardContent {
        title,
        detail,
        ratio,
        label,
        space,
        image,
    } = content;
    let mut card = protocol::Card {
        slot: slot.into(),
        ttl_s,
        rows: Default::default(),
        image,
    };
    let mut header = protocol::Row {
        elements: Default::default(),
    };
    header
        .elements
        .push(protocol::Element::Text {
            text: title.try_into().map_err(|_| {
                format!(
                    "title は UTF-8 で {} byte 以下にしてください",
                    protocol::MAX_CARD_TEXT_BYTES
                )
            })?,
        })
        .map_err(|_| "Card の要素が多すぎます")?;
    if let Some(detail) = detail {
        header
            .elements
            .push(protocol::Element::Text {
                text: detail.try_into().map_err(|_| {
                    format!(
                        "detail は UTF-8 で {} byte 以下にしてください",
                        protocol::MAX_CARD_TEXT_BYTES
                    )
                })?,
            })
            .map_err(|_| "Card の要素が多すぎます")?;
    }
    card.rows
        .push(header)
        .map_err(|_| "Card の行が多すぎます")?;
    if let Some(ratio) = ratio {
        let mut row = protocol::Row {
            elements: Default::default(),
        };
        row.elements
            .push(protocol::Element::Bar {
                ratio,
                label: label.try_into().map_err(|_| {
                    format!(
                        "label は UTF-8 で {} byte 以下にしてください",
                        protocol::MAX_BAR_LABEL_BYTES
                    )
                })?,
            })
            .map_err(|_| "Card の要素が多すぎます")?;
        card.rows.push(row).map_err(|_| "Card の行が多すぎます")?;
    }
    if card.image.is_some() {
        let mut row = protocol::Row {
            elements: Default::default(),
        };
        row.elements
            .push(protocol::Element::Image)
            .map_err(|_| "Card の要素が多すぎます")?;
        card.rows.push(row).map_err(|_| "Card の行が多すぎます")?;
    }
    if let Some(height) = space {
        let mut row = protocol::Row {
            elements: Default::default(),
        };
        row.elements
            .push(protocol::Element::Spacer { height })
            .map_err(|_| "Card の要素が多すぎます")?;
        card.rows.push(row).map_err(|_| "Card の行が多すぎます")?;
    }
    card.validate().map_err(str::to_owned)?;
    Ok(protocol::Message::Card(card))
}

fn exchange(
    port: &mut (impl Read + Write),
    message: &protocol::Message,
) -> Result<protocol::Reply, Box<dyn std::error::Error>> {
    let mut buffer = [0; protocol::MAX_FRAME_BYTES];
    let frame = protocol::encode(message, &mut buffer)?;
    port.write_all(frame)?;
    port.flush()?;
    read_reply(port)
}

fn send_checked(
    port: &mut (impl Read + Write),
    message: &protocol::Message,
) -> Result<u32, Box<dyn std::error::Error>> {
    let nonce = 0x5A5A_1234;
    let reply = exchange(port, &protocol::Message::Ping { nonce })?;
    validate_pong(&reply, nonce)?;
    match exchange(port, message)? {
        protocol::Reply::Ack { seq } => Ok(seq),
        protocol::Reply::Rejected { count } => {
            Err(format!("表示更新が拒否されました: rejected={count}").into())
        }
        protocol::Reply::Pong { .. } => Err("表示更新に対して Ack 以外の応答を受信しました".into()),
    }
}

fn send_display(
    port: Option<String>,
    message: &protocol::Message,
) -> Result<(), Box<dyn std::error::Error>> {
    let (port_name, mut port) = open_port(port)?;
    let seq = send_checked(&mut port, message)?;
    println!("{port_name}: Ack {{ seq: {seq} }}");
    Ok(())
}

fn read_reply(reader: &mut impl Read) -> Result<protocol::Reply, Box<dyn std::error::Error>> {
    let mut rx = [0u8; protocol::MAX_FRAME_BYTES];
    let mut len = 0;
    let mut discarding = false;
    let deadline = Instant::now() + Duration::from_secs(1);
    // 起動ログや空の区切りを読み飛ばしても、連続入力で永久に待たない。
    while Instant::now() < deadline {
        let mut byte = [0];
        let n = reader.read(&mut byte)?;
        if n == 0 {
            return Err("応答がありません".into());
        }
        if byte[0] == 0 {
            if !discarding && len != 0 {
                rx[len] = 0;
                if let Ok(reply) = protocol::decode(&mut rx[..=len]) {
                    return Ok(reply);
                }
            }
            len = 0;
            discarding = false;
        } else if !discarding {
            if len == rx.len() - 1 {
                discarding = true;
            } else {
                rx[len] = byte[0];
                len += 1;
            }
        }
    }
    Err("応答待ちがタイムアウトしました".into())
}

fn validate_pong(reply: &protocol::Reply, expected_nonce: u32) -> Result<(), String> {
    match reply {
        protocol::Reply::Pong { nonce, .. } if *nonce != expected_nonce => {
            Err("nonce が一致しません".into())
        }
        protocol::Reply::Pong { version, .. } if *version != protocol::VERSION => Err(format!(
            "プロトコル版が一致しません: host={}, firmware={version}",
            protocol::VERSION
        )),
        protocol::Reply::Pong { .. } => Ok(()),
        _ => Err("Ping に対して Pong 以外の応答を受信しました".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: u32 = 0x5A5A_1234;

    struct Link {
        input: std::io::Cursor<Vec<u8>>,
        output: Vec<u8>,
    }
    impl Link {
        fn new(replies: &[protocol::Reply]) -> Self {
            let mut bytes = Vec::new();
            for reply in replies {
                let mut buffer = [0; 32];
                bytes.extend(protocol::encode(reply, &mut buffer).unwrap());
            }
            Self {
                input: std::io::Cursor::new(bytes),
                output: Vec::new(),
            }
        }
        fn messages(&self) -> Vec<protocol::Message> {
            self.output
                .split_inclusive(|&byte| byte == 0)
                .map(|frame| protocol::decode(&mut frame.to_vec()).unwrap())
                .collect()
        }
    }
    impl Read for Link {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buffer)
        }
    }
    impl Write for Link {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn display_command_is_not_sent_when_handshake_fails() {
        for reply in [
            protocol::Reply::Pong {
                nonce: NONCE,
                version: protocol::VERSION.wrapping_add(1),
            },
            protocol::Reply::Pong {
                nonce: NONCE + 1,
                version: protocol::VERSION,
            },
            protocol::Reply::Rejected { count: 1 },
        ] {
            let mut link = Link::new(&[reply]);
            assert!(send_checked(&mut link, &protocol::Message::Clear).is_err());
            assert_eq!(link.messages(), [protocol::Message::Ping { nonce: NONCE }]);
        }
    }

    #[test]
    fn text_requires_ack_after_successful_handshake() {
        let message = text_message(TextSlot::Bottom, 7, "Hello").unwrap();
        let pong = protocol::Reply::Pong {
            nonce: NONCE,
            version: protocol::VERSION,
        };
        let mut link = Link::new(&[pong, protocol::Reply::Ack { seq: 42 }]);
        assert_eq!(send_checked(&mut link, &message).unwrap(), 42);
        assert_eq!(
            link.messages(),
            [protocol::Message::Ping { nonce: NONCE }, message.clone()]
        );
        for reply in [protocol::Reply::Rejected { count: 1 }, pong] {
            assert!(send_checked(&mut Link::new(&[pong, reply]), &message).is_err());
        }
    }

    #[test]
    fn text_limit_counts_utf8_bytes_and_cli_validates_ttl_and_slot() {
        assert!(text_message(TextSlot::Top, 0, &"a".repeat(512)).is_ok());
        assert!(text_message(TextSlot::Top, 0, &"a".repeat(513)).is_err());
        assert!(text_message(TextSlot::Overlay, 1, &"日".repeat(171)).is_err());
        assert!(Cli::try_parse_from(["stackchan", "text", "--ttl", "65536", "test"]).is_err());
        assert!(Cli::try_parse_from(["stackchan", "text", "--slot", "unknown", "test"]).is_err());
    }

    #[test]
    fn presence_cli_encodes_activity_and_checks_detail_bytes() {
        let cli = Cli::try_parse_from([
            "stackchan",
            "status",
            "--activity",
            "working",
            "--detail",
            "BUILD",
            "--gaze",
            "left",
            "--eyes",
            "half-lidded",
            "--ttl",
            "5",
        ])
        .unwrap();
        let Command::Status {
            activity,
            detail,
            gaze,
            eyes,
            ttl,
            ..
        } = cli.command
        else {
            panic!("status expected")
        };
        assert_eq!(ttl, 5);
        assert!(matches!(activity, HostActivity::Working));
        let message = presence_message(
            Some(activity.into()),
            &detail,
            protocol::Expression::Focused,
            gaze.into(),
            eyes.into(),
            ttl,
        )
        .unwrap();
        let protocol::Message::Presence(presence) = message else {
            panic!("presence expected")
        };
        assert_eq!(presence.activity, Some(protocol::Activity::Working));
        assert_eq!(presence.gaze, protocol::Gaze::Left);
        assert_eq!(presence.eyes, protocol::EyeStyle::HalfLidded);
        assert_eq!(presence.detail.as_str(), "BUILD");
        assert!(
            presence_message(
                None,
                &"日".repeat(7),
                protocol::Expression::Happy,
                protocol::Gaze::Center,
                protocol::EyeStyle::Auto,
                0
            )
            .is_err()
        );
        assert!(Cli::try_parse_from(["stackchan", "status", "--activity", "unknown"]).is_err());
    }

    #[test]
    fn card_builder_checks_layout_and_old_firmware_version_before_sending() {
        let message = card_message(
            TextSlot::Top,
            2,
            CardContent {
                detail: Some("75%"),
                ratio: Some(75),
                ..CardContent::new("CPU")
            },
        )
        .unwrap();
        let protocol::Message::Card(card) = &message else {
            panic!("Card を生成できませんでした");
        };
        assert_eq!(card.rows.len(), 2);
        assert_eq!(card.validate(), Ok(()));
        assert!(
            card_message(
                TextSlot::Top,
                0,
                CardContent {
                    ratio: Some(101),
                    ..CardContent::new("CPU")
                }
            )
            .is_err()
        );
        assert!(
            card_message(
                TextSlot::Top,
                0,
                CardContent {
                    ratio: Some(50),
                    space: Some(10),
                    ..CardContent::new("CPU")
                }
            )
            .is_err()
        );
        assert!(card_message(TextSlot::Overlay, 0, CardContent::new(&"日".repeat(17))).is_err());
        let mut link = Link::new(&[protocol::Reply::Pong {
            nonce: NONCE,
            version: protocol::VERSION - 1,
        }]);
        assert!(send_checked(&mut link, &message).is_err());
        assert_eq!(link.messages(), [protocol::Message::Ping { nonce: NONCE }]);
    }

    #[test]
    fn accepts_matching_nonce_and_version() {
        let reply = protocol::Reply::Pong {
            nonce: NONCE,
            version: protocol::VERSION,
        };
        assert_eq!(validate_pong(&reply, NONCE), Ok(()));
    }

    #[test]
    fn rejects_incompatible_version_even_with_matching_nonce() {
        let version = protocol::VERSION.wrapping_add(1);
        let reply = protocol::Reply::Pong {
            nonce: NONCE,
            version,
        };
        assert_eq!(
            validate_pong(&reply, NONCE),
            Err(format!(
                "プロトコル版が一致しません: host={}, firmware={version}",
                protocol::VERSION
            ))
        );
    }

    #[test]
    fn rejects_wrong_nonce_even_with_matching_version() {
        let reply = protocol::Reply::Pong {
            nonce: NONCE + 1,
            version: protocol::VERSION,
        };
        assert_eq!(
            validate_pong(&reply, NONCE),
            Err("nonce が一致しません".into())
        );
    }

    #[test]
    fn rejects_non_pong_replies() {
        for reply in [
            protocol::Reply::Ack { seq: 0 },
            protocol::Reply::Rejected { count: 0 },
        ] {
            assert_eq!(
                validate_pong(&reply, NONCE),
                Err("Ping に対して Pong 以外の応答を受信しました".into())
            );
        }
    }

    #[test]
    fn reply_reader_resynchronizes_after_boot_log_and_oversized_frame() {
        let reply = protocol::Reply::Pong {
            nonce: NONCE,
            version: protocol::VERSION,
        };
        let mut buffer = [0; 32];
        let frame = protocol::encode(&reply, &mut buffer).unwrap();
        let mut stream = b"boot: Loaded app\r\n\0\0".to_vec();
        stream.extend([1; protocol::MAX_FRAME_BYTES]);
        // 上限超過フレームの末尾にある正しい応答も、区切りまでは捨てる。
        stream.extend(frame);
        stream.extend(frame);
        assert_eq!(read_reply(&mut stream.as_slice()).unwrap(), reply);
    }

    #[test]
    fn reply_reader_rejects_truncated_input() {
        let mut stream = &[1, 6, 1, 2][..];
        assert!(read_reply(&mut stream).is_err());
    }

    #[test]
    #[ignore = "Ping 対応 firmware の実機と STACKCHAN_TEST_PORT が必要"]
    fn hardware_ping_and_frame_recovery() {
        let port_name = std::env::var("STACKCHAN_TEST_PORT").expect("STACKCHAN_TEST_PORT が必要");
        let mut port = serialport::new(port_name, 115_200)
            .timeout(Duration::from_secs(2))
            .open()
            .unwrap();
        port.clear(serialport::ClearBuffer::Input).unwrap();
        let mut buffer = [0; 32];
        let ping =
            protocol::encode(&protocol::Message::Ping { nonce: NONCE }, &mut buffer).unwrap();

        // USB パケットが分割されても終端まで蓄積できることを実機で確認する。
        for byte in ping {
            port.write_all(&[*byte]).unwrap();
            port.flush().unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            validate_pong(&read_reply(&mut port).unwrap(), NONCE),
            Ok(())
        );

        port.write_all(&[0xFF, 1, 2, 0]).unwrap();
        port.flush().unwrap();
        let protocol::Reply::Rejected { count } = read_reply(&mut port).unwrap() else {
            panic!("不正フレームが拒否されませんでした");
        };
        assert!(count > 0);

        // 上限超過フレームの末尾が正しい Ping でも、その区切りまでは破棄する。
        port.write_all(&[1; protocol::MAX_FRAME_BYTES * 2]).unwrap();
        port.write_all(ping).unwrap();
        port.flush().unwrap();
        assert_eq!(
            read_reply(&mut port).unwrap(),
            protocol::Reply::Rejected {
                count: count.saturating_add(1)
            }
        );

        // 連結されたフレームを順番どおり返し、拒否後にも正常に復帰する。
        port.write_all(ping).unwrap();
        port.write_all(ping).unwrap();
        port.flush().unwrap();
        for _ in 0..2 {
            assert_eq!(
                validate_pong(&read_reply(&mut port).unwrap(), NONCE),
                Ok(())
            );
        }
    }

    #[test]
    #[ignore = "Text/Clear 対応 firmware の実機と STACKCHAN_TEST_PORT が必要"]
    fn hardware_text_and_clear() {
        let port_name = std::env::var("STACKCHAN_TEST_PORT").expect("STACKCHAN_TEST_PORT が必要");
        let (_, mut port) = open_port(Some(port_name)).unwrap();
        let mut seq = send_checked(&mut port, &protocol::Message::Clear).unwrap();
        for (slot, text, ttl) in [
            (TextSlot::Top, "USB Text OK".to_string(), 0),
            (TextSlot::Bottom, "TTL test".to_string(), 1),
            (TextSlot::Overlay, "W".repeat(protocol::MAX_TEXT_BYTES), 1),
        ] {
            let next = send_checked(&mut port, &text_message(slot, ttl, &text).unwrap()).unwrap();
            assert_eq!(next, seq.wrapping_add(1));
            seq = next;
        }
        // TTL 処理中も通信が継続し、自発的な Ack を送らないことを確認する。
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(
            send_checked(&mut port, &protocol::Message::Clear).unwrap(),
            seq.wrapping_add(1)
        );
        assert_eq!(
            validate_pong(
                &exchange(&mut port, &protocol::Message::Ping { nonce: NONCE }).unwrap(),
                NONCE
            ),
            Ok(())
        );
    }

    #[test]
    #[ignore = "Card 対応 firmware の実機と STACKCHAN_TEST_PORT が必要"]
    fn hardware_card_and_invalid_ratio() {
        let port_name = std::env::var("STACKCHAN_TEST_PORT").expect("STACKCHAN_TEST_PORT が必要");
        let (_, mut port) = open_port(Some(port_name)).unwrap();
        let mut seq = send_checked(&mut port, &protocol::Message::Clear).unwrap();
        let banner = card_message(
            TextSlot::Top,
            0,
            CardContent {
                detail: Some("75%"),
                ratio: Some(75),
                ..CardContent::new("CPU")
            },
        )
        .unwrap();
        let next = send_checked(&mut port, &banner).unwrap();
        assert_eq!(next, seq.wrapping_add(1));
        seq = next;

        // 型として復号できるが、比率が範囲外の Card は Ack せず拒否する。
        let mut invalid = banner.clone();
        let protocol::Message::Card(card) = &mut invalid else {
            panic!("Card が必要です");
        };
        card.rows[1].elements[0] = protocol::Element::Bar {
            ratio: 101,
            label: "bad".try_into().unwrap(),
        };
        assert!(matches!(
            exchange(&mut port, &invalid).unwrap(),
            protocol::Reply::Rejected { .. }
        ));

        let overlay = card_message(
            TextSlot::Overlay,
            1,
            CardContent {
                detail: Some("Overlay"),
                ratio: Some(50),
                space: Some(8),
                ..CardContent::new("Card OK")
            },
        )
        .unwrap();
        let next = send_checked(&mut port, &overlay).unwrap();
        assert_eq!(next, seq.wrapping_add(1));
        seq = next;
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(
            send_checked(&mut port, &protocol::Message::Clear).unwrap(),
            seq.wrapping_add(1)
        );
        assert_eq!(
            validate_pong(
                &exchange(&mut port, &protocol::Message::Ping { nonce: NONCE }).unwrap(),
                NONCE
            ),
            Ok(())
        );
    }

    #[test]
    #[ignore = "画像対応 firmware の実機と STACKCHAN_TEST_PORT が必要"]
    fn hardware_inline_image_and_invalid_length() {
        let port_name = std::env::var("STACKCHAN_TEST_PORT").expect("STACKCHAN_TEST_PORT が必要");
        let (_, mut port) = open_port(Some(port_name)).unwrap();
        let mut pixels = protocol::ImageData {
            width: 16,
            height: 16,
            pixels: Default::default(),
        };
        for _ in 0..16 {
            for column in 0..16 {
                let color: u16 = [0xF800, 0x07E0, 0x001F, 0xFFFF][column / 4];
                pixels
                    .pixels
                    .extend_from_slice(&color.to_be_bytes())
                    .unwrap();
            }
        }
        let image = card_message(
            TextSlot::Top,
            0,
            CardContent {
                image: Some(pixels),
                ..CardContent::new("RGB565")
            },
        )
        .unwrap();
        let seq = send_checked(&mut port, &protocol::Message::Clear).unwrap();
        assert_eq!(
            send_checked(&mut port, &image).unwrap(),
            seq.wrapping_add(1)
        );

        let mut invalid = image.clone();
        let protocol::Message::Card(card) = &mut invalid else {
            panic!("Card が必要です");
        };
        card.image.as_mut().unwrap().pixels.pop();
        assert!(matches!(
            exchange(&mut port, &invalid).unwrap(),
            protocol::Reply::Rejected { .. }
        ));

        // 画像 512 byte と最大長のテキスト 7 個を同時に送る。
        let mut maximal = image;
        let protocol::Message::Card(card) = &mut maximal else {
            panic!("Card が必要です");
        };
        card.slot = protocol::Slot::Overlay;
        card.rows.clear();
        for row_index in 0..protocol::MAX_CARD_ROWS {
            let mut row = protocol::Row {
                elements: Default::default(),
            };
            for column_index in 0..protocol::MAX_ROW_ELEMENTS {
                row.elements
                    .push(if row_index == 0 && column_index == 0 {
                        protocol::Element::Image
                    } else {
                        protocol::Element::Text {
                            text: "x"
                                .repeat(protocol::MAX_CARD_TEXT_BYTES)
                                .as_str()
                                .try_into()
                                .unwrap(),
                        }
                    })
                    .unwrap();
            }
            card.rows.push(row).unwrap();
        }
        assert_eq!(card.validate(), Ok(()));
        assert_eq!(
            send_checked(&mut port, &maximal).unwrap(),
            seq.wrapping_add(2)
        );
        assert_eq!(
            send_checked(&mut port, &protocol::Message::Clear).unwrap(),
            seq.wrapping_add(3)
        );
    }

    #[test]
    #[ignore = "Presence 対応 firmware の実機と STACKCHAN_TEST_PORT が必要"]
    fn hardware_presence_and_ttl() {
        let port_name = std::env::var("STACKCHAN_TEST_PORT").expect("STACKCHAN_TEST_PORT が必要");
        let (_, mut port) = open_port(Some(port_name)).unwrap();
        let mut seq = send_checked(&mut port, &protocol::Message::Clear).unwrap();
        for message in [
            presence_message(
                None,
                "",
                protocol::Expression::Surprised,
                protocol::Gaze::Left,
                protocol::EyeStyle::Wide,
                0,
            )
            .unwrap(),
            presence_message(
                Some(protocol::Activity::Working),
                "BUILD",
                protocol::Expression::Focused,
                protocol::Gaze::Right,
                protocol::EyeStyle::HalfLidded,
                1,
            )
            .unwrap(),
        ] {
            let next = send_checked(&mut port, &message).unwrap();
            assert_eq!(next, seq.wrapping_add(1));
            seq = next;
        }
        std::thread::sleep(Duration::from_millis(1200));
        assert_eq!(
            validate_pong(
                &exchange(&mut port, &protocol::Message::Ping { nonce: NONCE }).unwrap(),
                NONCE
            ),
            Ok(())
        );
        assert_eq!(
            send_checked(&mut port, &protocol::Message::Clear).unwrap(),
            seq.wrapping_add(1)
        );
    }
}
