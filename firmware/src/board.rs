//! CoreS3 の表示に必要な電源と共有信号を制御する。

use core::{cell::RefCell, convert::Infallible};

use embedded_hal::digital;
use embedded_hal::spi::SpiBus;
use embedded_hal_bus::spi::RefCellDevice;
use embedded_sdmmc::{SdCard, SdCardError};
use esp_hal::gpio::{Flex, Level, Output};
use esp_hal::{
    Blocking,
    delay::Delay,
    spi::master::{Config, Spi},
    time::Rate,
};

type Card<'a, 'd> = SdCard<RefCellDevice<'a, Spi<'d, Blocking>, &'a mut Output<'d>, Delay>, Delay>;

#[derive(Debug)]
pub enum SdError {
    Bus,
    Card(SdCardError),
    Filesystem(my_stackchan_firmware::storage::Error<SdCardError>),
}

impl core::fmt::Display for SdError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Bus => f.write_str("SPI transfer/configuration"),
            Self::Card(error) => write!(f, "card: {error:?}"),
            Self::Filesystem(error) => write!(f, "filesystem: {error:?}"),
        }
    }
}

/// main の単一タスクからだけ呼ぶ。LCD の転送が無い区間で速度を切り替える。
pub fn with_sd<'a, 'd, R>(
    bus: &'a RefCell<Spi<'d, Blocking>>,
    cs: &'a mut Output<'d>,
    operation: impl FnOnce(
        Card<'a, 'd>,
    ) -> Result<R, my_stackchan_firmware::storage::Error<SdCardError>>,
) -> Result<R, SdError> {
    let result = (|| {
        cs.set_high();
        {
            let mut spi = bus.borrow_mut();
            spi.apply_config(&Config::default().with_frequency(Rate::from_khz(400)))
                .map_err(|_| SdError::Bus)?;
            // SD の SPI モード移行には CS=High の状態で 74 clocks 以上が必要。
            SpiBus::write(&mut *spi, &[0xff; 10]).map_err(|_| SdError::Bus)?;
            SpiBus::flush(&mut *spi).map_err(|_| SdError::Bus)?;
        }
        let device = RefCellDevice::new(bus, cs, Delay::new()).map_err(|_| SdError::Bus)?;
        let card = SdCard::new(device, Delay::new());
        card.num_bytes().map_err(SdError::Card)?;
        bus.borrow_mut()
            .apply_config(&Config::default().with_frequency(Rate::from_mhz(10)))
            .map_err(|_| SdError::Bus)?;
        operation(card).map_err(SdError::Filesystem)
    })();
    // エラーでも LCD の速度に戻す。SD の MISO 解放用に CS=High で追加 clock を送る。
    let mut spi = bus.borrow_mut();
    let released = SpiBus::write(&mut *spi, &[0xff]).and_then(|_| SpiBus::flush(&mut *spi));
    let restored = spi.apply_config(&Config::default().with_frequency(Rate::from_mhz(40)));
    if released.is_err() || restored.is_err() {
        return Err(SdError::Bus);
    }
    result
}

// GPIO35 は microSD の MISO と共有される。D/C の値はラッチだけに設定し、
// RefCellDevice が CS を操作するときだけ出力を有効にする。
pub struct DcPin<'a, 'd>(pub &'a RefCell<Flex<'d>>);

pub struct CsPin<'a, 'd> {
    pub pin: Output<'d>,
    pub dc: &'a RefCell<Flex<'d>>,
}

impl digital::ErrorType for DcPin<'_, '_> {
    type Error = Infallible;
}

impl digital::OutputPin for DcPin<'_, '_> {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        self.0.borrow_mut().set_level(Level::Low);
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Self::Error> {
        self.0.borrow_mut().set_level(Level::High);
        Ok(())
    }
}

impl digital::ErrorType for CsPin<'_, '_> {
    type Error = Infallible;
}

impl digital::OutputPin for CsPin<'_, '_> {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        self.pin.set_low();
        self.dc.borrow_mut().set_output_enable(true);
        Ok(())
    }

    fn set_high(&mut self) -> Result<(), Self::Error> {
        self.dc.borrow_mut().set_output_enable(false);
        self.pin.set_high();
        Ok(())
    }
}
