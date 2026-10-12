//! LTR-553ALS-WA の近接・明暗。物理距離や照度への換算は行わない。
use embedded_hal::i2c::I2c;

const ADDRESS: u8 = 0x23;

pub fn init<I: I2c>(bus: &mut I) -> Result<bool, I::Error> {
    let mut id = [0; 2];
    bus.write_read(ADDRESS, &[0x86], &mut id)?;
    if id != [0x92, 0x05] {
        return Ok(false);
    }
    // 再起動前の設定にも依存しないよう、停止してから測定条件を指定する。
    for pair in [
        [0x80, 0],
        [0x81, 0],
        [0x8f, 0],
        [0x82, 0x7b], // 60 kHz / 100% / 50 mA
        [0x83, 4],
        [0x84, 2],
        [0x85, 1],
        [0x80, 1],
        [0x81, 0x22],
    ] {
        bus.write(ADDRESS, &pair)?;
    }
    Ok(true)
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct Sample {
    pub proximity: Option<u16>,
    pub ambient: Option<u16>,
}

pub fn read<I: I2c>(bus: &mut I) -> Result<Sample, I::Error> {
    let mut status = [0];
    bus.write_read(ADDRESS, &[0x8c], &mut status)?;
    let mut sample = Sample::default();
    if status[0] & 4 != 0 {
        let mut data = [0; 4];
        // CH1→CH0 の順に読むことで、測定更新を跨いだ値の混在を防ぐ。
        bus.write_read(ADDRESS, &[0x88], &mut data)?;
        if status[0] & 0xf0 == 0 {
            sample.ambient = Some(u16::from_le_bytes([data[2], data[3]]));
        }
    }
    if status[0] & 1 != 0 {
        let mut data = [0; 2];
        bus.write_read(ADDRESS, &[0x8d], &mut data)?;
        if data[1] & 0x80 == 0 {
            sample.proximity = Some(u16::from_le_bytes([data[0], data[1] & 7]));
        }
    }
    Ok(sample)
}

#[derive(Default)]
struct Hysteresis {
    active: bool,
    pending: u8,
}

impl Hysteresis {
    fn update(&mut self, enter: bool, leave: bool) {
        if if self.active { leave } else { enter } {
            self.pending += 1;
            if self.pending == 3 {
                self.active = !self.active;
                self.pending = 0;
            }
        } else {
            self.pending = 0;
        }
    }
}

#[derive(Default)]
pub struct Filter {
    near: Hysteresis,
    dark: Hysteresis,
}

impl Filter {
    pub fn update(&mut self, sample: Sample) -> (bool, bool) {
        // 境界付近のノイズで反応を連発しない。閾値は実機で調整する生値。
        if let Some(value) = sample.proximity {
            self.near.update(value >= 250, value <= 150);
        }
        if let Some(value) = sample.ambient {
            self.dark.update(value <= 5, value >= 15);
        }
        (self.near.active, self.dark.active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_requires_three_fresh_samples_and_separate_exit_thresholds() {
        let mut filter = Filter::default();
        for _ in 0..2 {
            assert_eq!(
                filter.update(Sample {
                    proximity: Some(300),
                    ambient: Some(2)
                }),
                (false, false)
            );
        }
        assert_eq!(filter.update(Sample::default()), (false, false));
        assert_eq!(
            filter.update(Sample {
                proximity: Some(300),
                ambient: Some(2)
            }),
            (true, true)
        );
        for _ in 0..10 {
            assert_eq!(
                filter.update(Sample {
                    proximity: Some(200),
                    ambient: Some(10)
                }),
                (true, true)
            );
        }
        for _ in 0..3 {
            filter.update(Sample {
                proximity: Some(100),
                ambient: Some(20),
            });
        }
        assert_eq!(filter.update(Sample::default()), (false, false));
    }
}
