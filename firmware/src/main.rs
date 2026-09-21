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
use my_stackchan_firmware::{model::Controller, power, renderer, transport::Receiver};
use static_cell::StaticCell;

static RECEIVER: StaticCell<Receiver> = StaticCell::new();
static CONTROLLER: StaticCell<Controller> = StaticCell::new();

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
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
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

    let mut usb = UsbSerialJtag::new(peripherals.USB_DEVICE);
    // Card を含む表示状態と受信バッファは大きいため、main のスタックから分離する。
    let receiver = RECEIVER.init_with(Receiver::default);
    let controller = CONTROLLER.init_with(Controller::default);
    let mut tx = [0u8; 32];
    let mut tx_len = 0;
    let mut tx_sent = 0;
    loop {
        let now_ms = Instant::now().duration_since_epoch().as_millis();
        if controller.tick(now_ms, &mut display).is_err() {
            esp_println::println!("display expiry drawing failed");
        }
        // 応答 1 件分だけを保持し、送信待ちでもブロッキング API を使わない。
        if tx_len == 0 {
            for _ in 0..64 {
                let Ok(byte) = usb.read_byte() else {
                    break;
                };
                if let Some(message) = receiver.push(byte) {
                    let reply = match message {
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
        delay.delay(Duration::from_millis(1));
    }
}
