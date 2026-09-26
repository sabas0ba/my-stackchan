//! 画面全体のフレームバッファと、変化した範囲だけの転送。
//!
//! 描画はいったんこのバッファに行い、前回の転送から内容が変わった範囲だけを LCD へ
//! 送る。画面全体を黒で塗ってから描き直す途中経過が表示されず、同じ内容の描き直しでは
//! 転送そのものが起きない。320×240 の RGB565 で 150 KB を内部 SRAM に static に置く
//! (動的確保は行わない)。
//!
//! 変化は書込み時ではなく転送時に検出する。描画処理は画面を黒で塗ってから描き直すため、
//! 書込み時に追跡すると元の色に戻った画素まで変化として扱ってしまう。画面を 16×16 px の
//! タイルに分け、前回転送した内容のハッシュと比べて、変わったタイルを行ごとに横へ
//! まとめて送る。LCD の内容を複製するより少ない RAM (2.4 KB) で済む。
//!
//! ハッシュの計算は画面全体で数十 ms かかる (実機で約 39 ms)。前回の転送から書込みが
//! 無い場合は計算を省く。メインループは周ごとに転送を試みるため、省かなければ受信と
//! 応答がこの時間だけ遅れる。

use core::convert::Infallible;

use embedded_graphics::{
    pixelcolor::{Rgb565, raw::RawU16},
    prelude::*,
    primitives::Rectangle,
};

pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 240;
const TILE: usize = 16;
const TILE_COLUMNS: usize = WIDTH / TILE;
const TILE_ROWS: usize = HEIGHT / TILE;

/// 全フィールドの初期値を 0 とし、static に置いたときに .bss へ配置されるようにする。
/// 0 以外の初期値を持つと、150 KB の初期値が書込みイメージに含まれる。
pub struct FrameBuffer {
    pixels: [u16; WIDTH * HEIGHT],
    /// 前回転送した時点の各タイルのハッシュ。
    flushed: [u64; TILE_COLUMNS * TILE_ROWS],
    /// `flushed` が LCD の内容を表しているか。起動直後と `invalidate` の後は偽。
    synced: bool,
    /// 前回の転送の後に書込みがあったか。
    written: bool,
}

impl Default for FrameBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// FNV-1a (64 bit) の画素 (16 bit) 単位の変形。暗号的な強度は不要で、表示内容の変化を
/// 見落とさない分散があればよい。byte 単位より乗算 (32 bit CPU では複数命令) が半分で済む。
fn tile_hash(pixels: &[u16; WIDTH * HEIGHT], column: usize, row: usize) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for y in row * TILE..(row + 1) * TILE {
        for &pixel in &pixels[y * WIDTH + column * TILE..y * WIDTH + (column + 1) * TILE] {
            hash ^= u64::from(pixel);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

impl FrameBuffer {
    /// 黒 (0x0000) で初期化する。LCD の内容とは同期していない状態で始まり、最初の
    /// `flush` で全体を送る。
    pub const fn new() -> Self {
        Self {
            pixels: [0; WIDTH * HEIGHT],
            flushed: [0; TILE_COLUMNS * TILE_ROWS],
            synced: false,
            written: false,
        }
    }

    /// LCD の内容が不明になった場合 (初期化し直した場合等) に、次回の転送で全体を送る。
    pub fn invalidate(&mut self) {
        self.synced = false;
    }

    /// 前回の転送から内容が変わった範囲を LCD へ送る。
    ///
    /// 失敗した場合は同期していないものとし、次回に全体を送り直す。LCD にどこまで
    /// 書き込まれたかは分からないためである。
    pub fn flush<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        display: &mut D,
    ) -> Result<(), D::Error> {
        if self.synced && !self.written {
            return Ok(());
        }
        let result = self.transfer(display);
        match result {
            Ok(()) => self.written = false,
            Err(_) => self.synced = false,
        }
        result
    }

    fn transfer<D: DrawTarget<Color = Rgb565>>(&mut self, display: &mut D) -> Result<(), D::Error> {
        if !self.synced {
            self.send(display, self.bounding_box())?;
            for row in 0..TILE_ROWS {
                for column in 0..TILE_COLUMNS {
                    self.flushed[row * TILE_COLUMNS + column] =
                        tile_hash(&self.pixels, column, row);
                }
            }
            self.synced = true;
            return Ok(());
        }
        for row in 0..TILE_ROWS {
            let mut changed: Option<(usize, usize)> = None;
            for column in 0..TILE_COLUMNS {
                let hash = tile_hash(&self.pixels, column, row);
                let stored = &mut self.flushed[row * TILE_COLUMNS + column];
                if *stored != hash {
                    *stored = hash;
                    changed = Some(changed.map_or((column, column), |(first, _)| (first, column)));
                }
            }
            if let Some((first, last)) = changed {
                let area = Rectangle::new(
                    Point::new((first * TILE) as i32, (row * TILE) as i32),
                    Size::new(((last - first + 1) * TILE) as u32, TILE as u32),
                );
                self.send(display, area)?;
            }
        }
        Ok(())
    }

    fn send<D: DrawTarget<Color = Rgb565>>(
        &self,
        display: &mut D,
        area: Rectangle,
    ) -> Result<(), D::Error> {
        let pixels = &self.pixels;
        let colors = area.points().map(|point| {
            Rgb565::from(RawU16::new(
                pixels[point.y as usize * WIDTH + point.x as usize],
            ))
        });
        display.fill_contiguous(&area, colors)
    }

    #[cfg(test)]
    pub fn pixel(&self, x: usize, y: usize) -> Rgb565 {
        Rgb565::from(RawU16::new(self.pixels[y * WIDTH + x]))
    }
}

impl OriginDimensions for FrameBuffer {
    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }
}

