//! 実機と同じ描画処理を PC 上で実行し、表示状態を画像で比較する。

use std::{
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use embedded_graphics::{
    pixelcolor::{Rgb565, RgbColor},
    prelude::{DrawTarget, OriginDimensions, Pixel, Point, Size},
};
use my_stackchan_firmware::{model::Controller, renderer};
use protocol::{
    Activity, Card, Element, Emote, Expression, EyeStyle, Gaze, ImageBegin, ImageData, ImageRows,
    MAX_CARD_TEXT_BYTES, MAX_IMAGE_ROWS_BYTES, MAX_TEXT_BYTES, Message, Presence, Reply, Row, Slot,
};

const WIDTH: usize = 320;
const HEIGHT: usize = 240;

#[derive(Debug)]
struct ScreenError(Point);

impl std::fmt::Display for ScreenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "画面外への描画: {:?}", self.0)
    }
}

impl std::error::Error for ScreenError {}

struct Screen {
    pixels: Vec<Rgb565>,
}

impl Default for Screen {
    fn default() -> Self {
        Self {
            pixels: vec![Rgb565::BLACK; WIDTH * HEIGHT],
        }
    }
}

impl OriginDimensions for Screen {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for Screen {
    type Color = Rgb565;
    type Error = ScreenError;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Self::Error> {
        for Pixel(Point { x, y }, color) in pixels {
            if !(0..WIDTH as i32).contains(&x) || !(0..HEIGHT as i32).contains(&y) {
                return Err(ScreenError(Point::new(x, y)));
            }
            self.pixels[y as usize * WIDTH + x as usize] = color;
        }
        Ok(())
    }
}

/// 24-bit BMP は Windows とブラウザーで開け、追加 crate や非固定の画像変換器を要しない。
fn write_bmp(mut output: impl Write, screen: &Screen) -> io::Result<()> {
    let row_bytes = WIDTH * 3;
    let padding = (4 - row_bytes % 4) % 4;
    let image_bytes = (row_bytes + padding) * HEIGHT;
    let file_bytes = 54 + image_bytes;

    output.write_all(b"BM")?;
    output.write_all(&(file_bytes as u32).to_le_bytes())?;
    output.write_all(&[0; 4])?;
    output.write_all(&54u32.to_le_bytes())?;
    output.write_all(&40u32.to_le_bytes())?;
    output.write_all(&(WIDTH as i32).to_le_bytes())?;
    // 負の高さで上から下へ格納し、DrawTarget の座標系と一致させる。
    output.write_all(&(-(HEIGHT as i32)).to_le_bytes())?;
    output.write_all(&1u16.to_le_bytes())?;
    output.write_all(&24u16.to_le_bytes())?;
    output.write_all(&0u32.to_le_bytes())?;
    output.write_all(&(image_bytes as u32).to_le_bytes())?;
    output.write_all(&[0; 16])?;

    for row in screen.pixels.chunks_exact(WIDTH) {
        for color in row {
            let red = color.r();
            let green = color.g();
            let blue = color.b();
            output.write_all(&[
                (blue << 3) | (blue >> 2),
                (green << 2) | (green >> 4),
                (red << 3) | (red >> 2),
            ])?;
        }
        output.write_all(&[0; 3][..padding])?;
    }
    Ok(())
}

fn save_bmp(path: &Path, screen: &Screen) -> io::Result<()> {
    write_bmp(BufWriter::new(File::create(path)?), screen)
}

struct Options {
    output_dir: PathBuf,
    top: String,
    bottom: String,
    overlay: String,
    ttl_s: u16,
    card_title: String,
    card_ratio: u8,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from(".work/simulation"),
            top: "USB Text OK".into(),
            bottom: "BannerBottom OK".into(),
            overlay: "Overlay test".into(),
            ttl_s: 5,
            card_title: "CPU".into(),
            card_ratio: 75,
        }
    }
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self::default();
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("{flag} に値が必要です"))?;
            match flag.as_str() {
                "--out" => options.output_dir = value.into(),
                "--top" => options.top = value,
                "--bottom" => options.bottom = value,
                "--overlay" => options.overlay = value,
                "--card-title" => options.card_title = value,
                "--card-ratio" => {
                    options.card_ratio = value
                        .parse()
                        .map_err(|_| "--card-ratio must be an integer from 0 to 100")?;
                }
                "--ttl" => {
                    options.ttl_s = value
                        .parse()
                        .map_err(|_| "--ttl は 1..65535 の整数にしてください")?;
                }
                _ => return Err(format!("不明なオプション: {flag}")),
            }
        }
        if options.ttl_s == 0 {
            return Err("--ttl は 1..65535 の整数にしてください".into());
        }
        if options.card_ratio > 100 {
            return Err("--card-ratio must be an integer from 0 to 100".into());
        }
        if options.card_title.len() > MAX_CARD_TEXT_BYTES {
            return Err(format!(
                "--card-title must be at most {MAX_CARD_TEXT_BYTES} UTF-8 bytes"
            ));
        }
        for (name, text) in [
            ("--top", &options.top),
            ("--bottom", &options.bottom),
            ("--overlay", &options.overlay),
        ] {
            if text.len() > MAX_TEXT_BYTES {
                return Err(format!(
                    "{name} は UTF-8 で {MAX_TEXT_BYTES} byte 以下にしてください"
                ));
            }
        }
        Ok(options)
    }
}

