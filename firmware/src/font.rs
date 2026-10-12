//! フラッシュ上の固定長字形を二分探索する。RAM にフォントを展開しない。

use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::Rectangle,
    text::{Baseline, Text},
};

const DATA: &[u8] = include_bytes!("../../assets/fonts/japanese.bin");
const RECORD: usize = 37;

fn lookup(ch: char) -> Option<&'static [u8]> {
    let mut low = 0;
    let mut high = DATA.len() / RECORD;
    while low < high {
        let middle = low + (high - low) / 2;
        let record = &DATA[middle * RECORD..(middle + 1) * RECORD];
        let cp = u32::from_le_bytes(record[..4].try_into().unwrap());
        match cp.cmp(&(ch as u32)) {
            core::cmp::Ordering::Less => low = middle + 1,
            core::cmp::Ordering::Greater => high = middle,
            core::cmp::Ordering::Equal => return Some(record),
        }
    }
    None
}

pub fn width(ch: char) -> u32 {
    if ch.is_control() {
        0
    } else if ch.is_ascii() {
        10
    } else {
        lookup(ch).map_or(16, |record| u32::from(record[4]))
    }
}

pub fn text_width(text: &str) -> u32 {
    text.chars().map(width).sum()
}

pub fn glyph<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    ch: char,
    point: Point,
) -> Result<(), D::Error> {
    if ch.is_control() {
        return Ok(());
    }
    if ch.is_ascii() {
        let bytes = [ch as u8];
        Text::with_baseline(
            core::str::from_utf8(&bytes).unwrap(),
            point,
            MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE),
            Baseline::Top,
        )
        .draw(display)?;
    } else {
        let record = lookup(ch)
            .or_else(|| lookup('□'))
            .expect("embedded fallback");
        display.draw_iter((0..16).flat_map(|y| {
            let bits = u16::from_be_bytes([record[5 + y * 2], record[6 + y * 2]]);
            (0..record[4]).filter_map(move |x| {
                (bits & (0x8000 >> x) != 0).then_some(Pixel(
                    point + Point::new(i32::from(x), y as i32 + 2),
                    Rgb565::WHITE,
                ))
            })
        }))?;
    }
    Ok(())
}

fn no_start(ch: char) -> bool {
    "、。，．？！!?)]）］｝〕〉》」』】〙〗〟’”ー〜…ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮヵヶ々ゝゞヽヾ".contains(ch)
}
fn no_end(ch: char) -> bool {
    "([（［｛〔〈《「『【〘〖〝‘“".contains(ch)
}

// 改行候補を UTF-8 境界で保持する。禁則を満たせない極小領域でも必ず前進する。
fn line_end(text: &str, available: u32) -> (usize, usize) {
    let mut used = 0;
    let mut fitted = 0;
    let mut legal = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        if ch == '\n' {
            return (index, index + 1);
        }
        if used + width(ch) > available {
            let end = if legal > 0 {
                legal
            } else if fitted > 0 {
                fitted
            } else {
                index + ch.len_utf8()
            };
            return (end, end);
        }
        used += width(ch);
        fitted = index + ch.len_utf8();
        if !no_end(ch) && chars.peek().is_none_or(|(_, next)| !no_start(*next)) {
            legal = fitted;
        }
    }
    (text.len(), text.len())
}