impl DrawTarget for FrameBuffer {
    type Color = Rgb565;
    type Error = Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Infallible> {
        self.written = true;
        for Pixel(point, color) in pixels {
            // 画面外は描画先で捨てる (embedded-graphics の DrawTarget の約束)。
            if let (Ok(x @ 0..WIDTH), Ok(y @ 0..HEIGHT)) =
                (usize::try_from(point.x), usize::try_from(point.y))
            {
                self.pixels[y * WIDTH + x] = RawU16::from(color).into_inner();
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), Infallible> {
        self.written = true;
        let area = area.intersection(&self.bounding_box());
        let Some(bottom_right) = area.bottom_right() else {
            return Ok(());
        };
        let raw = RawU16::from(color).into_inner();
        for y in area.top_left.y as usize..=bottom_right.y as usize {
            self.pixels[y * WIDTH + area.top_left.x as usize..=y * WIDTH + bottom_right.x as usize]
                .fill(raw);
        }
        Ok(())
    }

    fn clear(&mut self, color: Rgb565) -> Result<(), Infallible> {
        self.written = true;
        self.pixels.fill(RawU16::from(color).into_inner());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Controller;
    use embedded_graphics::primitives::PrimitiveStyle;
    use protocol::{Message, Slot};
    use std::boxed::Box;
    use std::vec::Vec;

    /// 転送された矩形と画素を記録する LCD の代わり。
    #[derive(Default)]
    struct Lcd {
        fills: Vec<(Rectangle, Vec<Rgb565>)>,
        fail: bool,
    }

    impl OriginDimensions for Lcd {
        fn size(&self) -> Size {
            Size::new(WIDTH as u32, HEIGHT as u32)
        }
    }

    impl DrawTarget for Lcd {
        type Color = Rgb565;
        type Error = ();

        fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, _: I) -> Result<(), ()> {
            unreachable!("フレームバッファは矩形単位でのみ転送する")
        }

        fn fill_contiguous<I: IntoIterator<Item = Rgb565>>(
            &mut self,
            area: &Rectangle,
            colors: I,
        ) -> Result<(), ()> {
            if self.fail {
                return Err(());
            }
            self.fills.push((*area, colors.into_iter().collect()));
            Ok(())
        }
    }

    fn synced() -> (Box<FrameBuffer>, Lcd) {
        let mut frame = Box::new(FrameBuffer::new());
        let mut lcd = Lcd::default();
        frame.flush(&mut lcd).unwrap();
        assert_eq!(lcd.fills.len(), 1, "最初の転送は画面全体");
        assert_eq!(lcd.fills[0].0, frame.bounding_box());
        lcd.fills.clear();
        (frame, lcd)
    }

    #[test]
    fn only_changed_tiles_are_transferred_row_by_row() {
        let (mut frame, mut lcd) = synced();
        frame.flush(&mut lcd).unwrap();
        assert!(lcd.fills.is_empty(), "変化が無ければ転送しない");

        // タイル (0, 1) と (2, 1) にまたがる変更と、タイル (19, 14) の変更。
        Rectangle::new(Point::new(10, 20), Size::new(30, 2))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(&mut *frame)
            .unwrap();
        Pixel(Point::new(319, 239), Rgb565::RED)
            .draw(&mut *frame)
            .unwrap();
        frame.flush(&mut lcd).unwrap();
        let areas: Vec<Rectangle> = lcd.fills.iter().map(|(area, _)| *area).collect();
        assert_eq!(
            areas,
            [
                Rectangle::new(Point::new(0, 16), Size::new(48, 16)),
                Rectangle::new(Point::new(304, 224), Size::new(16, 16)),
            ]
        );
        let (_, colors) = &lcd.fills[0];
        assert_eq!(colors[4 * 48 + 10], Rgb565::WHITE, "(10, 20) の画素");
        assert_eq!(colors[0], Rgb565::BLACK);
        assert_eq!(*lcd.fills[1].1.last().unwrap(), Rgb565::RED);
    }

    #[test]
    fn scan_is_skipped_until_the_next_write() {
        let (mut frame, mut lcd) = synced();
        assert!(!frame.written);
        Pixel(Point::new(0, 0), Rgb565::RED)
            .draw(&mut *frame)
            .unwrap();
        assert!(frame.written);
        frame.flush(&mut lcd).unwrap();
        assert!(!frame.written);
        assert_eq!(lcd.fills.len(), 1);
        // 書込みの無い転送は何も送らず、フラグも変えない。
        frame.flush(&mut lcd).unwrap();
        assert_eq!(lcd.fills.len(), 1);
        // 失敗した転送の後は、書込みが無くても全体を送り直す。
        Pixel(Point::new(0, 0), Rgb565::BLUE)
            .draw(&mut *frame)
            .unwrap();
        lcd.fail = true;
        assert!(frame.flush(&mut lcd).is_err());
        lcd.fail = false;
        lcd.fills.clear();
        frame.flush(&mut lcd).unwrap();
        assert_eq!(lcd.fills[0].0, frame.bounding_box());
    }

    #[test]
    fn restoring_the_previous_content_transfers_nothing() {
        let (mut frame, mut lcd) = synced();
        frame.clear(Rgb565::WHITE).unwrap();
        frame.clear(Rgb565::BLACK).unwrap();
        frame.flush(&mut lcd).unwrap();
        assert!(lcd.fills.is_empty());
    }

    #[test]
    fn failed_transfer_resends_everything() {
        let (mut frame, mut lcd) = synced();
        Pixel(Point::new(0, 0), Rgb565::RED)
            .draw(&mut *frame)
            .unwrap();
        lcd.fail = true;
        assert!(frame.flush(&mut lcd).is_err());
        lcd.fail = false;
        frame.flush(&mut lcd).unwrap();
        assert_eq!(lcd.fills.len(), 1);
        assert_eq!(lcd.fills[0].0, frame.bounding_box());
    }

    #[test]
    fn redrawing_the_same_state_transfers_nothing() {
        let (mut frame, mut lcd) = synced();
        let mut controller = Controller::default();
        let text = |content: &str| Message::Text {
            slot: Slot::BannerTop,
            ttl_s: 15,
            text: content.try_into().unwrap(),
        };
        controller.handle(text("12:34"), 0, &mut *frame).unwrap();
        frame.flush(&mut lcd).unwrap();
        let first = lcd.fills.len();
        assert!(first > 0);

        // daemon の送り直し (期限だけが異なる同じ内容) は転送を生じない。
        controller
            .handle(text("12:34"), 1_000, &mut *frame)
            .unwrap();
        frame.flush(&mut lcd).unwrap();
        assert_eq!(lcd.fills.len(), first);

        // 帯の内容が変わった場合は、帯の範囲だけを送る。
        controller
            .handle(text("12:35"), 2_000, &mut *frame)
            .unwrap();
        frame.flush(&mut lcd).unwrap();
        assert!(lcd.fills.len() > first);
        for (area, _) in &lcd.fills[first..] {
            assert!(area.bottom_right().unwrap().y < 48, "{area:?}");
        }
    }
}
