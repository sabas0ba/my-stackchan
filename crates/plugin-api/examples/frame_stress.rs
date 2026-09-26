//! 固定 seed から plugin フレームの列を変異させ、FrameReader と検証の panic を検出する。
//!
//! daemon は信頼しない plugin の出力を読むため、任意のバイト列に対して panic せず、
//! 確保量がフレーム長の上限で抑えられることを確認する。

use plugin_api::{
    API_VERSION, Capabilities, CardPut, Element, Emote, Expression, EyeStyle, FrameError,
    FrameReader, Gaze, Hello, HostMessage, Init, Limits, LogLevel, MAX_CARD_ROWS, MAX_FRAME_BYTES,
    MAX_ROW_ELEMENTS, MAX_TEXT_BYTES, Notify, Placement, PluginMessage, Presence, Priority, Row,
    write_frame,
};
use serde::Serialize;

const DEFAULT_CASES: usize = 5_000;
const DEFAULT_SEED: u64 = 0x2F6B_91D4_0C3E_A751;
const MAX_INPUT_BYTES: usize = MAX_FRAME_BYTES * 2 + 64;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn range(&mut self, upper: usize) -> usize {
        (self.next() as usize) % upper
    }
}

fn wire(value: &impl Serialize) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, value).unwrap();
    bytes
}

fn largest_card() -> PluginMessage {
    let text = "x".repeat(MAX_TEXT_BYTES);
    PluginMessage::CardPut(CardPut {
        card: 7,
        placement: Placement::Overlay,
        priority: Priority::High,
        ttl_s: u16::MAX,
        rows: vec![
            Row {
                elements: vec![Element::Text { text: text.clone() }; MAX_ROW_ELEMENTS],
                action: Some(u8::MAX),
            };
            MAX_CARD_ROWS
        ],
    })
}

fn corpus() -> Vec<Vec<u8>> {
    let hello = PluginMessage::Hello(Hello {
        api_version: API_VERSION,
        name: "clock".into(),
        version: "0.1.0".into(),
        capabilities: Capabilities {
            cards: 2,
            notify: true,
            presence: true,
            motion: false,
        },
    });
    let init = HostMessage::Init(Init {
        api_version: API_VERSION,
        granted: Capabilities::default(),
        params: vec![("key".into(), "value".into())],
        limits: Limits {
            banner_rows: 2,
            overlay_rows: 4,
            row_elements: 2,
            text_bytes: 48,
            bar_label_bytes: 12,
            notify_bytes: 512,
            presence_detail_bytes: 20,
        },
    });
    let mut frames = vec![
        wire(&hello),
        wire(&largest_card()),
        wire(&PluginMessage::CardPut(CardPut {
            card: 0,
            placement: Placement::Banner,
            priority: Priority::Low,
            ttl_s: 0,
            rows: vec![Row {
                elements: vec![
                    Element::Bar {
                        ratio: 100,
                        label: "CPU".into(),
                    },
                    Element::Spacer { height: 32 },
                ],
                action: None,
            }],
        })),
        wire(&PluginMessage::CardRemove { card: 0 }),
        wire(&PluginMessage::Notify(Notify {
            text: "done".into(),
            priority: Priority::High,
            ttl_s: 10,
        })),
        wire(&PluginMessage::Presence(Presence {
            activity: None,
            detail: String::new(),
            expression: Expression::Calm,
            gaze: Gaze::Point { x: 100, y: -100 },
            eyes: EyeStyle::Wide,
            ttl_s: 30,
        })),
        wire(&PluginMessage::Emote(Emote {
            expression: Expression::Wink,
            gaze: Gaze::Left,
            eyes: EyeStyle::Auto,
            intensity: 100,
            duration_ms: 10_000,
        })),
        wire(&PluginMessage::Log {
            level: LogLevel::Debug,
            text: "x".repeat(1024),
        }),
        wire(&init),
        wire(&HostMessage::Action { card: 1, action: 2 }),
        wire(&HostMessage::Shutdown),
        vec![],
        vec![0],
        vec![0xFF, 0],
    ];
    frames.push(vec![0xFF; MAX_FRAME_BYTES - 1]);
    frames.push(vec![0xFF; MAX_FRAME_BYTES + 1]);
    frames
}

