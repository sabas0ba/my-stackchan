//! CoreS3 の表示に必要な電源と共有信号を制御する。

use core::{cell::RefCell, convert::Infallible};

use embedded_hal::digital;
use esp_hal::gpio::{Flex, Level, Output};

// GPIO35 は microSD の MISO と共有される。D/C の値はラッチだけに設定し、
// ExclusiveDevice が CS を操作するときだけ出力を有効にする。
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
