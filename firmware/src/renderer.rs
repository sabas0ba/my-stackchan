//! 起動確認画面と Slot の描画。描画先は任意の DrawTarget で、実機では
//! `framebuffer::FrameBuffer` に描いてから変化した範囲だけを LCD へ送る。

use crate::model::{Content, DisplayState, ImageRegion};
use embedded_graphics::{
    image::{Image, ImageRawBE},
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use protocol::{Activity, Card, Element, Expression, EyeStyle, Gaze, ImageData, Presence, Slot};

const IMAGE_WIDTH: usize = 96;
const IMAGE_HEIGHT: usize = 16;

// RGB と白の順序から、色順・RGB565 のバイト順を実機で確認できる。
const fn color_bars() -> [u8; IMAGE_WIDTH * IMAGE_HEIGHT * 2] {
    let colors: [u16; 4] = [0xf800, 0x07e0, 0x001f, 0xffff];
    let mut bytes = [0; IMAGE_WIDTH * IMAGE_HEIGHT * 2];
    let mut pixel = 0;
    while pixel < IMAGE_WIDTH * IMAGE_HEIGHT {
        let color = colors[(pixel % IMAGE_WIDTH) / (IMAGE_WIDTH / 4)].to_be_bytes();
        bytes[pixel * 2] = color[0];
        bytes[pixel * 2 + 1] = color[1];
        pixel += 1;
    }
    bytes
}

static COLOR_BARS: [u8; IMAGE_WIDTH * IMAGE_HEIGHT * 2] = color_bars();

pub fn draw<D: DrawTarget<Color = Rgb565>>(display: &mut D) -> Result<(), D::Error> {
    draw_startup_with_blink(display, false)
}

pub fn draw_startup_with_blink<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    blink: bool,
) -> Result<(), D::Error> {
    display.clear(Rgb565::BLACK)?;
    Rectangle::new(Point::zero(), Size::new(320, 240))
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
        .draw(display)?;
    Text::new(
        "my-stackchan / CoreS3",
        Point::new(12, 25),
        MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE),
    )
    .draw(display)?;

    draw_face(
        display,
        Expression::Happy,
        Gaze::Center,
        EyeStyle::Auto,
        blink,
    )?;

    Text::new(
        "ASCII 0123456789 !?",
        Point::new(12, 180),
        MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE),
    )
    .draw(display)?;
    Text::new(
        "RGB565",
        Point::new(12, 217),
        MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE),
    )
    .draw(display)?;
    let image = ImageRawBE::<Rgb565>::new(&COLOR_BARS, IMAGE_WIDTH as u32);
    Image::new(&image, Point::new(112, 200)).draw(display)?;
    Ok(())
}

