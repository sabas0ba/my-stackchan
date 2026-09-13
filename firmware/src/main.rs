//! CoreS3 向け firmware。
//!
//! 起動時に Phase 1 の表示確認画面を描画する。

#![no_std]
#![no_main]

use core::cell::RefCell;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::main;
use esp_hal::time::Duration;
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
use my_stackchan_firmware::{power, renderer};

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

    loop {
        delay.delay(Duration::from_millis(1000));
    }
}
