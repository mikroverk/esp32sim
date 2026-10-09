//! Physical GPIO matrix and IO_MUX route decoding shared by chip buses and board devices.
use esp_periph::{Gpio, GpSpi, RegRam};
use crate::board::SpiPins;

pub struct ChipPins {
    pub valid: u64,
    pub input_select: u32,
    /// OUT_SEL and OUT_INV, excluding output-enable control bits.
    pub output_mask: u32,
}
impl ChipPins {
    pub const C3: Self = Self { valid: (1 << 22) - 1, input_select: 0x40, output_mask: 0x1ff };
    pub const C6: Self = Self { valid: (1 << 31) - 1, input_select: 0x80, output_mask: 0x1ff };
    pub const S3: Self = Self { valid: ((1u64 << 49) - 1) & !(15 << 22), input_select: 0x80, output_mask: 0x3ff };
    pub fn routes<'a>(&'a self, gpio: &'a Gpio, mux: &'a RegRam) -> PinRoutes<'a> { PinRoutes { chip: self, gpio, mux } }
}

pub struct PinRoutes<'a> {
    chip: &'a ChipPins,
    gpio: &'a Gpio,
    mux: &'a RegRam,
}
impl PinRoutes<'_> {
    pub fn valid_pin(&self, pin: usize) -> bool { pin < 64 && self.chip.valid & (1u64 << pin) != 0 }
    fn mux(&self, pin: usize) -> u32 { self.mux.read(4 + 4 * pin as u32) }
    pub fn function(&self, pin: usize, function: u32) -> bool { self.valid_pin(pin) && self.mux(pin) & (7 << 12) == function << 12 }
    pub fn input_function(&self, pin: usize, function: u32) -> bool { self.function(pin, function) && self.mux(pin) & (1 << 9) != 0 }
    /// Matrix input selection and FUN_IE, independent of the output function.
    pub fn input_pin(&self, signal: usize) -> Option<u8> {
        let sel = self.gpio.func_in_sel[signal];
        let invert = self.chip.input_select >> 1;
        let pin = (sel & (invert - 1)) as usize;
        (sel & (self.chip.input_select | invert) == self.chip.input_select && self.valid_pin(pin) && self.mux(pin) & (1 << 9) != 0).then_some(pin as u8)
    }
    pub fn matrix_input(&self, signal: usize) -> Option<u8> { self.input_pin(signal).filter(|&pin| self.function(pin as usize, 1)) }
    pub fn matrix_output(&self, pin: usize, signal: u32) -> bool {
        if !self.function(pin, 1) { return false; }
        let sel = self.gpio.func_out_sel[pin];
        let oen = self.chip.output_mask + 1;
        sel & (self.chip.output_mask | oen << 1) == signal && (sel & oen == 0 || self.gpio.enable & (1 << pin) != 0)
    }
    pub fn i2c_pin(&self, signal: usize) -> Option<u8> {
        self.matrix_input(signal).filter(|&pin| self.matrix_output(pin as usize, signal as u32))
    }
    /// Signals are CLK, MISO, MOSI, CS0..5; native tuples hold function, CLK/MOSI/MISO and CS pins.
    /// IDF v5.5.4 components/soc/esp32c6/register/soc/spi_reg.h:365-403,497-559
    /// defines the shared USER phase and MISC CS disable/polarity fields.
    pub fn spi_pins(&self, spi: &GpSpi, signals: [u32; 9], gpio_signal: u32, native: &[(u32, usize, usize, usize, &[usize])]) -> SpiPins {
        let outputs = |signal, role| {
            let mut mask = 0;
            for pin in 0..64 {
                if self.valid_pin(pin)
                    && (self.matrix_output(pin, signal)
                        || native.iter().any(|&(f, c, o, _, _)| {
                            self.mux(pin) & (7 << 12) == f << 12 && [c, o][role] == pin
                        }))
                {
                    mask |= 1 << pin;
                }
            }
            mask
        };
        let user = spi.read(0x10);
        let mut pins = SpiPins {
            sclk: outputs(signals[0], 0),
            ..Default::default()
        };
        if user & (1 << 31 | 1 << 30 | 1 << 27) != 0 {
            pins.mosi = outputs(signals[2], 1);
        }
        for pin in 0..64 {
            if !self.valid_pin(pin) {
                continue;
            }
            if self.matrix_output(pin, gpio_signal)
                && self.gpio.enable & (1 << pin) != 0
                && self.gpio.out & (1 << pin) == 0
            {
                pins.cs |= 1 << pin;
            }
            for cs in 0..6 {
                if spi.read(0x20) & ((1 << cs) | (1 << (cs + 7))) == 0
                    && (self.matrix_output(pin, signals[3 + cs])
                        || native.iter().any(|&(f, _, _, _, selects)| {
                            selects.get(cs) == Some(&pin) && self.function(pin, f)
                        }))
                {
                    pins.cs |= 1 << pin;
                }
            }
        }
        if user & (1 << 28) != 0 {
            pins.miso = self.matrix_input(signals[1] as usize);
            if self.gpio.func_in_sel[signals[1] as usize] & self.chip.input_select == 0 {
                pins.miso = native.iter().find_map(|&(f, _, _, pin, _)| {
                    self.input_function(pin, f).then_some(pin as u8)
                });
            }
        }
        pins
    }
}