fn text(slot: Slot, content: &str, ttl_s: u16) -> Message {
    Message::Text {
        slot,
        ttl_s,
        text: content.try_into().expect("validated text length"),
    }
}

fn card(options: &Options) -> Message {
    let mut card = Card {
        slot: Slot::BannerTop,
        ttl_s: 0,
        id: 0,
        rows: Default::default(),
        image: None,
    };
    let mut title = Row {
        elements: Default::default(),
        action: None,
    };
    title
        .elements
        .push(Element::Text {
            text: options
                .card_title
                .as_str()
                .try_into()
                .expect("validated title"),
        })
        .expect("title row has capacity");
    card.rows.push(title).expect("card has capacity");
    let mut bar = Row {
        elements: Default::default(),
        action: None,
    };
    bar.elements
        .push(Element::Bar {
            ratio: options.card_ratio,
            label: "Usage".try_into().expect("fixed label fits"),
        })
        .expect("bar row has capacity");
    card.rows.push(bar).expect("card has capacity");
    card.validate().expect("valid card layout");
    Message::Card(card)
}

fn image_card() -> Message {
    let mut pixels = heapless::Vec::new();
    for _row in 0..16 {
        for column in 0..16 {
            let color: u16 = [0xF800, 0x07E0, 0x001F, 0xFFFF][column / 4];
            pixels
                .extend_from_slice(&color.to_be_bytes())
                .expect("16x16 image fits");
        }
    }
    let mut row = Row {
        elements: Default::default(),
        action: None,
    };
    row.elements
        .push(Element::Text {
            text: "RGB565".try_into().unwrap(),
        })
        .unwrap();
    row.elements.push(Element::Image).unwrap();
    let mut card = Card {
        slot: Slot::BannerTop,
        ttl_s: 0,
        id: 0,
        rows: Default::default(),
        image: Some(ImageData {
            width: 16,
            height: 16,
            pixels,
        }),
    };
    card.rows.push(row).unwrap();
    card.validate().expect("valid image card");
    Message::Card(card)
}

fn presence(
    activity: Option<Activity>,
    detail: &str,
    expression: Expression,
    gaze: Gaze,
    ttl_s: u16,
) -> Message {
    presence_with_eyes(activity, detail, expression, gaze, EyeStyle::Auto, ttl_s)
}

fn presence_with_eyes(
    activity: Option<Activity>,
    detail: &str,
    expression: Expression,
    gaze: Gaze,
    eyes: EyeStyle,
    ttl_s: u16,
) -> Message {
    Message::Presence(Presence {
        activity,
        detail: detail.try_into().expect("simulation detail fits"),
        expression,
        gaze,
        eyes,
        ttl_s,
    })
}

