use crate::periph::Peripherals;
use esp_soc::{board::SpiPins, pins::ChipPins};

impl Peripherals {
    pub(crate) fn i2c_pin(&self, signal: usize) -> Option<u8> { ChipPins::S3.routes(&self.gpio, &self.io_mux).i2c_pin(signal) }
    pub(crate) fn spi2_pins(&self) -> SpiPins {
        ChipPins::S3.routes(&self.gpio, &self.io_mux).spi_pins(&self.spi2, [101, 102, 103, 110, 111, 112, 113, 114, 115], 256, &[(4, 12, 11, 13, &[10]), (2, 36, 35, 37, &[34])])
    }
}
