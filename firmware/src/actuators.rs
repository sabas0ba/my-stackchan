//! M5 StackChan K151 の PY32 LED と SCSCL サーボへの通信データ。
//!
//! 参照: m5stack/StackChan-BSP 8d4d6fc3b7a6be379c6317c45a02a30bff8c492e。
//! 永続レジスタ、サーボ ID、角度リミット、校正値は書き換えない。

use embedded_hal::i2c::I2c;

use crate::behavior::ActuatorTarget;

const EXPANDER: u8 = 0x6f;
const VERSION: u8 = 0x02;
const GPIO_MODE_LOW: u8 = 0x03;
const GPIO_MODE_HIGH: u8 = 0x04;
const GPIO_OUT_LOW: u8 = 0x05;
const GPIO_PULL_UP_LOW: u8 = 0x09;
const GPIO_PULL_UP_HIGH: u8 = 0x0a;
const GPIO_DRIVE_HIGH: u8 = 0x14;
const LED_CONFIG: u8 = 0x24;
const LED_RAM: u8 = 0x30;

pub const PRIMARY_ADDRESS: u8 = EXPANDER;
pub const ALTERNATE_ADDRESS: u8 = 0x71;

pub fn probe_version<I: I2c>(bus: &mut I, address: u8) -> Option<u8> {
    let mut value = [0];
    bus.write_read(address, &[VERSION], &mut value).ok()?;
    (value[0] != 0 && value[0] != 0xff).then_some(value[0])
}

pub fn probe_register<I: I2c>(bus: &mut I, address: u8, register: u8) -> Option<u8> {
    let mut value = [0];
    bus.write_read(address, &[register], &mut value).ok()?;
    Some(value[0])
}

pub const VM_OUT_REGISTER: u8 = GPIO_OUT_LOW;
pub const LED_CONFIG_REGISTER: u8 = LED_CONFIG;

fn read<I: I2c>(bus: &mut I, register: u8) -> Result<u8, I::Error> {
    let mut value = [0];
    bus.write_read(EXPANDER, &[register], &mut value)?;
    Ok(value[0])
}

fn update<I: I2c>(bus: &mut I, register: u8, mask: u8, value: u8) -> Result<(), I::Error> {
    let previous = read(bus, register)?;
    bus.write(EXPANDER, &[register, (previous & !mask) | (value & mask)])
}

/// モジュール不在なら false。失敗時はサーボ電源を有効化しない。
pub fn prepare<I: I2c>(bus: &mut I) -> Result<bool, I::Error> {
    let version = read(bus, VERSION)?;
    if version == 0 || version == 0xff {
        return Ok(false);
    }
    // VM_EN の出力ラッチを Low にしてから出力モードへ切り替える。
    update(bus, GPIO_OUT_LOW, 0x01, 0)?;
    update(bus, GPIO_MODE_LOW, 0x01, 0x01)?;
    update(bus, GPIO_PULL_UP_LOW, 0x01, 0x01)?;
    // IO14 は拡張器上の pin 13 (High バイト bit 5)。
    update(bus, GPIO_MODE_HIGH, 0x20, 0x20)?;
    update(bus, GPIO_PULL_UP_HIGH, 0x20, 0x20)?;
    update(bus, GPIO_DRIVE_HIGH, 0x20, 0)?;
    update(bus, LED_CONFIG, 0x3f, 12)?;
    show_rgb(bus, [0, 0, 0])?;
    update(bus, GPIO_OUT_LOW, 0x01, 0x01)?;
    Ok(true)
}

pub fn show_rgb<I: I2c>(bus: &mut I, rgb: [u8; 3]) -> Result<(), I::Error> {
    let color =
        (u16::from(rgb[0] & 0xf8) << 8) | (u16::from(rgb[1] & 0xfc) << 3) | u16::from(rgb[2] >> 3);
    let [high, low] = color.to_be_bytes();
    let mut data = [0; 25];
    data[0] = LED_RAM;
    for led in data[1..].chunks_exact_mut(2) {
        led.copy_from_slice(&[low, high]);
    }
    bus.write(EXPANDER, &data)?;
    update(bus, LED_CONFIG, 0x40, 0x40)
}

