//! C3 physical routes. IO_MUX offsets are 4 + 4 * GPIO number.
use crate::periph::Peripherals;
use esp_soc::board::SpiPins;

impl Peripherals {
    fn valid_pin(pin: usize) -> bool {
        pin < 22
    }

    fn mux(&self, pin: usize) -> u32 {
        self.io_mux.read(4 + 4 * pin as u32)
    }

    pub(crate) fn i2c_pin(&self, signal: usize) -> Option<u8> {
        let sel = self.gpio.func_in_sel[signal];
        let pin = (sel & 31) as usize;
        (Self::valid_pin(pin)
            && sel & 0x60 == 0x40
            && self.mux(pin) & (7 << 12 | 1 << 9) == (1 << 12 | 1 << 9)
            && self.matrix_output(pin, signal as u32))
        .then_some(pin as u8)
    }

    fn matrix_output(&self, pin: usize, signal: u32) -> bool {
        let sel = self.gpio.func_out_sel[pin];
        self.mux(pin) & (7 << 12) == 1 << 12
            && sel & (0xff | 1 << 8 | 1 << 10) == signal
            && (sel & (1 << 9) == 0 || self.gpio.enable & (1 << pin) != 0)
    }

    pub(crate) fn spi2_pins(&self) -> SpiPins {
        // Native FSPI routes: function, SCLK, MOSI, MISO, CS0.
        const NATIVE: [(u32, usize, usize, usize, usize); 1] = [(2, 6, 7, 2, 10)];
        let outputs = |signal, role| {
            let mut mask = 0;
            for pin in 0..22 {
                if Self::valid_pin(pin)
                    && (self.matrix_output(pin, signal)
                        || NATIVE.iter().any(|&(f, c, o, _, s)| {
                            self.mux(pin) & (7 << 12) == f << 12 && [c, o, s][role] == pin
                        }))
                {
                    mask |= 1 << pin;
                }
            }
            mask
        };
        let user = self.spi2.read(0x10);
        let mut pins = SpiPins {
            sclk: outputs(63, 0),
            ..Default::default()
        };
        if user & (1 << 31 | 1 << 30 | 1 << 27) != 0 {
            pins.mosi = outputs(65, 1);
        }
        for pin in 0..22 {
            if !Self::valid_pin(pin) {
                continue;
            }
            if self.matrix_output(pin, 128)
                && self.gpio.enable & (1 << pin) != 0
                && self.gpio.out & (1 << pin) == 0
            {
                pins.cs |= 1 << pin;
            }
            for cs in 0..6 {
                if self.spi2.read(0x20) & ((1 << cs) | (1 << (cs + 7))) == 0
                    && self.matrix_output(pin, 68 + cs)
                {
                    pins.cs |= 1 << pin;
                }
            }
        }
        if self.spi2.read(0x20) & (1 | 1 << 7) == 0 {
            pins.cs |= outputs(68, 2);
        }
        if user & (1 << 28) != 0 {
            let sel = self.gpio.func_in_sel[64];
            let pin = (sel & 31) as usize;
            if sel & 0x40 != 0 {
                if Self::valid_pin(pin)
                    && sel & 0x20 == 0
                    && self.mux(pin) & (7 << 12 | 1 << 9) == (1 << 12 | 1 << 9)
                {
                    pins.miso = Some(pin as u8);
                }
            } else {
                pins.miso = NATIVE.iter().find_map(|&(f, _, _, pin, _)| {
                    (self.mux(pin) & (7 << 12 | 1 << 9) == (f << 12 | 1 << 9)).then_some(pin as u8)
                });
            }
        }
        pins
    }
}
