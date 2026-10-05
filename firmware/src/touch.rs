//! CoreS3 の FT6336U を内部 I2C から読み、接触をタップと長押しに判定する。

use embedded_hal::i2c::I2c;

const ADDRESS: u8 = 0x38;
const DEVICE_MODE: u8 = 0x00;
const TOUCH_COUNT: u8 = 0x02;
const INTERRUPT_MODE: u8 = 0xa4;

pub fn init<I: I2c>(bus: &mut I) -> Result<(), I::Error> {
    // ポーリングで安定して読めるよう、作業モードと割込みモードを明示する。
    bus.write(ADDRESS, &[DEVICE_MODE, 0])?;
    bus.write(ADDRESS, &[INTERRUPT_MODE, 0])?;
    read_pressed(bus).map(|_| ())
}

pub fn read_pressed<I: I2c>(bus: &mut I) -> Result<bool, I::Error> {
    read_point(bus).map(|point| point.is_some())
}

/// 1 点目の接触位置を読む。接触が無い場合は None。
///
/// 接触点数 (0x02) に続く P1_XH/XL/YH/YL (0x03..0x06) を 1 回の転送で読む。
/// 座標は 12 bit で、上位 4 bit 以外はイベント種別と接触 ID である。
pub fn read_point<I: I2c>(bus: &mut I) -> Result<Option<(u16, u16)>, I::Error> {
    let mut registers = [0; 5];
    bus.write_read(ADDRESS, &[TOUCH_COUNT], &mut registers)?;
    Ok(decode_point(registers))
}

fn decode_point(registers: [u8; 5]) -> Option<(u16, u16)> {
    if !matches!(registers[0] & 0x0f, 1 | 2) {
        return None;
    }
    let x = (u16::from(registers[1] & 0x0f) << 8) | u16::from(registers[2]);
    let y = (u16::from(registers[3] & 0x0f) << 8) | u16::from(registers[4]);
    // 画面外の値は端に丸め、照合の範囲外を生じさせない。
    Some((x.min(319), y.min(239)))
}

/// 長押しと判定する接触の継続時間。
pub const LONG_PRESS_MS: u64 = 600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// 短い接触。位置は接触を始めた点 (指を離す直前の点より安定しているため)。
    Tap { x: u16, y: u16 },
    /// `LONG_PRESS_MS` 以上の接触。接触が続いている間に 1 回だけ通知する。
    LongPress,
}

#[derive(Clone, Copy)]
struct Press {
    x: u16,
    y: u16,
    started_ms: u64,
    long_fired: bool,
}

/// 接触の有無の列から、タップと長押しを判定する。
///
/// タップは指を離した時点で確定する。押した時点で確定すると、長押しの始まりと区別
/// できないためである。
#[derive(Default)]
pub struct TapDetector {
    armed: bool,
    released_polls: u8,
    press: Option<Press>,
}

impl TapDetector {
    pub fn sample(&mut self, point: Option<(u16, u16)>, now_ms: u64) -> Option<Gesture> {
        let Some((x, y)) = point else {
            // 離した状態を 2 回観測してから、接触の終わりとして扱い、次を受け付ける。
            // 1 回だけの欠落は同じ接触の続きとみなす。
            self.released_polls = self.released_polls.saturating_add(1);
            if self.released_polls < 2 {
                return None;
            }
            self.armed = true;
            let press = self.press.take()?;
            return (!press.long_fired).then_some(Gesture::Tap {
                x: press.x,
                y: press.y,
            });
        };
        self.released_polls = 0;
        match &mut self.press {
            Some(press)
                if !press.long_fired
                    && now_ms.saturating_sub(press.started_ms) >= LONG_PRESS_MS =>
            {
                press.long_fired = true;
                Some(Gesture::LongPress)
            }
            Some(_) => None,
            None => {
                if self.armed {
                    self.armed = false;
                    self.press = Some(Press {
                        x,
                        y,
                        started_ms: now_ms,
                        long_fired: false,
                    });
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_registers_are_decoded_and_clamped() {
        assert_eq!(decode_point([0, 0x81, 0x2c, 0x00, 0xf0]), None);
        assert_eq!(
            decode_point([1, 0x81, 0x2c, 0x40, 0xf0]),
            Some((300, 240 - 1))
        );
        assert_eq!(decode_point([1, 0x00, 0x0a, 0x00, 0x14]), Some((10, 20)));
        assert_eq!(decode_point([2, 0x0f, 0xff, 0x0f, 0xff]), Some((319, 239)));
        assert_eq!(decode_point([0x0f, 0, 1, 0, 1]), None);
    }

    /// 20 ms 間隔で観測した接触の列を与え、通知された判定を時刻とともに返す。
    fn run(samples: &[Option<(u16, u16)>]) -> std::vec::Vec<(u64, Gesture)> {
        let mut detector = TapDetector::default();
        let mut gestures = std::vec::Vec::new();
        for (index, point) in samples.iter().enumerate() {
            let now_ms = index as u64 * 20;
            if let Some(gesture) = detector.sample(*point, now_ms) {
                gestures.push((now_ms, gesture));
            }
        }
        gestures
    }

    const DOWN: Option<(u16, u16)> = Some((100, 50));

    #[test]
    fn tap_is_reported_once_after_release_at_the_first_contact_point() {
        // 起動時から触れている接触は、一度離すまで受け付けない。
        let samples = [
            DOWN,
            None,
            None,
            Some((10, 20)),
            Some((12, 24)),
            None,
            // 1 回だけの欠落は同じ接触の続きで、2 回目のタップにしない。
            Some((14, 28)),
            None,
            None,
            None,
            DOWN,
            None,
            None,
        ];
        assert_eq!(
            run(&samples),
            [
                (160, Gesture::Tap { x: 10, y: 20 }),
                (240, Gesture::Tap { x: 100, y: 50 }),
            ]
        );
    }

    #[test]
    fn long_press_is_reported_once_while_held_and_not_followed_by_a_tap() {
        let mut samples = std::vec![None, None];
        // 接触の開始は 40 ms。600 ms 後の 640 ms で長押しになる。
        samples.extend([DOWN; 60]);
        samples.extend([None, None, None]);
        assert_eq!(run(&samples), [(640, Gesture::LongPress)]);
    }

    #[test]
    fn contact_shorter_than_the_threshold_is_a_tap() {
        let mut samples = std::vec![None, None];
        samples.extend([DOWN; 30]); // 40 ms から 620 ms まで (継続 580 ms)
        samples.extend([None, None]);
        assert_eq!(run(&samples), [(660, Gesture::Tap { x: 100, y: 50 })]);
    }
}