fn apply(controller: &mut Controller, screen: &mut Screen, message: Message) {
    let reply = controller
        .handle(message, 0, screen)
        .expect("screen is infallible");
    assert!(matches!(reply, Reply::Ack { .. }));
}

/// 画像領域の試験模様 (色相の横方向の変化と縦方向の明度) を ImageBegin/Rows/End で送る。
fn apply_image_region(controller: &mut Controller, screen: &mut Screen, width: u16, height: u16) {
    apply(
        controller,
        screen,
        Message::ImageBegin(ImageBegin {
            id: 1,
            x: (WIDTH as u16 - width) / 2,
            y: (HEIGHT as u16 - height) / 2,
            width,
            height,
            ttl_s: 0,
        }),
    );
    let mut pixels = Vec::new();
    for y in 0..u32::from(height) {
        for x in 0..u32::from(width) {
            let r = (x * 31 / u32::from(width)) as u16;
            let g = (y * 63 / u32::from(height)) as u16;
            let b = 31 - r;
            pixels.extend_from_slice(&((r << 11) | (g << 5) | b).to_be_bytes());
        }
    }
    let row_bytes = usize::from(width) * 2;
    let rows_per_message = MAX_IMAGE_ROWS_BYTES / row_bytes;
    for (index, chunk) in pixels.chunks(row_bytes * rows_per_message).enumerate() {
        apply(
            controller,
            screen,
            Message::ImageRows(ImageRows {
                id: 1,
                row: (index * rows_per_message) as u16,
                pixels: heapless::Vec::from_slice(chunk).unwrap(),
            }),
        );
    }
    apply(controller, screen, Message::ImageEnd { id: 1 });
}