fn draw_face<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    expression: Expression,
    gaze: Gaze,
    eyes: EyeStyle,
    blink: bool,
) -> Result<(), D::Error> {
    let eyes = match eyes {
        EyeStyle::Auto if blink => EyeStyle::Closed,
        value => value,
    };
    let (dx, dy) = match gaze {
        Gaze::Center => (0, 0),
        Gaze::Left => (-8, 0),
        Gaze::Right => (8, 0),
        Gaze::Up => (0, -7),
        Gaze::Down => (0, 7),
        Gaze::Point { x, y } => (
            i32::from(x.clamp(-100, 100)) * 8 / 100,
            i32::from(y.clamp(-100, 100)) * 7 / 100,
        ),
    };
    for (index, x) in [90, 202].into_iter().enumerate() {
        draw_eye(display, x + dx, 72 + dy, index, expression, eyes)?;
    }
    let brows: &[(i32, i32, i32, i32)] = match expression {
        Expression::Focused | Expression::Determined => &[(90, 65, 118, 70), (202, 70, 230, 65)],
        Expression::Worried | Expression::Sad => &[(90, 70, 118, 65), (202, 65, 230, 70)],
        Expression::Curious => &[(202, 65, 230, 61)],
        _ => &[],
    };
    for &(x1, y1, x2, y2) in brows {
        Line::new(Point::new(x1 + dx, y1 + dy), Point::new(x2 + dx, y2 + dy))
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 2))
            .draw(display)?;
    }

    match expression {
        Expression::Happy => draw_mouth_lines(
            display,
            &[
                (130, 126, 145, 140),
                (145, 140, 175, 140),
                (175, 140, 190, 126),
            ],
        )?,
        Expression::Focused => draw_mouth_lines(display, &[(140, 138, 180, 138)])?,
        Expression::Sleepy => draw_mouth_lines(display, &[(148, 140, 172, 140)])?,
        Expression::Worried => draw_mouth_lines(
            display,
            &[
                (130, 140, 145, 126),
                (145, 126, 175, 126),
                (175, 126, 190, 140),
            ],
        )?,
        Expression::Surprised => Circle::new(Point::new(150, 126), 20)
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 3))
            .draw(display)?,
        Expression::Grin => draw_mouth_lines(
            display,
            &[
                (130, 125, 190, 125),
                (130, 125, 144, 143),
                (144, 143, 176, 143),
                (176, 143, 190, 125),
            ],
        )?,
        Expression::Calm => draw_mouth_lines(
            display,
            &[
                (140, 130, 150, 136),
                (150, 136, 170, 136),
                (170, 136, 180, 130),
            ],
        )?,
        Expression::Curious => draw_mouth_lines(
            display,
            &[
                (135, 130, 149, 139),
                (149, 139, 173, 137),
                (173, 137, 188, 128),
            ],
        )?,
        Expression::Playful => draw_mouth_lines(
            display,
            &[
                (135, 128, 185, 128),
                (150, 128, 150, 144),
                (150, 144, 160, 150),
                (160, 150, 170, 144),
                (170, 144, 170, 128),
            ],
        )?,
        Expression::Wink => draw_mouth_lines(
            display,
            &[
                (130, 126, 145, 140),
                (145, 140, 175, 140),
                (175, 140, 190, 126),
            ],
        )?,
        Expression::Sad => draw_mouth_lines(
            display,
            &[
                (130, 143, 145, 129),
                (145, 129, 175, 129),
                (175, 129, 190, 143),
            ],
        )?,
        Expression::Determined => draw_mouth_lines(
            display,
            &[
                (132, 131, 148, 140),
                (148, 140, 172, 140),
                (172, 140, 188, 131),
            ],
        )?,
    }
    Ok(())
}

fn draw_eye<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    x: i32,
    y: i32,
    index: usize,
    expression: Expression,
    eyes: EyeStyle,
) -> Result<(), D::Error> {
    let outline = PrimitiveStyle::with_stroke(Rgb565::WHITE, 3);
    match eyes {
        EyeStyle::Auto => {}
        EyeStyle::Open => {
            Circle::new(Point::new(x, y), 28)
                .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
                .draw(display)?;
            return Ok(());
        }
        EyeStyle::Wide => {
            Circle::new(Point::new(x - 3, y - 3), 34)
                .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
                .draw(display)?;
            return Ok(());
        }
        EyeStyle::Closed => {
            Line::new(Point::new(x, y + 14), Point::new(x + 27, y + 14))
                .into_styled(outline)
                .draw(display)?;
            return Ok(());
        }
        EyeStyle::HalfLidded => {
            Circle::new(Point::new(x, y), 28)
                .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
                .draw(display)?;
            Rectangle::new(Point::new(x - 1, y - 1), Size::new(30, 15))
                .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
                .draw(display)?;
            Line::new(Point::new(x, y + 14), Point::new(x + 27, y + 14))
                .into_styled(outline)
                .draw(display)?;
            return Ok(());
        }
    }
    match expression {
        Expression::Sleepy | Expression::Calm => {
            Line::new(Point::new(x, y + 14), Point::new(x + 27, y + 14))
                .into_styled(outline)
                .draw(display)?;
        }
        Expression::Grin | Expression::Wink if expression == Expression::Grin || index == 1 => {
            for (start, end) in [
                (Point::new(x, y + 13), Point::new(x + 8, y + 5)),
                (Point::new(x + 8, y + 5), Point::new(x + 19, y + 5)),
                (Point::new(x + 19, y + 5), Point::new(x + 27, y + 13)),
            ] {
                Line::new(start, end).into_styled(outline).draw(display)?;
            }
        }
        Expression::Playful if index == 0 => {
            for (start, end) in [
                (Point::new(x, y + 7), Point::new(x + 17, y + 14)),
                (Point::new(x + 17, y + 14), Point::new(x, y + 21)),
            ] {
                Line::new(start, end).into_styled(outline).draw(display)?;
            }
        }
        _ => {
            let eye_y = if expression == Expression::Curious && index == 1 {
                y - 5
            } else {
                y
            };
            Circle::new(Point::new(x, eye_y), 28)
                .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
                .draw(display)?;
        }
    }
    Ok(())
}

