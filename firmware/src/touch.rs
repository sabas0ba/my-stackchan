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
    let mut count = [0];
    bus.write_read(ADDRESS, &[TOUCH_COUNT], &mut count)?;
    Ok(matches!(count[0] & 0x0f, 1 | 2))
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
