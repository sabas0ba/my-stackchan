//! 画像の表示を試す plugin。plugin API の `ImageFrame` の参照実装を兼ねる。
//!
//! 帯に 1 行の Card を置き、その行をタップすると (daemon の `input forward` 時)、試験模様を
//! 1 秒に 1 枚、10 秒間送る。`param.autoplay = "true"` の場合は起動時にも 1 回送る
//! (タップを使えない環境での確認用)。
//!
//! 画像はデバイスの画像領域の上限 (`Limits`) の大きさで生成する。カメラ等の実際の画像を
//! 扱う plugin も、デコードと縮小を plugin 側で行い、同じ形式 (RGB565 big-endian) で送る。

use std::time::{Duration, Instant};

use plugin_api::client::{self, ClientError};
use plugin_api::{
    API_VERSION, Capabilities, CardPut, Element, Hello, HostMessage, ImageFrame, LogLevel,
    MAX_IMAGE_HEIGHT, MAX_IMAGE_WIDTH, Placement, PluginMessage, Priority, Row,
};

const CARD: u8 = 0;
/// 行のタップで返される値。
const PLAY: u8 = 0;
const FRAMES: u16 = 10;
const FRAME_INTERVAL: Duration = Duration::from_secs(1);
/// 各画像の表示を保つ秒数。次の画像が届かない場合 (plugin の停止等) に速やかに顔へ戻す。
const FRAME_TTL_S: u16 = 2;

fn banner() -> PluginMessage {
    PluginMessage::CardPut(CardPut {
        card: CARD,
        placement: Placement::Banner,
        priority: Priority::Low,
        ttl_s: 0,
        rows: vec![Row {
            elements: vec![Element::Text {
                text: "Image demo".into(),
            }],
            action: Some(PLAY),
        }],
    })
}

fn rgb565(r: u32, g: u32, b: u32) -> [u8; 2] {
    // 各成分は 0..=255 で受け取り、5/6/5 bit に落とす。
    let value = ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3);
    (value as u16).to_be_bytes()
}

/// `index` 枚目の試験模様。横方向の色の変化と縦方向の明度に、枚数に応じて右へ移る縦帯を
/// 重ねる。帯の位置で画像が更新されたことを目視で確かめられる。
fn pattern(width: u16, height: u16, index: u16) -> Vec<u8> {
    let (width, height) = (u32::from(width), u32::from(height));
    let bar_width = (width / 10).max(1);
    let bar = u32::from(index % 10) * width / 10;
    let mut pixels = Vec::with_capacity((width * height * 2) as usize);
    for y in 0..height {
        for x in 0..width {
            let color = if (bar..bar + bar_width).contains(&x) {
                rgb565(255, 255, 255)
            } else {
                let r = x * 255 / width;
                let g = y * 255 / height;
                rgb565(r, g, 255 - r)
            };
            pixels.extend_from_slice(&color);
        }
    }
    pixels
}

fn frame(width: u16, height: u16, index: u16) -> PluginMessage {
    PluginMessage::ImageFrame(ImageFrame {
        width,
        height,
        ttl_s: FRAME_TTL_S,
        pixels: pattern(width, height, index),
    })
}

fn run() -> Result<(), ClientError> {
    let mut connection = client::connect_stdio(Hello {
        api_version: API_VERSION,
        name: "image-demo".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        capabilities: Capabilities {
            cards: 1,
            image: true,
            ..Capabilities::default()
        },
    })?;
    let init = connection.init();
    if !init.granted.image {
        connection.send(&PluginMessage::Log {
            level: LogLevel::Error,
            text: "image が許可されていません (設定の image = true)".into(),
        })?;
        return Ok(());
    }
    let width = init.limits.image_width.min(MAX_IMAGE_WIDTH);
    let height = init.limits.image_height.min(MAX_IMAGE_HEIGHT);
    let autoplay = connection.param("autoplay") == Some("true");
    connection.send(&banner())?;

    // 再生中は次の画像を送る時刻と枚数を持つ。
    let mut playing = autoplay.then(|| (Instant::now(), 0));
    loop {
        let wait = match &mut playing {
            Some((next, index)) => {
                let now = Instant::now();
                if now >= *next {
                    connection.send(&frame(width, height, *index))?;
                    *index += 1;
                    *next = now + FRAME_INTERVAL;
                }
                if *index >= FRAMES {
                    playing = None;
                    Duration::from_secs(60)
                } else {
                    next.saturating_duration_since(Instant::now())
                }
            }
            None => Duration::from_secs(60),
        };
        match connection.recv_timeout(wait)? {
            Some(HostMessage::Shutdown) => return Ok(()),
            Some(HostMessage::Rejected { reason }) => eprintln!("rejected: {reason}"),
            Some(HostMessage::Action {
                card: CARD,
                action: PLAY,
            }) if playing.is_none() => {
                playing = Some((Instant::now(), 0));
            }
            _ if connection.is_closed() => return Ok(()),
            _ => {}
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_valid_and_differ_by_index() {
        let first = frame(MAX_IMAGE_WIDTH, MAX_IMAGE_HEIGHT, 0);
        assert_eq!(first.validate(), Ok(()));
        assert_ne!(first, frame(MAX_IMAGE_WIDTH, MAX_IMAGE_HEIGHT, 1));
        // 小さな上限を通知された場合と、帯の幅が 0 にならない最小の大きさ。
        assert_eq!(frame(7, 3, 9).validate(), Ok(()));
        assert_eq!(frame(1, 1, 0).validate(), Ok(()));
    }

    #[test]
    fn white_bar_moves_with_index() {
        let pixels = pattern(20, 1, 3);
        let white = rgb565(255, 255, 255);
        assert_eq!(&pixels[6 * 2..6 * 2 + 2], &white);
        assert_ne!(&pixels[0..2], &white);
    }

    #[test]
    fn banner_fits_limits() {
        assert_eq!(banner().validate(), Ok(()));
    }
}