fn draw_mouth_lines<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    segments: &[(i32, i32, i32, i32)],
) -> Result<(), D::Error> {
    for &(x1, y1, x2, y2) in segments {
        Line::new(Point::new(x1, y1), Point::new(x2, y2))
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 4))
            .draw(display)?;
    }
    Ok(())
}

pub fn draw_state<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    state: &DisplayState,
) -> Result<(), D::Error> {
    draw_state_with_blink(display, state, false)
}

pub fn draw_state_with_blink<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    state: &DisplayState,
    blink: bool,
) -> Result<(), D::Error> {
    draw_state_with_image(display, state, &[], blink)
}

/// 画像領域の画素 (`image`) を与えて描く。画素は表示状態とは別に持つ (model の ImagePixels)。
/// 描画のたびに表示状態を複製するため、38 KB の画素を複製の対象に含めない。
pub fn draw_state_with_image<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    state: &DisplayState,
    image: &[u8],
    blink: bool,
) -> Result<(), D::Error> {
    display.clear(Rgb565::BLACK)?;
    if let Some(entry) = state.get(Slot::Overlay) {
        draw_content(
            display,
            &entry.content,
            image,
            Rectangle::new(Point::zero(), Size::new(320, 240)),
        )?;
    } else {
        let face = state
            .emote()
            .map(|emote| (emote.expression, emote.gaze, emote.eyes))
            .or_else(|| {
                state
                    .presence()
                    .map(|presence| (presence.expression, presence.gaze, presence.eyes))
            })
            .unwrap_or((Expression::Happy, Gaze::Center, EyeStyle::Auto));
        draw_face(display, face.0, face.1, face.2, blink)?;
        if let Some(presence) = state.presence() {
            draw_presence_status(display, presence)?;
        }
        for (slot, y) in [(Slot::BannerTop, 0), (Slot::BannerBottom, 192)] {
            if let Some(entry) = state.get(slot) {
                draw_content(
                    display,
                    &entry.content,
                    image,
                    Rectangle::new(Point::new(0, y), Size::new(320, 48)),
                )?;
            }
        }
    }
    Ok(())
}

fn draw_presence_status<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    presence: &Presence,
) -> Result<(), D::Error> {
    let activity = presence.activity.map(|value| match value {
        Activity::Idle => "IDLE",
        Activity::Working => "WORKING",
        Activity::Waiting => "WAITING",
        Activity::Done => "DONE",
        Activity::Error => "ERROR",
    });
    if let Some(activity) = activity {
        draw_cell_label(
            display,
            activity,
            Rectangle::new(Point::new(10, 164), Size::new(90, 20)),
        )?;
    }
    if !presence.detail.is_empty() {
        let x = if activity.is_some() { 100 } else { 10 };
        draw_cell_label(
            display,
            &presence.detail,
            Rectangle::new(Point::new(x, 164), Size::new(310 - x as u32, 20)),
        )?;
    }
    Ok(())
}

fn draw_content<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    content: &Content,
    image: &[u8],
    area: Rectangle,
) -> Result<(), D::Error> {
    match content {
        Content::Text(text) => draw_text(display, text, area),
        Content::Card(card) => draw_card(display, card, area),
        Content::Image(region) => draw_region_image(display, image, *region),
    }
}

/// 画像領域を描く。画素が領域に足りない場合は描かない (領域は黒のまま)。
///
/// インライン展開させないのは、`draw_inline_image` と同じく `ImageRaw::new` の除算
/// (画素数 / (幅 × 2)) が条件の判定より前に先行実行されるのを防ぐためである。
#[inline(never)]
fn draw_region_image<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    image: &[u8],
    region: ImageRegion,
) -> Result<(), D::Error> {
    let bytes = usize::from(region.width) * usize::from(region.height) * 2;
    if region.width == 0 || image.len() < bytes {
        return Ok(());
    }
    let raw = ImageRawBE::<Rgb565>::new(&image[..bytes], u32::from(region.width));
    Image::new(&raw, Point::new(i32::from(region.x), i32::from(region.y))).draw(display)
}