/// 20 px 行送り。単行表示と最終行では、収まらない文字列を省略記号で示す。
pub fn draw<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    text: &str,
    area: Rectangle,
    wrap: bool,
) -> Result<(), D::Error> {
    if area.size.width == 0 || area.size.height < 20 {
        return Ok(());
    }
    let rows = if wrap { area.size.height / 20 } else { 1 };
    let mut rest = text;
    let mut clipped = display.clipped(&area);
    for row in 0..rows {
        if rest.is_empty() {
            break;
        }
        let (end, consumed) = line_end(rest, area.size.width);
        let truncated = row + 1 == rows && consumed < rest.len();
        let limit = area
            .size
            .width
            .saturating_sub(if truncated { width('…') } else { 0 });
        // 省略記号の予約で折返し位置が変わるため、禁則も残り幅で再評価する。
        let end = if truncated {
            line_end(rest, limit).0
        } else {
            end
        };
        let mut x = 0;
        for ch in rest[..end].chars() {
            if x + width(ch) > limit {
                break;
            }
            glyph(
                &mut clipped,
                ch,
                area.top_left + Point::new(x as i32, row as i32 * 20),
            )?;
            x += width(ch);
        }
        if truncated && area.size.width >= width('…') {
            glyph(
                &mut clipped,
                '…',
                area.top_left + Point::new(x as i32, row as i32 * 20),
            )?;
        }
        rest = &rest[consumed..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn review_truncated_lines_keep_opening_brackets_with_the_following_text() {
        use embedded_graphics::mock_display::MockDisplay;
        for (text, wrap, rows) in [("日（本語", false, 1), ("ABC\n日（本語", true, 2)] {
            let mut actual = MockDisplay::<Rgb565>::new();
            draw(
                &mut actual,
                text,
                Rectangle::new(Point::zero(), Size::new(48, rows * 20)),
                wrap,
            )
            .unwrap();
            let mut expected = MockDisplay::<Rgb565>::new();
            if rows == 2 {
                draw(
                    &mut expected,
                    "ABC",
                    Rectangle::new(Point::zero(), Size::new(48, 20)),
                    false,
                )
                .unwrap();
            }
            let y = (rows as i32 - 1) * 20;
            glyph(&mut expected, '日', Point::new(0, y)).unwrap();
            glyph(&mut expected, '…', Point::new(16, y)).unwrap();
            assert_eq!(actual, expected);
        }
    }
    #[test]
    fn embedded_table_is_sorted_and_contains_japanese() {
        assert_eq!(DATA.len() % RECORD, 0);
        let mut previous = 0;
        for record in DATA.chunks_exact(RECORD) {
            let cp = u32::from_le_bytes(record[..4].try_into().unwrap());
            assert!(cp > previous && char::from_u32(cp).is_some());
            assert!(matches!(record[4], 8 | 16));
            previous = cp;
        }
        for ch in "日本語こんにちはカタカナ。、□…ｱ".chars() {
            assert!(lookup(ch).is_some(), "{ch}");
        }
        assert!(lookup('😀').is_none());
        assert_eq!(width('😀'), 16);
        assert_eq!(text_width("A日本"), 42);
    }
    #[test]
    fn wraps_at_unicode_boundaries_and_observes_punctuation() {
        let text = "あいう、え";
        let (end, _) = line_end(text, 48);
        assert_eq!(&text[..end], "あい");
        let text = "あ「いう";
        let (end, _) = line_end(text, 32);
        assert_eq!(&text[..end], "あ");
        assert_eq!(line_end("日\n本", 32), (3, 4));
        assert_eq!(line_end("日", 1), (3, 3));
        let text = "日本（語）";
        assert_eq!(&text[..line_end(text, 48).0], "日本");
    }

    #[test]
    fn fallback_and_truncation_render_inside_the_requested_rectangle() {
        use embedded_graphics::mock_display::MockDisplay;
        let mut actual = MockDisplay::<Rgb565>::new();
        let area = Rectangle::new(Point::new(0, 0), Size::new(32, 20));
        draw(&mut actual, "日本語", area, false).unwrap();
        let mut expected = MockDisplay::<Rgb565>::new();
        glyph(&mut expected, '日', Point::zero()).unwrap();
        glyph(&mut expected, '…', Point::new(16, 0)).unwrap();
        assert_eq!(actual, expected);
        let mut actual = MockDisplay::<Rgb565>::new();
        draw(&mut actual, "😀", area, false).unwrap();
        let mut expected = MockDisplay::<Rgb565>::new();
        glyph(&mut expected, '□', Point::zero()).unwrap();
        assert_eq!(actual, expected);
        assert!(actual.affected_area().size.width <= 32);
    }
}
