//! 表示用のレジスタだけを更新し、他の電源と GPIO 設定を保持する。

use embedded_hal::{delay::DelayNs, i2c::I2c};

const AXP2101: u8 = 0x34;
const AW9523: u8 = 0x58;
const LCD_RESET: u8 = 1 << 1;
const BACKLIGHT_ENABLE: u8 = 1 << 7;

fn update<I: I2c>(
    bus: &mut I,
    address: u8,
    register: u8,
    mask: u8,
    value: u8,
) -> Result<(), I::Error> {
    let mut data = [0];
    bus.write_read(address, &[register], &mut data)?;
    bus.write(address, &[register, (data[0] & !mask) | (value & mask)])
}

pub fn prepare_display<I: I2c>(bus: &mut I, delay: &mut impl DelayNs) -> Result<(), I::Error> {
    // 描画完了まで消灯し、リセット前の VRAM が見えることを避ける。
    set_backlight(bus, false)?;
    // DLDO1 は 0.5 V + 100 mV/step。2.8 V は公式の輝度設定範囲内。
    update(bus, AXP2101, 0x99, 0x1f, 23)?;
    // 出力ラッチを先に Low とし、GPIO mode、出力方向の順に設定する。
    update(bus, AW9523, 0x03, LCD_RESET, 0)?;
    update(bus, AW9523, 0x13, LCD_RESET, LCD_RESET)?;
    update(bus, AW9523, 0x05, LCD_RESET, 0)?;
    delay.delay_ms(20);
    update(bus, AW9523, 0x03, LCD_RESET, LCD_RESET)?;
    delay.delay_ms(120);
    Ok(())
}

pub fn set_backlight<I: I2c>(bus: &mut I, enabled: bool) -> Result<(), I::Error> {
    update(
        bus,
        AXP2101,
        0x90,
        BACKLIGHT_ENABLE,
        if enabled { BACKLIGHT_ENABLE } else { 0 },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal::i2c::{ErrorKind, ErrorType, Operation};
    use std::vec::Vec;

    struct Bus {
        registers: [[u8; 256]; 2],
        calls: usize,
        fail_at: Option<usize>,
    }

    impl Bus {
        fn new(fill: u8) -> Self {
            Self {
                registers: [[fill; 256]; 2],
                calls: 0,
                fail_at: None,
            }
        }
    }

    impl ErrorType for Bus {
        type Error = ErrorKind;
    }

    impl I2c for Bus {
        fn transaction(
            &mut self,
            address: u8,
            operations: &mut [Operation<'_>],
        ) -> Result<(), Self::Error> {
            let call = self.calls;
            self.calls += 1;
            if self.fail_at == Some(call) {
                return Err(ErrorKind::Other);
            }
            let registers = &mut self.registers[match address {
                0x34 => 0,
                0x58 => 1,
                _ => panic!("unexpected address"),
            }];
            match operations {
                [Operation::Write(request), Operation::Read(response)] => {
                    assert_eq!(request.len(), 1);
                    assert_eq!(response.len(), 1);
                    response[0] = registers[request[0] as usize];
                }
                [Operation::Write(data)] => {
                    assert_eq!(data.len(), 2);
                    registers[data[0] as usize] = data[1];
                }
                _ => panic!("unexpected transaction"),
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct Delay(Vec<u32>);

    impl DelayNs for Delay {
        fn delay_ns(&mut self, ns: u32) {
            self.0.push(ns);
        }
    }

    #[test]
    fn preserves_every_unrelated_register_bit() {
        for initial in 0..=255 {
            let mut bus = Bus::new(initial);
            let mut expected = bus.registers;
            expected[0][0x90] &= 0x7f;
            expected[0][0x99] = (initial & 0xe0) | 23;
            expected[1][0x03] |= 2;
            expected[1][0x13] |= 2;
            expected[1][0x05] &= !2;
            let mut delay = Delay::default();
            prepare_display(&mut bus, &mut delay).unwrap();
            assert_eq!(bus.registers, expected);
            assert_eq!(delay.0, [20_000_000, 120_000_000]);
        }
    }

    #[test]
    fn stops_at_each_failed_i2c_transaction() {
        for failure in 0..12 {
            let mut bus = Bus::new(0);
            bus.fail_at = Some(failure);
            assert_eq!(
                prepare_display(&mut bus, &mut Delay::default()),
                Err(ErrorKind::Other)
            );
            assert_eq!(bus.calls, failure + 1);
            assert_eq!(bus.registers[0][0x90] & 0x80, 0);
        }
    }

    #[test]
    fn backlight_toggle_preserves_other_rails() {
        for initial in 0..=255 {
            let mut bus = Bus::new(initial);
            set_backlight(&mut bus, true).unwrap();
            assert_eq!(bus.registers[0][0x90], initial | 0x80);
            set_backlight(&mut bus, false).unwrap();
            assert_eq!(bus.registers[0][0x90], initial & 0x7f);
        }
    }
}
