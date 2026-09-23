//! CoreS3 の FT6336U を内部 I2C から読み、押下エッジを一度だけ通知する。

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

#[derive(Default)]
pub struct TapDetector {
    armed: bool,
    released_polls: u8,
}

impl TapDetector {
    pub fn sample(&mut self, pressed: bool) -> bool {
        if pressed {
            self.released_polls = 0;
            if self.armed {
                self.armed = false;
                return true;
            }
        } else {
            // 離した状態を 2 回観測してから再び受け付ける。
            self.released_polls = self.released_polls.saturating_add(1);
            if self.released_polls >= 2 {
                self.armed = true;
            }
        }
        false
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

    #[test]
    fn one_tap_per_press_and_release() {
        let mut detector = TapDetector::default();
        for (pressed, expected) in [
            (true, false),
            (false, false),
            (false, false),
            (true, true),
            (true, false),
            (false, false),
            (true, false),
            (false, false),
            (false, false),
            (true, true),
        ] {
            assert_eq!(detector.sample(pressed), expected);
        }
    }
}