fn generate(options: &Options) -> io::Result<()> {
    fs::create_dir_all(&options.output_dir)?;
    let mut screen = Screen::default();
    let mut controller = Controller::default();

    renderer::draw(&mut screen).expect("screen is infallible");
    save_bmp(&options.output_dir.join("01-startup.bmp"), &screen)?;

    apply(
        &mut controller,
        &mut screen,
        text(Slot::BannerTop, &options.top, 0),
    );
    apply(
        &mut controller,
        &mut screen,
        text(Slot::BannerBottom, &options.bottom, 0),
    );
    save_bmp(&options.output_dir.join("02-banners.bmp"), &screen)?;

    apply(
        &mut controller,
        &mut screen,
        text(Slot::Overlay, &options.overlay, options.ttl_s),
    );
    save_bmp(&options.output_dir.join("03-overlay.bmp"), &screen)?;

    controller
        .tick(u64::from(options.ttl_s) * 1000, &mut screen)
        .expect("screen is infallible");
    save_bmp(&options.output_dir.join("04-expired.bmp"), &screen)?;

    apply(&mut controller, &mut screen, card(options));
    save_bmp(&options.output_dir.join("05-card.bmp"), &screen)?;

    apply(&mut controller, &mut screen, image_card());
    save_bmp(&options.output_dir.join("06-image.bmp"), &screen)?;

    apply(
        &mut controller,
        &mut screen,
        presence(
            Some(Activity::Working),
            "BUILD",
            Expression::Focused,
            Gaze::Right,
            0,
        ),
    );
    save_bmp(&options.output_dir.join("07-working.bmp"), &screen)?;
    apply(
        &mut controller,
        &mut screen,
        presence(
            Some(Activity::Waiting),
            "REVIEW",
            Expression::Sleepy,
            Gaze::Up,
            0,
        ),
    );
    save_bmp(&options.output_dir.join("08-waiting.bmp"), &screen)?;
    apply(
        &mut controller,
        &mut screen,
        presence(
            Some(Activity::Error),
            "FAILED",
            Expression::Worried,
            Gaze::Down,
            0,
        ),
    );
    save_bmp(&options.output_dir.join("09-error.bmp"), &screen)?;
    apply(
        &mut controller,
        &mut screen,
        presence(None, "", Expression::Surprised, Gaze::Left, 0),
    );
    save_bmp(&options.output_dir.join("10-face.bmp"), &screen)?;
    for (name, expression) in [
        ("11-grin.bmp", Expression::Grin),
        ("12-calm.bmp", Expression::Calm),
        ("13-curious.bmp", Expression::Curious),
        ("14-playful.bmp", Expression::Playful),
        ("15-wink.bmp", Expression::Wink),
        ("16-sad.bmp", Expression::Sad),
        ("17-determined.bmp", Expression::Determined),
    ] {
        apply(
            &mut controller,
            &mut screen,
            presence(None, "", expression, Gaze::Center, 0),
        );
        save_bmp(&options.output_dir.join(name), &screen)?;
    }
    for (name, gaze, eyes) in [
        ("18-look-left.bmp", Gaze::Left, EyeStyle::Open),
        ("19-look-right.bmp", Gaze::Right, EyeStyle::Open),
        ("20-look-up.bmp", Gaze::Up, EyeStyle::Open),
        ("21-look-down.bmp", Gaze::Down, EyeStyle::Open),
        ("22-eyes-wide.bmp", Gaze::Center, EyeStyle::Wide),
        ("23-eyes-closed.bmp", Gaze::Center, EyeStyle::Closed),
        (
            "24-eyes-half-lidded.bmp",
            Gaze::Center,
            EyeStyle::HalfLidded,
        ),
    ] {
        apply(
            &mut controller,
            &mut screen,
            presence_with_eyes(None, "", Expression::Happy, gaze, eyes, 0),
        );
        save_bmp(&options.output_dir.join(name), &screen)?;
    }
    apply(
        &mut controller,
        &mut screen,
        Message::Emote(Emote {
            expression: Expression::Curious,
            gaze: Gaze::Point { x: -60, y: 25 },
            eyes: EyeStyle::Wide,
            intensity: 75,
            duration_ms: 800,
        }),
    );
    save_bmp(&options.output_dir.join("25-emote.bmp"), &screen)?;
    controller
        .tick(800, &mut screen)
        .expect("screen is infallible");
    save_bmp(&options.output_dir.join("26-emote-expired.bmp"), &screen)?;
    apply_image_region(&mut controller, &mut screen, 160, 120);
    save_bmp(&options.output_dir.join("27-image-region.bmp"), &screen)?;
    apply(
        &mut controller,
        &mut screen,
        Message::ClearSlot(Slot::Overlay),
    );
    for index in 0..my_stackchan_firmware::model::DEMO_FACE_COUNT {
        controller
            .tap(index as u64, &mut screen)
            .expect("screen is infallible");
        save_bmp(
            &options
                .output_dir
                .join(format!("demo-{:02}.bmp", index + 1)),
            &screen,
        )?;
    }

    fs::write(
        options.output_dir.join("index.html"),
        gallery_html(options.ttl_s),
    )?;
    Ok(())
}