fn draw_card<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    card: &Card,
    area: Rectangle,
) -> Result<(), D::Error> {
    let mut y = area.top_left.y + 4;
    for row in &card.rows {
        let width = (area.size.width - 20) / row.elements.len() as u32;
        for (index, element) in row.elements.iter().enumerate() {
            let cell = Rectangle::new(
                Point::new(area.top_left.x + 10 + index as i32 * width as i32, y),
                Size::new(width, u32::from(row.height())),
            );
            let mut clipped = display.clipped(&cell);
            match element {
                Element::Text { text } => draw_cell_label(&mut clipped, text, cell)?,
                Element::Bar { ratio, label } => {
                    let label_width = (width / 3).min(80);
                    draw_cell_label(
                        &mut clipped,
                        label,
                        Rectangle::new(cell.top_left, Size::new(label_width, 20)),
                    )?;
                    let bar_width = width - label_width - 8;
                    let bar = Rectangle::new(
                        Point::new(cell.top_left.x + label_width as i32 + 4, y + 6),
                        Size::new(bar_width, 8),
                    );
                    bar.into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
                        .draw(&mut clipped)?;
                    Rectangle::new(
                        bar.top_left + Point::new(1, 1),
                        Size::new((bar_width - 2) * u32::from(*ratio) / 100, 6),
                    )
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
                    .draw(&mut clipped)?;
                }
                Element::Spacer { .. } => {}
                Element::Image => {
                    if let Some(image) = &card.image {
                        draw_inline_image(&mut clipped, image, cell.top_left + Point::new(0, 2))?;
                    }
                }
            }
        }
        y += i32::from(row.height());
    }
    Ok(())
}

/// Card の画像を描く。
///
/// インライン展開させないのは、`ImageRaw::new` の高さの計算 (画素数 / (幅 × 2)) が
/// `card.image` の判定より前に先行実行されるのを防ぐためである。Xtensa の除算命令は
/// 0 で例外を起こすため、画像を持たない Card の未初期化の幅が 0 だと描画中に停止した
/// (2026-09-26、`quou` による IntegerDivideByZero を逆アセンブルで確認)。
#[inline(never)]
fn draw_inline_image<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    image: &ImageData,
    top_left: Point,
) -> Result<(), D::Error> {
    let raw = ImageRawBE::<Rgb565>::new(&image.pixels, u32::from(image.width));
    Image::new(&raw, top_left).draw(display)
}

fn draw_cell_label<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    text: &str,
    cell: Rectangle,
) -> Result<(), D::Error> {
    let mut clipped = display.clipped(&cell);
    for (column, ch) in text.chars().filter(|ch| !ch.is_control()).enumerate() {
        if column as u32 >= cell.size.width / 10 {
            break;
        }
        draw_glyph(
            &mut clipped,
            ch,
            cell.top_left + Point::new(column as i32 * 10, 0),
        )?;
    }
    Ok(())
}

fn draw_glyph<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    ch: char,
    point: Point,
) -> Result<(), D::Error> {
    let glyph = if ch.is_ascii() { ch as u8 } else { b'?' };
    let bytes = [glyph];
    Text::with_baseline(
        core::str::from_utf8(&bytes).expect("ASCII glyph"),
        point,
        MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE),
        Baseline::Top,
    )
    .draw(display)?;
    Ok(())
}

