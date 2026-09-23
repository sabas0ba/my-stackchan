//! CoreS3 向け firmware。
//!
//! 起動時に表示確認画面を描画し、USB 経由の表示更新と Ping に応答する。

#![no_std]
#![no_main]

use core::cell::RefCell;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::main;
use esp_hal::time::{Duration, Instant};
use esp_hal::uart::{Config as UartConfig, Uart};
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_hal::{
    gpio::{Flex, Level, Output, OutputConfig},
    i2c::master::{Config as I2cConfig, I2c},
    spi::master::{Config as SpiConfig, Spi},
    time::Rate,
};
use mipidsi::{
    Builder,
    interface::SpiInterface,
    models::ILI9342CRgb565,
    options::{ColorInversion, ColorOrder},
};

mod board;
use my_stackchan_firmware::{
    actuators, model::Controller, power, renderer, touch, transport::Receiver,
};
use static_cell::StaticCell;

static RECEIVER: StaticCell<Receiver> = StaticCell::new();
static CONTROLLER: StaticCell<Controller> = StaticCell::new();

fn send_servo(
    uart: &mut Uart<'_, esp_hal::Blocking>,
    packet: &[u8],
) -> Result<(), esp_hal::uart::TxError> {
    let mut remaining = packet;
    while !remaining.is_empty() {
        remaining = &remaining[uart.write(remaining)?..];
    }
    uart.flush()
}

fn read_servo_register(
    uart: &mut Uart<'_, esp_hal::Blocking>,
    id: u8,
    register: u8,
    length: u8,
) -> Option<u16> {
    for _ in 0..2 {
        if let Some(value) = read_servo_register_once(uart, id, register, length) {
            return Some(value);
        }
    }
    None
}

fn read_servo_register_once(
    uart: &mut Uart<'_, esp_hal::Blocking>,
    id: u8,
    register: u8,
    length: u8,
) -> Option<u16> {
    // 前の位置指令に対する ACK を捨て、今回の読み取り応答だけを検査する。
    let mut stale = [0; 32];
    for _ in 0..4 {
        if uart.read_buffered(&mut stale).ok()? == 0 {
            break;
        }
    }
    send_servo(uart, &actuators::servo_read_packet(id, register, length)).ok()?;
    let start_ms = Instant::now().duration_since_epoch().as_millis();
    let mut response = [0; 24];
    let mut received = 0;
    while Instant::now()
        .duration_since_epoch()
        .as_millis()
        .saturating_sub(start_ms)
        < 30
    {
        let count = uart.read_buffered(&mut response[received..]).ok()?;
        received += count;
        if let Some(value) = actuators::servo_read_value(id, length, &response[..received]) {
            return Some(value);
        }
        if received == response.len() {
            break;
        }
    }
    None
}