pub fn disable<I: I2c>(bus: &mut I) -> Result<(), I::Error> {
    update(bus, GPIO_OUT_LOW, 0x01, 0)?;
    show_rgb(bus, [0, 0, 0])
}

/// 位置指令のみ。X/Y とも SRAM の 0x2A..0x2F に書き、EEPROM には触れない。
pub fn servo_packet(id: u8, position: u16) -> [u8; 13] {
    assert!((1..=2).contains(&id));
    assert!(position <= 1000);
    let [position_high, position_low] = position.to_be_bytes();
    // 公式 BSP の WritePos と同じ位置/時間/速度の構造。50 は穏やかな移動時間。
    let mut packet = [
        0xff,
        0xff,
        id,
        9,
        3,
        42,
        position_high,
        position_low,
        0,
        50,
        0,
        0,
        0,
    ];
    packet[12] = !packet[2..12]
        .iter()
        .fold(0u8, |sum, value| sum.wrapping_add(*value));
    packet
}

pub fn servo_read_packet(id: u8, register: u8, length: u8) -> [u8; 8] {
    assert!((1..=2).contains(&id));
    assert!((1..=2).contains(&length));
    let mut packet = [0xff, 0xff, id, 4, 2, register, length, 0];
    packet[7] = !packet[2..7]
        .iter()
        .fold(0u8, |sum, value| sum.wrapping_add(*value));
    packet
}

pub fn servo_torque_packet(id: u8, enabled: bool) -> [u8; 8] {
    assert!((1..=2).contains(&id));
    let mut packet = [0xff, 0xff, id, 4, 3, 40, u8::from(enabled), 0];
    packet[7] = !packet[2..7]
        .iter()
        .fold(0u8, |sum, value| sum.wrapping_add(*value));
    packet
}

/// 一度の指令で 8 ステップ (約 2.5 度) までに制限する。
pub fn step_position(current: u16, target: u16) -> u16 {
    if current < target {
        current.saturating_add(8).min(target)
    } else {
        current.saturating_sub(8).max(target)
    }
}

/// SCSCL 応答の ID、長さ、エラー、チェックサムを検査する。
pub fn servo_read_value(id: u8, length: u8, bytes: &[u8]) -> Option<u16> {
    let total = usize::from(length) + 6;
    for frame in bytes.windows(total) {
        if frame[0..2] != [0xff, 0xff] || frame[2] != id || frame[3] != length + 2 || frame[4] != 0
        {
            continue;
        }
        let checksum = !frame[2..total - 1]
            .iter()
            .fold(0u8, |sum, value| sum.wrapping_add(*value));
        if checksum != frame[total - 1] {
            continue;
        }
        return Some(if length == 1 {
            u16::from(frame[5])
        } else {
            u16::from_be_bytes([frame[5], frame[6]])
        });
    }
    None
}

