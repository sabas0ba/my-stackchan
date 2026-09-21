//! 起動確認画面と Slot の描画。フレームバッファを持たず描画先へ直接出力する。

use crate::model::DisplayState;
use embedded_graphics::{
    image::{Image, ImageRawBE},
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use protocol::Slot;

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

    draw_face(display)?;

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

fn draw_face<D: DrawTarget<Color = Rgb565>>(display: &mut D) -> Result<(), D::Error> {
    let face = PrimitiveStyle::with_fill(Rgb565::WHITE);
    for x in [90, 202] {
        Circle::new(Point::new(x, 72), 28)
            .into_styled(face)
            .draw(display)?;
    }
    for (start, end) in [
        (Point::new(130, 126), Point::new(145, 140)),
        (Point::new(145, 140), Point::new(175, 140)),
        (Point::new(175, 140), Point::new(190, 126)),
    ] {
        Line::new(start, end)
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 4))
            .draw(display)?;
    }

    Ok(())
}

pub fn draw_state<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    state: &DisplayState,
) -> Result<(), D::Error> {
    display.clear(Rgb565::BLACK)?;
    if let Some(entry) = state.get(Slot::Overlay) {
        draw_text(
            display,
            &entry.text,
            Rectangle::new(Point::zero(), Size::new(320, 240)),
        )?;
    } else {
        draw_face(display)?;
        for (slot, y) in [(Slot::BannerTop, 0), (Slot::BannerBottom, 192)] {
            if let Some(entry) = state.get(slot) {
                draw_text(
                    display,
                    &entry.text,
                    Rectangle::new(Point::new(0, y), Size::new(320, 48)),
                )?;
            }
        }
    }
    Ok(())
}

fn draw_text<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    text: &str,
    area: Rectangle,
) -> Result<(), D::Error> {
    let style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
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
        let glyph = if ch.is_ascii() { ch as u8 } else { b'?' };
        let bytes = [glyph];
        let glyph = core::str::from_utf8(&bytes).expect("ASCII glyph");
        Text::with_baseline(
            glyph,
            area.top_left + Point::new(10 + column * 10, 4 + row as i32 * 20),
            style,
            Baseline::Top,
        )
        .draw(&mut clipped)?;
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
}
