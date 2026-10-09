use crate::periph::Peripherals;
use esp_soc::{board::SpiPins, pins::ChipPins};

impl Peripherals {
    pub(crate) fn i2c_pin(&self, signal: usize) -> Option<u8> { ChipPins::C3.routes(&self.gpio, &self.io_mux).i2c_pin(signal) }
    pub(crate) fn spi2_pins(&self) -> SpiPins {
        ChipPins::C3.routes(&self.gpio, &self.io_mux).spi_pins(&self.spi2, [63, 64, 65, 68, 69, 70, 71, 72, 73], 128, &[(2, 6, 7, 2, &[10])])
    }
}
