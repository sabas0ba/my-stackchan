//! CoreS3 向け firmware。
//!
//! 現段階は toolchain の動作確認を目的とした最小構成である。ペリフェラルの初期化
//! (AXP2101、AW9523、ILI9342C) と受信ループは docs/design.md のフェーズに従って追加する。

#![no_std]
#![no_main]

use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::main;
use esp_hal::time::Duration;

#[main]
fn main() -> ! {
    let _peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();

    esp_println::println!("my-stackchan firmware: protocol v{}", protocol::VERSION);

    loop {
        delay.delay(Duration::from_millis(1000));
    }
}
