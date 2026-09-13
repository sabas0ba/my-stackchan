//! Phase 1 の受入確認画面。フレームバッファを持たず、描画先へ直接出力する。

use embedded_graphics::{
    image::{Image, ImageRawBE},
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle},
    text::Text,
};

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
    }
}