// 壁時計を含めず、同じソースから同じ記述子を生成する。
esp_bootloader_esp_idf::esp_app_desc!(
    env!("CARGO_PKG_VERSION"),
    env!("CARGO_PKG_NAME"),
    "00:00:00",
    "1970-01-01",
    esp_bootloader_esp_idf::ESP_IDF_COMPATIBLE_VERSION,
    esp_bootloader_esp_idf::MMU_PAGE_SIZE,
    0,
    u16::MAX,
    esp_bootloader_esp_idf::SECURE_VERSION
);

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let mut delay = Delay::new();

    esp_println::println!("my-stackchan firmware: protocol v{}", protocol::VERSION);

    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("I2C configuration")
    .with_scl(peripherals.GPIO11)
    .with_sda(peripherals.GPIO12);
    power::prepare_display(&mut i2c, &mut delay).expect("display power/reset");

    // microSD を選択しない。カードの使用には別途バスの仲裁と初期化が必要。
    let _sd_cs = Output::new(peripherals.GPIO4, Level::High, OutputConfig::default());
    let dc = RefCell::new(Flex::new(peripherals.GPIO35));
    let cs = board::CsPin {
        pin: Output::new(peripherals.GPIO3, Level::High, OutputConfig::default()),
        dc: &dc,
    };
    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default().with_frequency(Rate::from_mhz(40)),
    )
    .expect("SPI configuration")
    .with_sck(peripherals.GPIO36)
    .with_mosi(peripherals.GPIO37);
    let device = ExclusiveDevice::new(spi, cs, Delay::new()).expect("LCD chip select");
    let mut buffer = [0u8; 512];
    let interface = SpiInterface::new(device, board::DcPin(&dc), &mut buffer);
    let mut display = Builder::new(ILI9342CRgb565, interface)
        .color_order(ColorOrder::Bgr)
        .invert_colors(ColorInversion::Inverted)
        .init(&mut delay)
        .expect("ILI9342C initialization");
    renderer::draw(&mut display).expect("display drawing");
    power::set_backlight(&mut i2c, true).expect("display backlight");
    esp_println::println!("Phase 1 display ready: face / ASCII / RGB565");

    let mut touch_enabled = power::prepare_touch(&mut i2c, &mut delay)
        .and_then(|_| touch::init(&mut i2c))
        .is_ok();
    if !touch_enabled {
        esp_println::println!("touch initialization failed; USB display remains active");
    }
    let mut tap_detector = touch::TapDetector::default();
    let mut next_touch_poll_ms = 0u64;

    let mut actuator_ready = false;
    if power::prepare_body_power(&mut i2c, &mut delay).is_ok() {
        for _ in 0..6 {
            match actuators::prepare(&mut i2c) {
                Ok(true) => {
                    actuator_ready = true;
                    break;
                }
                Ok(false) | Err(_) => delay.delay(Duration::from_millis(200)),
            }
        }
    }
    if !actuator_ready {
        esp_println::println!("StackChan body not detected; servo/LED disabled");
    }
    let mut servo_uart = Uart::new(
        peripherals.UART1,
        UartConfig::default().with_baudrate(1_000_000),
    )
    .expect("servo UART configuration")
    .with_tx(peripherals.GPIO6)
    .with_rx(peripherals.GPIO7);
    if actuator_ready
        && (send_servo(&mut servo_uart, &actuators::servo_torque_packet(1, false)).is_err()
            || send_servo(&mut servo_uart, &actuators::servo_torque_packet(2, false)).is_err())
    {
        actuator_ready = false;
    }
    let mut last_target =
        actuators::servo_positions(my_stackchan_firmware::behavior::ActuatorTarget::NEUTRAL);
    let mut last_rgb = [0u8; 3];
    let mut last_servo_write_ms = None;
    let mut servo_position_known = false;
    let mut torque_enabled = false;
    let mut next_position_probe_ms = 0u64;
    let mut next_idle_torque_off_ms = 0u64;

    let mut usb = UsbSerialJtag::new(peripherals.USB_DEVICE);
    // Card を含む表示状態と受信バッファは大きいため、main のスタックから分離する。
    let receiver = RECEIVER.init_with(Receiver::default);
    let controller = CONTROLLER.init_with(Controller::default);
    let mut tx = [0u8; 64];
    let mut tx_len = 0;
    let mut tx_sent = 0;
    let mut tx_started_ms = 0u64;
    loop {
        let now_ms = Instant::now().duration_since_epoch().as_millis();
        // 切断中の USB で応答 FIFO が詰まっても、新しい接続を受信できるようにする。
        if tx_len != 0 && now_ms.saturating_sub(tx_started_ms) >= 2_000 {
            tx_len = 0;
            tx_sent = 0;
        }
        if controller.tick(now_ms, &mut display).is_err() {
            esp_println::println!("display expiry drawing failed");
        }
        if now_ms >= next_touch_poll_ms {
            next_touch_poll_ms = now_ms.saturating_add(if touch_enabled { 20 } else { 1000 });
            if !touch_enabled {
                touch_enabled = power::prepare_touch(&mut i2c, &mut delay)
                    .and_then(|_| touch::init(&mut i2c))
                    .is_ok();
                tap_detector = touch::TapDetector::default();
            } else {
                match touch::read_pressed(&mut i2c) {
                    Ok(pressed) if tap_detector.sample(pressed) => {
                        if controller.tap(now_ms, &mut display).is_err() {
                            esp_println::println!("touch demo drawing failed");
                        }
                    }
                    Ok(_) => {}
                    Err(_) => {
                        touch_enabled = false;
                        esp_println::println!("touch read failed; USB display remains active");
                    }
                }
            }
        }
        // 応答 1 件分だけを保持し、送信待ちでもブロッキング API を使わない。
        if tx_len == 0 {
            for _ in 0..64 {
                let Ok(byte) = usb.read_byte() else {
                    break;
                };
                if let Some(message) = receiver.push(byte) {
                    let reply = match message {
                        Ok(protocol::Message::HardwareProbe) => {
                            let version_6f =
                                actuators::probe_version(&mut i2c, actuators::PRIMARY_ADDRESS);
                            let version_71 =
                                actuators::probe_version(&mut i2c, actuators::ALTERNATE_ADDRESS);
                            let address = version_6f
                                .map(|_| actuators::PRIMARY_ADDRESS)
                                .or_else(|| version_71.map(|_| actuators::ALTERNATE_ADDRESS));
                            let (bus_out, boost_out) = power::body_power_latches(&mut i2c)
                                .map(|(bus, boost)| (Some(bus), Some(boost)))
                                .unwrap_or((None, None));
                            let servo_x_position = read_servo_register(&mut servo_uart, 1, 56, 2);
                            let servo_y_position = read_servo_register(&mut servo_uart, 2, 56, 2);
                            let servo_x_goal = read_servo_register(&mut servo_uart, 1, 42, 2);
                            let servo_y_goal = read_servo_register(&mut servo_uart, 2, 42, 2);
                            let servo_x_min_limit = read_servo_register(&mut servo_uart, 1, 9, 2);
                            let servo_x_max_limit = read_servo_register(&mut servo_uart, 1, 11, 2);
                            let servo_y_min_limit = read_servo_register(&mut servo_uart, 2, 9, 2);
                            let servo_y_max_limit = read_servo_register(&mut servo_uart, 2, 11, 2);
                            let servo_x_voltage = read_servo_register(&mut servo_uart, 1, 62, 1);
                            let servo_y_voltage = read_servo_register(&mut servo_uart, 2, 62, 1);
                            let servo_x_torque = read_servo_register(&mut servo_uart, 1, 40, 1);
                            let servo_y_torque = read_servo_register(&mut servo_uart, 2, 40, 1);
                            protocol::Reply::HardwareStatus {
                                version_6f,
                                version_71,
                                vm_out: address.and_then(|address| {
                                    actuators::probe_register(
                                        &mut i2c,
                                        address,
                                        actuators::VM_OUT_REGISTER,
                                    )
                                }),
                                led_config: address.and_then(|address| {
                                    actuators::probe_register(
                                        &mut i2c,
                                        address,
                                        actuators::LED_CONFIG_REGISTER,
                                    )
                                }),
                                bus_out,
                                boost_out,
                                servo_x_position,
                                servo_y_position,
                                servo_x_goal,
                                servo_y_goal,
                                servo_x_min_limit,
                                servo_x_max_limit,
                                servo_y_min_limit,
                                servo_y_max_limit,
                                servo_x_voltage: servo_x_voltage.map(|value| value as u8),
                                servo_y_voltage: servo_y_voltage.map(|value| value as u8),
                                servo_x_torque: servo_x_torque.map(|value| value != 0),
                                servo_y_torque: servo_y_torque.map(|value| value != 0),
                                enabled: actuator_ready,
                            }
                        }
                        Ok(message) => controller
                            .handle(message, now_ms, &mut display)
                            .unwrap_or_else(|_| {
                                esp_println::println!("display update failed");
                                receiver.reject()
                            }),
                        Err(reply) => reply,
                    };
                    // bootloader が USB に出したログと応答の境界を保証する。
                    tx[0] = 0;
                    tx_len = 1 + protocol::encode(&reply, &mut tx[1..])
                        .expect("reply buffer capacity")
                        .len();
                    tx_started_ms = Instant::now().duration_since_epoch().as_millis();
                    break;
                }
            }
        }
        while tx_sent < tx_len {
            if usb.write_byte_nb(tx[tx_sent]).is_err() {
                break;
            }
            tx_sent += 1;
        }
        if tx_len != 0 && tx_sent == tx_len {
            // flush はパケットを送信要求する操作なので 1 度だけ行う。
            // WouldBlock でも FIFO へのコピーは完了している。次のパケットは
            // write_byte_nb が FIFO の空きを確認してから書き込む。
            let _ = usb.flush_tx_nb();
            tx_len = 0;
            tx_sent = 0;
        }
        if actuator_ready {
            let actuator_now_ms = Instant::now().duration_since_epoch().as_millis();
            let target = controller.actuator_target();
            let positions = actuators::servo_positions(target);
            let rgb = my_stackchan_firmware::behavior::breathing_rgb(target.rgb, actuator_now_ms);
            if rgb != last_rgb {
                if actuators::show_rgb(&mut i2c, rgb).is_ok() {
                    last_rgb = rgb;
                } else {
                    actuator_ready = false;
                }
            }
            if actuator_ready
                && !servo_position_known
                && target != my_stackchan_firmware::behavior::ActuatorTarget::NEUTRAL
                && actuator_now_ms >= next_position_probe_ms
            {
                next_position_probe_ms = actuator_now_ms.saturating_add(500);
                let x = read_servo_register(&mut servo_uart, 1, 56, 2);
                let y = read_servo_register(&mut servo_uart, 2, 56, 2);
                if let (Some(x), Some(y)) = (x, y)
                    && x <= 1000
                    && y <= 1000
                {
                    last_target = (x, y);
                    servo_position_known = true;
                }
            }
            if actuator_ready && servo_position_known {
                if positions != last_target
                    && last_servo_write_ms
                        .is_none_or(|last| actuator_now_ms.saturating_sub(last) >= 100)
                {
                    // 古いゴールが残っていてもトルク投入時に跳ねないよう、まず現在位置を保持する。
                    if !torque_enabled {
                        let x = read_servo_register(&mut servo_uart, 1, 56, 2);
                        let y = read_servo_register(&mut servo_uart, 2, 56, 2);
                        if let (Some(x), Some(y)) = (x, y)
                            && x <= 1000
                            && y <= 1000
                        {
                            last_target = (x, y);
                            if positions != last_target {
                                let hold_x = actuators::servo_packet(1, x);
                                let hold_y = actuators::servo_packet(2, y);
                                actuator_ready = send_servo(&mut servo_uart, &hold_x).is_ok()
                                    && send_servo(&mut servo_uart, &hold_y).is_ok()
                                    && send_servo(
                                        &mut servo_uart,
                                        &actuators::servo_torque_packet(1, true),
                                    )
                                    .is_ok()
                                    && send_servo(
                                        &mut servo_uart,
                                        &actuators::servo_torque_packet(2, true),
                                    )
                                    .is_ok();
                                torque_enabled = actuator_ready;
                            }
                        } else {
                            last_servo_write_ms = Some(actuator_now_ms);
                        }
                    }
                    if actuator_ready && torque_enabled {
                        let next = (
                            actuators::step_position(last_target.0, positions.0),
                            actuators::step_position(last_target.1, positions.1),
                        );
                        let x = actuators::servo_packet(1, next.0);
                        let y = actuators::servo_packet(2, next.1);
                        if send_servo(&mut servo_uart, &x).is_ok()
                            && send_servo(&mut servo_uart, &y).is_ok()
                        {
                            last_target = next;
                            last_servo_write_ms = Some(actuator_now_ms);
                        } else {
                            actuator_ready = false;
                        }
                    }
                } else if torque_enabled
                    && positions == last_target
                    && last_servo_write_ms
                        .is_some_and(|last| actuator_now_ms.saturating_sub(last) >= 800)
                {
                    actuator_ready =
                        send_servo(&mut servo_uart, &actuators::servo_torque_packet(1, false))
                            .is_ok()
                            && send_servo(
                                &mut servo_uart,
                                &actuators::servo_torque_packet(2, false),
                            )
                            .is_ok();
                    torque_enabled = false;
                }
            }
            // ESP 側だけ再起動した場合もサーボ側のトルク状態を残さない。
            if actuator_ready
                && target == my_stackchan_firmware::behavior::ActuatorTarget::NEUTRAL
                && positions == last_target
                && last_servo_write_ms
                    .is_none_or(|last| actuator_now_ms.saturating_sub(last) >= 800)
                && actuator_now_ms >= next_idle_torque_off_ms
            {
                actuator_ready =
                    send_servo(&mut servo_uart, &actuators::servo_torque_packet(1, false)).is_ok()
                        && send_servo(&mut servo_uart, &actuators::servo_torque_packet(2, false))
                            .is_ok();
                torque_enabled = false;
                next_idle_torque_off_ms = actuator_now_ms.saturating_add(5_000);
            }
            if !actuator_ready {
                let _ = actuators::disable(&mut i2c);
                esp_println::println!("StackChan servo/LED disabled after I/O failure");
            }
        }
        delay.delay(Duration::from_millis(1));
    }
}