fn draw_text<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    text: &str,
    area: Rectangle,
) -> Result<(), D::Error> {
    let mut clipped = display.clipped(&area);
    let mut column = 0;
    let mut row = 0;
    // 10 px の左右余白と 4 px の上下余白。帯は 2 行、Overlay は 11 行。
    let rows = (area.size.height - 8) / 20;
    for ch in text.chars() {
        if ch == '\n' {
            row += 1;
            column = 0;
            continue;
        }
        if ch.is_control() {
            continue;
        }
        if column == 30 {
            row += 1;
            column = 0;
        }
        if row >= rows {
            break;
        }
        // 日本語フォント導入前も UTF-8 のバイト単位で文字を分断しない。
        draw_glyph(
            &mut clipped,
            ch,
            area.top_left + Point::new(10 + column * 10, 4 + row as i32 * 20),
        )?;
        column += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{vec, vec::Vec};

    struct Screen(Vec<Rgb565>);

    impl OriginDimensions for Screen {
        fn size(&self) -> Size {
            Size::new(320, 240)
        }
    }

    impl DrawTarget for Screen {
        type Color = Rgb565;
        type Error = ();

        fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(
            &mut self,
            pixels: I,
        ) -> Result<(), Self::Error> {
            for Pixel(point, color) in pixels {
                assert!(
                    self.bounding_box().contains(point),
                    "pixel outside display: {point:?}"
                );
                self.0[point.y as usize * 320 + point.x as usize] = color;
            }
            Ok(())
        }
    }

    #[test]
    fn draws_within_screen_and_preserves_color_bar_order() {
        let mut screen = Screen(vec![Rgb565::MAGENTA; 320 * 240]);
        draw(&mut screen).unwrap();
        for y in 0..240 {
            for x in 0..320 {
                let pixel = screen.0[y * 320 + x];
                assert_ne!(pixel, Rgb565::MAGENTA);
                if x == 0 || x == 319 || y == 0 || y == 239 {
                    assert_eq!(pixel, Rgb565::WHITE);
                }
                if (112..208).contains(&x) && (200..216).contains(&y) {
                    let colors = [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE, Rgb565::WHITE];
                    assert_eq!(pixel, colors[(x - 112) / 24]);
                }
            }
        }
        // 顔と文字列が背景に埋もれていないことを確認する。
        assert_eq!(screen.0[85 * 320 + 103], Rgb565::WHITE);
        assert_eq!(screen.0[85 * 320 + 215], Rgb565::WHITE);
        assert!(
            screen.0[10 * 320..26 * 320]
                .chunks(320)
                .any(|row| row[12..212].contains(&Rgb565::WHITE))
        );
        assert!(
            screen.0[164 * 320..181 * 320]
                .chunks(320)
                .any(|row| row[12..192].contains(&Rgb565::WHITE))
        );
    }

    #[test]
    fn propagates_display_errors() {
        struct BrokenScreen;
        impl OriginDimensions for BrokenScreen {
            fn size(&self) -> Size {
                Size::new(320, 240)
            }
        }
        impl DrawTarget for BrokenScreen {
            type Color = Rgb565;
            type Error = ();
            fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, _: I) -> Result<(), ()> {
                Err(())
            }
        }
        assert_eq!(draw(&mut BrokenScreen), Err(()));
        assert_eq!(
            draw_state(&mut BrokenScreen, &DisplayState::default()),
            Err(())
        );
    }

    #[test]
    fn banners_preserve_face_and_overlay_expiry_restores_banners() {
        use crate::model::Controller;
        use protocol::Message;
        let mut controller = Controller::default();
        let mut screen = Screen(vec![Rgb565::BLACK; 320 * 240]);
        for slot in [Slot::BannerTop, Slot::BannerBottom] {
            controller
                .handle(
                    Message::Text {
                        slot,
                        ttl_s: 0,
                        text: "VISIBLE".try_into().unwrap(),
                    },
                    0,
                    &mut screen,
                )
                .unwrap();
        }
        let banners = screen.0.clone();
        assert_eq!(screen.0[85 * 320 + 103], Rgb565::WHITE);
        assert!(screen.0[..48 * 320].contains(&Rgb565::WHITE));
        assert!(screen.0[192 * 320..].contains(&Rgb565::WHITE));
        controller
            .handle(
                Message::Text {
                    slot: Slot::Overlay,
                    ttl_s: 1,
                    text: "Overlay".try_into().unwrap(),
                },
                0,
                &mut screen,
            )
            .unwrap();
        assert_eq!(screen.0[85 * 320 + 103], Rgb565::BLACK);
        assert!(!screen.0[192 * 320..].contains(&Rgb565::WHITE));
        controller.tick(1000, &mut screen).unwrap();
        assert_eq!(screen.0, banners);
        controller
            .handle(Message::Clear, 1000, &mut screen)
            .unwrap();
        assert_eq!(screen.0[85 * 320 + 103], Rgb565::WHITE);
        assert!(!screen.0[..48 * 320].contains(&Rgb565::WHITE));
        assert!(!screen.0[192 * 320..].contains(&Rgb565::WHITE));
    }

    #[test]
    fn presence_changes_gaze_and_status_then_expires_to_default_face() {
        use crate::model::Controller;
        use protocol::Message;
        let mut controller = Controller::default();
        let mut screen = Screen(vec![Rgb565::BLACK; 320 * 240]);
        controller.handle(Message::Clear, 0, &mut screen).unwrap();
        let default_face = screen.0.clone();
        controller
            .handle(
                Message::Presence(Presence {
                    activity: Some(Activity::Working),
                    detail: "BUILD".try_into().unwrap(),
                    expression: Expression::Focused,
                    gaze: Gaze::Right,
                    eyes: EyeStyle::Auto,
                    ttl_s: 1,
                }),
                0,
                &mut screen,
            )
            .unwrap();
        assert_eq!(screen.0[86 * 320 + 92], Rgb565::BLACK);
        assert_eq!(default_face[86 * 320 + 92], Rgb565::WHITE);
        assert!(screen.0[164 * 320..184 * 320].contains(&Rgb565::WHITE));
        controller.tick(999, &mut screen).unwrap();
        assert_ne!(screen.0, default_face);
        controller.tick(1000, &mut screen).unwrap();
        assert_eq!(screen.0, default_face);
    }

    #[test]
    fn automatic_blink_closes_and_reopens_eyes_without_changing_presence() {
        use crate::model::Controller;
        use protocol::Message;

        let mut controller = Controller::default();
        let mut screen = Screen(vec![Rgb565::BLACK; 320 * 240]);
        controller.handle(Message::Clear, 0, &mut screen).unwrap();
        let open_face = screen.0.clone();
        controller.tick(4_000, &mut screen).unwrap();
        assert_ne!(screen.0, open_face);
        controller.tick(4_120, &mut screen).unwrap();
        assert_eq!(screen.0, open_face);

        controller
            .handle(
                Message::Presence(Presence {
                    activity: None,
                    detail: Default::default(),
                    expression: Expression::Happy,
                    gaze: Gaze::Center,
                    eyes: EyeStyle::Wide,
                    ttl_s: 0,
                }),
                5_000,
                &mut screen,
            )
            .unwrap();
        let explicit_eyes = screen.0.clone();
        controller.tick(9_500, &mut screen).unwrap();
        assert_eq!(screen.0, explicit_eyes);
    }

    #[test]
    fn startup_blink_preserves_diagnostic_screen() {
        use crate::model::Controller;

        let mut controller = Controller::default();
        let mut screen = Screen(vec![Rgb565::BLACK; 320 * 240]);
        draw(&mut screen).unwrap();
        let startup = screen.0.clone();
        controller.tick(4_000, &mut screen).unwrap();
        assert_ne!(screen.0, startup);
        assert_eq!(screen.0[205 * 320 + 115], startup[205 * 320 + 115]);
        controller.tick(4_120, &mut screen).unwrap();
        assert_eq!(screen.0, startup);
    }

    #[test]
    fn emote_overrides_face_then_restores_presence_pixels() {
        use crate::model::Controller;
        use protocol::{Emote, Message};

        let mut controller = Controller::default();
        let mut screen = Screen(vec![Rgb565::BLACK; 320 * 240]);
        controller
            .handle(
                Message::Presence(Presence {
                    activity: None,
                    detail: Default::default(),
                    expression: Expression::Calm,
                    gaze: Gaze::Center,
                    eyes: EyeStyle::Auto,
                    ttl_s: 0,
                }),
                0,
                &mut screen,
            )
            .unwrap();
        let calm = screen.0.clone();
        controller
            .handle(
                Message::Emote(Emote {
                    expression: Expression::Curious,
                    gaze: Gaze::Point { x: -100, y: 50 },
                    eyes: EyeStyle::Wide,
                    intensity: 50,
                    duration_ms: 500,
                }),
                100,
                &mut screen,
            )
            .unwrap();
        assert_ne!(screen.0, calm);
        controller.tick(600, &mut screen).unwrap();
        assert_eq!(screen.0, calm);
    }

    #[test]
    fn eye_styles_change_shape_without_pupils() {
        use crate::model::Controller;
        use protocol::Message;
        let mut controller = Controller::default();
        let mut screen = Screen(vec![Rgb565::BLACK; 320 * 240]);
        let mut frames = vec![];
        for eyes in [
            EyeStyle::Open,
            EyeStyle::Wide,
            EyeStyle::Closed,
            EyeStyle::HalfLidded,
        ] {
            controller
                .handle(
                    Message::Presence(Presence {
                        activity: None,
                        detail: Default::default(),
                        expression: Expression::Happy,
                        gaze: Gaze::Center,
                        eyes,
                        ttl_s: 0,
                    }),
                    0,
                    &mut screen,
                )
                .unwrap();
            frames.push(screen.0.clone());
        }
        for left in 0..frames.len() {
            for right in left + 1..frames.len() {
                assert_ne!(frames[left], frames[right]);
            }
        }
        assert_eq!(frames[0][86 * 320 + 103], Rgb565::WHITE);
        assert_eq!(frames[1][86 * 320 + 103], Rgb565::WHITE);
        assert_eq!(frames[2][80 * 320 + 103], Rgb565::BLACK);
        assert_eq!(frames[3][80 * 320 + 103], Rgb565::BLACK);
        assert_eq!(frames[3][90 * 320 + 103], Rgb565::WHITE);
    }

    #[test]
    fn wrapping_unicode_controls_and_clipping_stay_inside_slot() {
        let area = Rectangle::new(Point::new(0, 192), Size::new(320, 48));
        let mut screen = Screen(vec![Rgb565::BLACK; 320 * 240]);
        let mut expected = Screen(vec![Rgb565::BLACK; 320 * 240]);
        draw_text(&mut screen, "A\t\u{1b}日\rB\nC", area).unwrap();
        draw_text(&mut expected, "A?B\nC", area).unwrap();
        assert_eq!(screen.0, expected.0);
        let long = "W".repeat(protocol::MAX_TEXT_BYTES);
        draw_text(&mut screen, &long, area).unwrap();
        assert!(!screen.0[..192 * 320].contains(&Rgb565::WHITE));
        assert!(screen.0[216 * 320..236 * 320].contains(&Rgb565::WHITE));
        draw_text(
            &mut screen,
            &long,
            Rectangle::new(Point::zero(), Size::new(320, 240)),
        )
        .unwrap();
    }

    #[test]
    fn card_bar_ratio_and_layout_are_rendered_inside_banner() {
        use crate::model::Controller;
        use protocol::{Message, Row};
        let mut rows = heapless::Vec::new();
        rows.push(Row {
            elements: heapless::Vec::from_slice(&[
                Element::Text {
                    text: "CPU".try_into().unwrap(),
                },
                Element::Text {
                    text: "75%".try_into().unwrap(),
                },
            ])
            .unwrap(),
            action: None,
        })
        .unwrap();
        rows.push(Row {
            elements: heapless::Vec::from_slice(&[Element::Bar {
                ratio: 75,
                label: "Usage".try_into().unwrap(),
            }])
            .unwrap(),
            action: None,
        })
        .unwrap();
        let mut controller = Controller::default();
        let mut screen = Screen(vec![Rgb565::BLACK; 320 * 240]);
        controller
            .handle(
                Message::Card(Card {
                    slot: Slot::BannerTop,
                    ttl_s: 0,
                    id: 0,
                    rows,
                    image: None,
                }),
                0,
                &mut screen,
            )
            .unwrap();
        // バーの外枠と、75% の内側が塗られている。右端側は黒のまま。
        assert_eq!(screen.0[33 * 320 + 117], Rgb565::WHITE);
        assert_eq!(screen.0[33 * 320 + 260], Rgb565::BLACK);
        assert!(!screen.0[48 * 320..72 * 320].contains(&Rgb565::WHITE));
    }

    #[test]
    fn inline_rgb565_image_preserves_pixel_colors_and_row_position() {
        use crate::model::Controller;
        use protocol::{Message, Row};
        let mut rows = heapless::Vec::new();
        rows.push(Row {
            elements: heapless::Vec::from_slice(&[
                Element::Text {
                    text: "RGB".try_into().unwrap(),
                },
                Element::Image,
            ])
            .unwrap(),
            action: None,
        })
        .unwrap();
        let card = Card {
            slot: Slot::BannerTop,
            ttl_s: 0,
            id: 0,
            rows,
            image: Some(ImageData {
                width: 2,
                height: 1,
                pixels: heapless::Vec::from_slice(&[0xF8, 0x00, 0x07, 0xE0]).unwrap(),
            }),
        };
        let mut controller = Controller::default();
        let mut screen = Screen(vec![Rgb565::BLACK; 320 * 240]);
        controller
            .handle(Message::Card(card), 0, &mut screen)
            .unwrap();
        assert_eq!(screen.0[6 * 320 + 160], Rgb565::RED);
        assert_eq!(screen.0[6 * 320 + 161], Rgb565::GREEN);
        assert_eq!(screen.0[6 * 320 + 162], Rgb565::BLACK);
    }
}
