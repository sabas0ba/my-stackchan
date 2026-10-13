//! Panel のボタンに描くピクトグラム。
//!
//! 表情と同じく画像を持たず、`embedded-graphics` の図形で構成する。大きさは描画先の
//! 正方形 (`area`) に合わせて決め、線の太さは一辺の 1/10 程度とする。色は前景と背景を
//! 受け取り、枠を塗ったボタン (active) では反転して描く。

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Arc, Circle, Ellipse, Line, Polyline, PrimitiveStyle, Rectangle, Triangle},
};
use protocol::Icon;

/// `area` (正方形) にピクトグラムを描く。`Icon::None` では何も描かない。
pub fn draw_icon<D: DrawTarget<Color = Rgb565>>(
    display: &mut D,
    icon: Icon,
    area: Rectangle,
    fg: Rgb565,
    bg: Rgb565,
) -> Result<(), D::Error> {
    let side = area.size.width.min(area.size.height) as i32;
    if side < 8 {
        return Ok(());
    }
    let origin = area.top_left;
    // 0..100 の比率で位置を指す。
    let at = |px: i32, py: i32| origin + Point::new(px * side / 100, py * side / 100);
    let stroke = (side / 10).max(2) as u32;
    let line_style = PrimitiveStyle::with_stroke(fg, stroke);
    let thin_style = PrimitiveStyle::with_stroke(fg, (stroke / 2).max(1));
    let fill_style = PrimitiveStyle::with_fill(fg);
    let bg_fill = PrimitiveStyle::with_fill(bg);
    let line = |display: &mut D, a: (i32, i32), b: (i32, i32)| {
        Line::new(at(a.0, a.1), at(b.0, b.1))
            .into_styled(line_style)
            .draw(display)
    };
    let triangle = |display: &mut D, a: (i32, i32), b: (i32, i32), c: (i32, i32)| {
        Triangle::new(at(a.0, a.1), at(b.0, b.1), at(c.0, c.1))
            .into_styled(fill_style)
            .draw(display)
    };
    let bar = |display: &mut D, a: (i32, i32), b: (i32, i32)| {
        Rectangle::with_corners(at(a.0, a.1), at(b.0, b.1))
            .into_styled(fill_style)
            .draw(display)
    };
    let arrow_right = |display: &mut D, y: i32, from: i32, to: i32| -> Result<(), D::Error> {
        line(display, (from, y), (to - 12, y))?;
        triangle(display, (to - 22, y - 16), (to, y), (to - 22, y + 16))
    };
    let arrow_left = |display: &mut D, y: i32, from: i32, to: i32| -> Result<(), D::Error> {
        line(display, (from, y), (to + 12, y))?;
        triangle(display, (to + 22, y - 16), (to, y), (to + 22, y + 16))
    };
    let circle = |display: &mut D, center: (i32, i32), diameter: i32, style| {
        let diameter = (diameter * side / 100) as u32;
        Circle::with_center(at(center.0, center.1), diameter)
            .into_styled(style)
            .draw(display)
    };
    // 円弧。角度は 3 時の位置を 0 とし、画面上で時計回りに増える (6 時が 90、12 時が 270)。
    let arc =
        |display: &mut D, center: (i32, i32), diameter: i32, start: f32, sweep: f32, style| {
            let diameter = (diameter * side / 100) as u32;
            Arc::with_center(
                at(center.0, center.1),
                diameter,
                Angle::from_degrees(start),
                Angle::from_degrees(sweep),
            )
            .into_styled(style)
            .draw(display)
        };

    match icon {
        Icon::None => {}
        Icon::Play => triangle(display, (22, 10), (88, 50), (22, 90))?,
        Icon::Pause => {
            bar(display, (20, 12), (40, 88))?;
            bar(display, (60, 12), (80, 88))?;
        }
        Icon::Stop => bar(display, (18, 18), (82, 82))?,
        Icon::Next => {
            triangle(display, (12, 12), (68, 50), (12, 88))?;
            bar(display, (72, 12), (88, 88))?;
        }
        Icon::Previous => {
            bar(display, (12, 12), (28, 88))?;
            triangle(display, (88, 12), (32, 50), (88, 88))?;
        }
        Icon::Shuffle => {
            // 交差する 2 本の矢印。
            Polyline::new(&[at(8, 28), at(35, 28), at(65, 72), at(80, 72)])
                .into_styled(line_style)
                .draw(display)?;
            Polyline::new(&[at(8, 72), at(35, 72), at(65, 28), at(80, 28)])
                .into_styled(line_style)
                .draw(display)?;
            triangle(display, (76, 16), (94, 28), (76, 40))?;
            triangle(display, (76, 60), (94, 72), (76, 84))?;
        }
        Icon::Repeat | Icon::RepeatOne => {
            arc(display, (50, 50), 70, 20.0, 300.0, line_style)?;
            // 矢じりを円弧の終端 (3 時のやや上) に置く。
            triangle(display, (72, 20), (96, 36), (72, 52))?;
            if icon == Icon::RepeatOne {
                line(display, (50, 36), (50, 66))?;
                line(display, (40, 44), (50, 36))?;
            }
        }
        Icon::Close => {
            line(display, (20, 20), (80, 80))?;
            line(display, (80, 20), (20, 80))?;
        }
        Icon::Back => arrow_left(display, 50, 88, 10)?,
        Icon::Forward => arrow_right(display, 50, 12, 90)?,
        Icon::First => {
            bar(display, (8, 18), (20, 82))?;
            arrow_left(display, 50, 92, 24)?;
        }
        Icon::Plus => {
            line(display, (50, 14), (50, 86))?;
            line(display, (14, 50), (86, 50))?;
        }
        Icon::Minus => line(display, (14, 50), (86, 50))?,
        Icon::Reset => {
            // 反時計回りの矢印。左上に矢じりを置く。
            arc(display, (50, 52), 66, 250.0, 310.0, line_style)?;
            triangle(display, (42, 6), (42, 36), (18, 22))?;
        }
        Icon::Save => {
            // 受け皿と、そこへ落ちる矢印。
            Polyline::new(&[at(14, 60), at(14, 88), at(86, 88), at(86, 60)])
                .into_styled(line_style)
                .draw(display)?;
            line(display, (50, 10), (50, 56))?;
            triangle(display, (32, 48), (50, 70), (68, 48))?;
        }
        Icon::Gear => {
            circle(display, (50, 50), 52, line_style)?;
            for (dx, dy) in [
                (0, -1),
                (0, 1),
                (-1, 0),
                (1, 0),
                (1, -1),
                (1, 1),
                (-1, -1),
                (-1, 1),
            ] {
                // 歯は中心から外へ向かう短い線。斜めは長さを合わせるため短くする。
                let (len, start) = if dx != 0 && dy != 0 {
                    (26, 20)
                } else {
                    (36, 28)
                };
                line(
                    display,
                    (50 + dx * start, 50 + dy * start),
                    (50 + dx * len, 50 + dy * len),
                )?;
            }
            circle(display, (50, 50), 18, fill_style)?;
        }
        Icon::Timer => {
            circle(display, (50, 54), 70, line_style)?;
            line(display, (50, 54), (50, 30))?;
            line(display, (50, 54), (66, 54))?;
            bar(display, (42, 8), (58, 16))?;
        }
        Icon::Power => {
            // 上に切れ目のある円と、切れ目を通る縦線。
            arc(display, (50, 54), 68, 290.0, 320.0, line_style)?;
            line(display, (50, 10), (50, 50))?;
        }
        Icon::Face => {
            circle(display, (50, 50), 80, line_style)?;
            circle(display, (36, 40), 12, fill_style)?;
            circle(display, (64, 40), 12, fill_style)?;
            arc(display, (50, 52), 40, 20.0, 140.0, thin_style)?;
        }
        Icon::Speech => {
            Rectangle::with_corners(at(10, 14), at(90, 68))
                .into_styled(line_style)
                .draw(display)?;
            triangle(display, (30, 64), (48, 64), (30, 86))?;
            // 吹き出しの枠と尾が重なる部分を背景で消す。
            bar(display, (34, 62), (44, 66))?;
            Rectangle::with_corners(at(34, 64), at(44, 66))
                .into_styled(bg_fill)
                .draw(display)?;
        }
        Icon::Motion => {
            line(display, (26, 50), (74, 50))?;
            triangle(display, (30, 32), (8, 50), (30, 68))?;
            triangle(display, (70, 32), (92, 50), (70, 68))?;
        }
        Icon::Light => {
            // 電球: 丸い部分と口金。
            circle(display, (50, 40), 56, line_style)?;
            bar(display, (38, 68), (62, 76))?;
            bar(display, (42, 80), (58, 88))?;
        }
        Icon::Touch => {
            // 指先と、触れた所から広がる波。
            circle(display, (50, 60), 20, fill_style)?;
            arc(display, (50, 60), 52, 210.0, 120.0, thin_style)?;
            arc(display, (50, 60), 84, 210.0, 120.0, thin_style)?;
        }
        Icon::Sense => {
            Ellipse::with_center(
                at(50, 50),
                Size::new(side as u32 * 84 / 100, side as u32 * 48 / 100),
            )
            .into_styled(line_style)
            .draw(display)?;
            circle(display, (50, 50), 24, fill_style)?;
        }
        Icon::Dark => {
            // 三日月: 円を塗り、ずらした円を背景色で抜く。
            circle(display, (46, 50), 72, fill_style)?;
            circle(display, (62, 42), 64, bg_fill)?;
        }
        Icon::Pace => {
            // 速度計: 上半分の円弧と針。
            arc(display, (50, 62), 84, 180.0, 180.0, line_style)?;
            line(display, (50, 62), (76, 36))?;
            circle(display, (50, 62), 14, fill_style)?;
        }
    }
    Ok(())
}