/// 工場既定の校正値を基準に、目標を安全な生位置へ写像する。
pub fn servo_positions(target: ActuatorTarget, pitch_trim_raw_steps: i16) -> (u16, u16) {
    assert!((-48..=48).contains(&pitch_trim_raw_steps));
    let yaw = 460 + i32::from(target.x_tenth_deg) * 16 / 50;
    let pitch = 620 + i32::from(target.y_tenth_deg) * 16 / 50 + i32::from(pitch_trim_raw_steps);
    // 補正込みでも Y は約 15..75 度に収め、機構の 5..85 度の推奨域に余裕を残す。
    (yaw.clamp(364, 556) as u16, pitch.clamp(668, 860) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal::i2c::{ErrorKind, ErrorType, Operation};
    use std::vec::Vec;

    struct Bus {
        registers: [u8; 256],
        calls: Vec<Vec<u8>>,
    }

    impl Bus {
        fn new() -> Self {
            let mut registers = [0xa0; 256];
            registers[VERSION as usize] = 1;
            Self {
                registers,
                calls: Vec::new(),
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
            assert_eq!(address, EXPANDER);
            match operations {
                [Operation::Write(request), Operation::Read(response)] => {
                    assert_eq!(request.len(), 1);
                    response[0] = self.registers[request[0] as usize];
                }
                [Operation::Write(data)] => {
                    self.calls.push(data.to_vec());
                    if data.len() == 2 {
                        self.registers[data[0] as usize] = data[1];
                    }
                }
                _ => panic!("unexpected I2C transaction"),
            }
            Ok(())
        }
    }

    #[test]
    fn servo_frames_use_sram_goal_register_and_checksum() {
        assert_eq!(
            servo_packet(1, 460),
            [0xff, 0xff, 1, 9, 3, 42, 1, 204, 0, 50, 0, 0, 201]
        );
        assert_eq!(servo_positions(ActuatorTarget::NEUTRAL, 0), (460, 764));
        assert_eq!(servo_positions(ActuatorTarget::NEUTRAL, -24), (460, 740));
        for x in [-300, 0, 300] {
            for y in [300, 450, 600] {
                let (yaw, pitch) = servo_positions(
                    ActuatorTarget {
                        x_tenth_deg: x,
                        y_tenth_deg: y,
                        rgb: [0; 3],
                    },
                    0,
                );
                assert!((364..=556).contains(&yaw));
                assert!((716..=812).contains(&pitch));
            }
        }
    }

    #[test]
    fn servo_read_frames_validate_reply_and_reject_corruption() {
        assert_eq!(servo_read_packet(1, 40, 1), [255, 255, 1, 4, 2, 40, 1, 207]);
        let reply = [255, 255, 1, 3, 0, 1, 250];
        assert_eq!(servo_read_value(1, 1, &reply), Some(1));
        assert_eq!(servo_read_value(2, 1, &reply), None);
        assert_eq!(servo_read_value(1, 1, &[255, 255, 1, 3, 0, 1, 0]), None);
        assert_eq!(
            servo_read_value(2, 2, &[0, 255, 255, 2, 4, 0, 2, 108, 139]),
            Some(620)
        );
    }

    #[test]
    fn servo_torque_is_volatile_and_steps_are_bounded() {
        assert_eq!(
            servo_torque_packet(1, true),
            [255, 255, 1, 4, 3, 40, 1, 206]
        );
        assert_eq!(
            servo_torque_packet(2, false),
            [255, 255, 2, 4, 3, 40, 0, 206]
        );
        assert_eq!(step_position(653, 764), 661);
        assert_eq!(step_position(760, 764), 764);
        assert_eq!(step_position(764, 653), 756);
        assert_eq!(step_position(764, 760), 760);
    }

    #[test]
    fn led_setup_preserves_unrelated_bits_and_uses_rgb565_ram() {
        let mut bus = Bus::new();
        assert_eq!(prepare(&mut bus), Ok(true));
        assert_eq!(bus.registers[GPIO_MODE_LOW as usize], 0xa1);
        assert_eq!(bus.registers[GPIO_MODE_HIGH as usize], 0xa0);
        assert_eq!(bus.registers[GPIO_OUT_LOW as usize], 0xa1);
        assert_eq!(bus.registers[LED_CONFIG as usize] & 0x3f, 12);
        show_rgb(&mut bus, [255, 0, 0]).unwrap();
        let led_write = bus
            .calls
            .iter()
            .rev()
            .find(|data| data.len() == 25)
            .unwrap();
        assert_eq!(led_write[0], LED_RAM);
        assert!(led_write[1..].chunks_exact(2).all(|led| led == [0, 0xf8]));
    }

    #[test]
    fn absent_expander_does_not_enable_servo_power() {
        let mut bus = Bus::new();
        bus.registers[VERSION as usize] = 0xff;
        assert_eq!(prepare(&mut bus), Ok(false));
        assert!(bus.calls.is_empty());
    }
}
