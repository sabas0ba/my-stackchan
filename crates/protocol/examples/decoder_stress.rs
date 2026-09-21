//! 固定 seed から COBS フレームを変異させ、decoder の panic を検出する。

use protocol::{
    Activity, Card, Element, Expression, EyeStyle, Gaze, ImageData, MAX_CARD_ROWS,
    MAX_CARD_TEXT_BYTES, MAX_FRAME_BYTES, MAX_IMAGE_BYTES, MAX_ROW_ELEMENTS, Message, Presence,
    Reply, Row, Slot,
};
use serde::Serialize;

const DEFAULT_CASES: usize = 20_000;
const DEFAULT_SEED: u64 = 0x7C3A_4D92_B615_EF08;
const MAX_INPUT_BYTES: usize = MAX_FRAME_BYTES + 32;

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
    let mut buffer = [0; MAX_FRAME_BYTES];
    protocol::encode(value, &mut buffer).unwrap().to_vec()
}

fn largest_card() -> Message {
    let mut pixels = ImageData {
        width: 16,
        height: 16,
        pixels: heapless::Vec::new(),
    };
    for index in 0..MAX_IMAGE_BYTES {
        pixels.pixels.push((index & 0xFF) as u8).unwrap();
    }
    let mut text = heapless::String::<MAX_CARD_TEXT_BYTES>::new();
    for _ in 0..MAX_CARD_TEXT_BYTES {
        text.push('x').unwrap();
    }
    let mut card = Card {
        slot: Slot::Overlay,
        ttl_s: 65535,
        rows: heapless::Vec::new(),
        image: Some(pixels),
    };
    for row_index in 0..MAX_CARD_ROWS {
        let mut row = Row {
            elements: heapless::Vec::new(),
        };
        for column_index in 0..MAX_ROW_ELEMENTS {
            let element = if row_index == 0 && column_index == 0 {
                Element::Image
            } else {
                Element::Text { text: text.clone() }
            };
            row.elements.push(element).unwrap();
        }
        card.rows.push(row).unwrap();
    }
    card.validate().unwrap();
    Message::Card(card)
}

fn corpus() -> Vec<Vec<u8>> {
    let mut frames = vec![
        wire(&Message::Ping { nonce: u32::MAX }),
        wire(&Message::Clear),
        wire(&Message::Text {
            slot: Slot::Overlay,
            ttl_s: u16::MAX,
            text: "x".repeat(512).as_str().try_into().unwrap(),
        }),
        wire(&largest_card()),
        wire(&Message::Presence(Presence {
            activity: Some(Activity::Working),
            detail: "BUILD".try_into().unwrap(),
            expression: Expression::Focused,
            gaze: Gaze::Right,
            eyes: EyeStyle::HalfLidded,
            ttl_s: 30,
        })),
        wire(&Reply::Pong {
            nonce: 0,
            version: protocol::VERSION,
        }),
        wire(&Reply::Ack { seq: u32::MAX }),
        wire(&Reply::Rejected { count: u32::MAX }),
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
        2,
        253,
        254,
        255,
        MAX_FRAME_BYTES - 1,
        MAX_FRAME_BYTES,
        MAX_FRAME_BYTES + 1,
        MAX_INPUT_BYTES,
    ];
    let len = if rng.range(2) == 0 {
        boundaries[rng.range(boundaries.len())]
    } else {
        rng.range(MAX_INPUT_BYTES + 1)
    };
    (0..len).map(|_| rng.next() as u8).collect()
}

fn exercise(input: &[u8]) {
    let mut message_bytes = input.to_vec();
    if let Ok(Message::Card(card)) = protocol::decode::<Message>(&mut message_bytes) {
        let _ = card.validate();
    }
    let mut reply_bytes = input.to_vec();
    let _ = protocol::decode::<Reply>(&mut reply_bytes);
}

fn checked_exercise(input: &[u8], seed: u64, index: usize) {
    let result = std::panic::catch_unwind(|| exercise(input));
    if let Err(payload) = result {
        eprintln!("decoder panic: seed={seed:#018x}, case={index}, bytes={input:02x?}");
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
        let mut input = if rng.range(3) == 0 {
            arbitrary_input(&mut rng)
        } else {
            corpus[rng.range(corpus.len())].clone()
        };
        for _ in 0..=rng.range(4) {
            mutate(&mut input, &mut rng);
        }
        checked_exercise(&input, seed, corpus.len() + index);
    }
    println!(
        "decoder stress passed: {} frames, seed {seed:#018x}",
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