fn mutate(input: &mut Vec<u8>, rng: &mut Rng) {
    match rng.range(6) {
        0 if !input.is_empty() => {
            let index = rng.range(input.len());
            input[index] ^= 1 << rng.range(8);
        }
        1 if !input.is_empty() => input.truncate(rng.range(input.len())),
        2 if input.len() < MAX_INPUT_BYTES => {
            let index = rng.range(input.len() + 1);
            input.insert(index, rng.next() as u8);
        }
        3 if !input.is_empty() => {
            let index = rng.range(input.len());
            input[index] = 0;
        }
        4 if !input.is_empty() => {
            let index = rng.range(input.len());
            input[index] = 0xFF;
        }
        _ if input.len() < MAX_INPUT_BYTES => input.push(rng.next() as u8),
        _ => {}
    }
}

fn arbitrary_input(rng: &mut Rng) -> Vec<u8> {
    let boundaries = [
        0,
        1,
        254,
        255,
        MAX_FRAME_BYTES - 1,
        MAX_FRAME_BYTES,
        MAX_FRAME_BYTES + 1,
    ];
    let len = if rng.range(2) == 0 {
        boundaries[rng.range(boundaries.len())]
    } else {
        rng.range(MAX_FRAME_BYTES + 1)
    };
    (0..len).map(|_| rng.next() as u8).collect()
}

/// 入力を 1 本のストリームとして読み、全フレームを復号・検証する。
fn exercise(input: &[u8]) {
    let mut plugin_side = FrameReader::new(input);
    // 読取りは毎回少なくとも 1 byte 進むため、入力長 + 1 回で必ず終わる。
    for _ in 0..=input.len() {
        match plugin_side.read_frame::<PluginMessage>() {
            Ok(Some(message)) => {
                let _ = message.validate();
            }
            Ok(None) => break,
            Err(FrameError::Io(error)) => panic!("unexpected I/O error: {error}"),
            Err(_) => {}
        }
    }
    let mut host_side = FrameReader::new(input);
    for _ in 0..=input.len() {
        match host_side.read_frame::<HostMessage>() {
            Ok(Some(message)) => {
                let _ = message.validate();
            }
            Ok(None) => break,
            Err(_) => {}
        }
    }
}

fn checked_exercise(input: &[u8], seed: u64, index: usize) {
    let result = std::panic::catch_unwind(|| exercise(input));
    if let Err(payload) = result {
        eprintln!(
            "frame reader panic: seed={seed:#018x}, case={index}, len={}",
            input.len()
        );
        std::panic::resume_unwind(payload);
    }
}

fn parse_number(value: &str) -> Result<u64, String> {
    if let Some(hex) = value.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).map_err(|_| format!("invalid number: {value}"))
    } else {
        value
            .parse()
            .map_err(|_| format!("invalid number: {value}"))
    }
}

fn run() -> Result<(), String> {
    let mut cases = DEFAULT_CASES;
    let mut seed = DEFAULT_SEED;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--cases" => {
                cases = usize::try_from(parse_number(&value)?)
                    .map_err(|_| "case count is too large".to_owned())?;
            }
            "--seed" => seed = parse_number(&value)?,
            _ => return Err(format!("unknown option: {flag}")),
        }
    }
    if cases == 0 || seed == 0 {
        return Err("--cases and --seed must be nonzero".into());
    }

    let corpus = corpus();
    for (index, input) in corpus.iter().enumerate() {
        checked_exercise(input, seed, index);
    }
    let mut rng = Rng(seed);
    for index in 0..cases {
        // 複数のフレームを連結し、フレーム境界をまたぐ変異も生じさせる。
        let mut input = Vec::new();
        for _ in 0..=rng.range(3) {
            if rng.range(3) == 0 {
                input.extend(arbitrary_input(&mut rng));
            } else {
                input.extend(&corpus[rng.range(corpus.len())]);
            }
        }
        for _ in 0..=rng.range(4) {
            mutate(&mut input, &mut rng);
        }
        checked_exercise(&input, seed, corpus.len() + index);
    }
    println!(
        "frame stress passed: {} inputs, seed {seed:#018x}",
        corpus.len() + cases
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