fn gallery_html(ttl_s: u16) -> String {
    let mut html = format!(
        r#"<!doctype html>
<html lang="ja">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>my-stackchan 表示シミュレーション</title>
<style>
body {{ margin: 2rem auto; max-width: 82rem; padding: 0 1rem; font: 1rem/1.5 system-ui, sans-serif; background: #f4f5f7; color: #17212b; }}
h1 {{ font-size: 1.5rem; }}
.frames {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(330px, 1fr)); gap: 1rem; }}
figure {{ margin: 0; padding: 1rem; background: white; border: 1px solid #d8dee6; border-radius: .5rem; }}
img {{ display: block; width: 320px; height: 240px; max-width: 100%; image-rendering: pixelated; background: black; }}
figcaption {{ margin-top: .5rem; font-weight: 600; }}
</style>
<h1>my-stackchan 表示シミュレーション</h1>
<p>実機と同じ描画・表示状態のコードを PC 上で実行した結果。時刻は表示命令の受理からの経過時間です。</p>
<div class="frames">
<figure><img src="01-startup.bmp" width="320" height="240" alt="起動確認画面"><figcaption>1. 起動時</figcaption></figure>
<figure><img src="02-banners.bmp" width="320" height="240" alt="上下の帯と顔"><figcaption>2. 上下の帯</figcaption></figure>
<figure><img src="03-overlay.bmp" width="320" height="240" alt="Overlay 表示"><figcaption>3. Overlay 表示直後</figcaption></figure>
<figure><img src="04-expired.bmp" width="320" height="240" alt="期限満了後の上下の帯と顔"><figcaption>4. Overlay 期限満了後（{ttl_s} 秒）</figcaption></figure>
<figure><img src="05-card.bmp" width="320" height="240" alt="Card layout"><figcaption>5. Card layout</figcaption></figure>
<figure><img src="06-image.bmp" width="320" height="240" alt="RGB565 image"><figcaption>6. RGB565 image</figcaption></figure>
<figure><img src="07-working.bmp" width="320" height="240" alt="作業中の表情と状態"><figcaption>7. Working / Focused / Right</figcaption></figure>
<figure><img src="08-waiting.bmp" width="320" height="240" alt="待機中の表情と状態"><figcaption>8. Waiting / Sleepy / Up</figcaption></figure>
<figure><img src="09-error.bmp" width="320" height="240" alt="エラー時の表情と状態"><figcaption>9. Error / Worried / Down</figcaption></figure>
<figure><img src="10-face.bmp" width="320" height="240" alt="驚いた表情と左視線"><figcaption>10. Surprised / Left</figcaption></figure>
<figure><img src="11-grin.bmp" width="320" height="240" alt="笑顔"><figcaption>11. Grin</figcaption></figure>
<figure><img src="12-calm.bmp" width="320" height="240" alt="穏やかな表情"><figcaption>12. Calm</figcaption></figure>
<figure><img src="13-curious.bmp" width="320" height="240" alt="興味を示す表情"><figcaption>13. Curious</figcaption></figure>
<figure><img src="14-playful.bmp" width="320" height="240" alt="遊び心のある表情"><figcaption>14. Playful</figcaption></figure>
<figure><img src="15-wink.bmp" width="320" height="240" alt="ウィンク"><figcaption>15. Wink</figcaption></figure>
<figure><img src="16-sad.bmp" width="320" height="240" alt="悲しい表情"><figcaption>16. Sad</figcaption></figure>
<figure><img src="17-determined.bmp" width="320" height="240" alt="決意した表情"><figcaption>17. Determined</figcaption></figure>
<figure><img src="18-look-left.bmp" width="320" height="240" alt="左を見る"><figcaption>18. Look left</figcaption></figure>
<figure><img src="19-look-right.bmp" width="320" height="240" alt="右を見る"><figcaption>19. Look right</figcaption></figure>
<figure><img src="20-look-up.bmp" width="320" height="240" alt="上を見る"><figcaption>20. Look up</figcaption></figure>
<figure><img src="21-look-down.bmp" width="320" height="240" alt="下を見る"><figcaption>21. Look down</figcaption></figure>
<figure><img src="22-eyes-wide.bmp" width="320" height="240" alt="目を見開く"><figcaption>22. Eyes wide</figcaption></figure>
<figure><img src="23-eyes-closed.bmp" width="320" height="240" alt="目を閉じる"><figcaption>23. Eyes closed</figcaption></figure>
<figure><img src="24-eyes-half-lidded.bmp" width="320" height="240" alt="ジト目"><figcaption>24. Half-lidded eyes</figcaption></figure>
<figure><img src="25-emote.bmp" width="320" height="240" alt="一時的な表情と二軸視線"><figcaption>25. Emote / Point</figcaption></figure>
<figure><img src="26-emote-expired.bmp" width="320" height="240" alt="期限後の表情"><figcaption>26. Emote expired</figcaption></figure>
<figure><img src="27-image-region.bmp" width="320" height="240" alt="画像領域 160x120"><figcaption>27. Image region 160x120</figcaption></figure>
"#
    );
    for index in 1..=my_stackchan_firmware::model::DEMO_FACE_COUNT {
        html.push_str(&format!(
            "<figure><img src=\"demo-{index:02}.bmp\" width=\"320\" height=\"240\" alt=\"タップデモ {index}\"><figcaption>Tap {index}</figcaption></figure>\n"
        ));
    }
    html.push_str("</div>\n</html>\n");
    html
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "usage: simulate [--out DIR] [--top TEXT] [--bottom TEXT] [--overlay TEXT] [--ttl SECONDS (1..65535)] [--card-title TEXT] [--card-ratio 0..100]"
        );
        return Ok(());
    }
    let options = Options::parse(args.into_iter())?;
    generate(&options)?;
    println!("{}", options.output_dir.join("index.html").display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmp_has_expected_dimensions_colors_and_top_down_order() {
        let mut screen = Screen::default();
        screen.pixels[0] = Rgb565::RED;
        screen.pixels[WIDTH - 1] = Rgb565::GREEN;
        screen.pixels[(HEIGHT - 1) * WIDTH] = Rgb565::BLUE;
        let mut bmp = Vec::new();
        write_bmp(&mut bmp, &screen).unwrap();
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(
            u32::from_le_bytes(bmp[2..6].try_into().unwrap()) as usize,
            bmp.len()
        );
        assert_eq!(
            i32::from_le_bytes(bmp[18..22].try_into().unwrap()),
            WIDTH as i32
        );
        assert_eq!(
            i32::from_le_bytes(bmp[22..26].try_into().unwrap()),
            -(HEIGHT as i32)
        );
        assert_eq!(&bmp[54..57], &[0, 0, 255]);
        assert_eq!(&bmp[54 + (WIDTH - 1) * 3..54 + WIDTH * 3], &[0, 255, 0]);
        assert_eq!(
            &bmp[54 + (HEIGHT - 1) * WIDTH * 3..54 + (HEIGHT - 1) * WIDTH * 3 + 3],
            &[255, 0, 0]
        );
    }

    #[test]
    fn screen_rejects_out_of_bounds_pixels() {
        let mut screen = Screen::default();
        assert!(
            screen
                .draw_iter([Pixel(Point::new(-1, 0), Rgb565::WHITE)])
                .is_err()
        );
        assert!(
            screen
                .draw_iter([Pixel(Point::new(320, 0), Rgb565::WHITE)])
                .is_err()
        );
    }

    #[test]
    fn simulator_reuses_controller_and_renderer_for_ttl_transition() {
        let mut controller = Controller::default();
        let mut screen = Screen::default();
        apply(
            &mut controller,
            &mut screen,
            text(Slot::BannerTop, "VISIBLE", 0),
        );
        let banners = screen.pixels.clone();
        apply(
            &mut controller,
            &mut screen,
            text(Slot::Overlay, "OVERLAY", 1),
        );
        assert_ne!(screen.pixels, banners);
        controller.tick(999, &mut screen).unwrap();
        assert_ne!(screen.pixels, banners);
        controller.tick(1000, &mut screen).unwrap();
        assert_eq!(screen.pixels, banners);
    }

    #[test]
    fn options_reject_invalid_ttl_and_utf8_byte_limit() {
        assert!(Options::parse(["--ttl", "0"].into_iter().map(String::from)).is_err());
        assert!(Options::parse(["--ttl", "65536"].into_iter().map(String::from)).is_err());
        assert!(
            Options::parse(["--top", &"日".repeat(171)].into_iter().map(String::from)).is_err()
        );
        assert!(Options::parse(["--top", &"日".repeat(170)].into_iter().map(String::from)).is_ok());
    }

    #[test]
    fn options_reject_invalid_card_values() {
        assert!(Options::parse(["--card-ratio", "101"].into_iter().map(String::from)).is_err());
        assert!(
            Options::parse(
                ["--card-title", &"x".repeat(49)]
                    .into_iter()
                    .map(String::from)
            )
            .is_err()
        );
    }
}
