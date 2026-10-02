//! Byte-level UART endpoints for board models. Pins are chip GPIO numbers, so a device's
//! receive pin connects to `tx_pins`, and its transmit pin connects to `rx_pin`.
use esp_periph::{Gpio, RegRam};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UartRoute {
    pub port: usize,
    /// Every non-inverted, enabled TX output, including GPIO matrix fan-out.
    pub tx_pins: u64,
    pub rx_pin: Option<u8>,
    /// None when no source or divider is configured.
    pub baud: Option<u32>,
}
impl UartRoute {
    /// Three percent tolerance for divider rounding and oscillator error.
    pub fn matches_baud(&self, baud: u32) -> bool {
        baud != 0
            && self
                .baud
                .is_some_and(|actual| actual.abs_diff(baud) as u64 * 100 <= baud as u64 * 3)
    }
    pub fn transmits_on(&self, pin: u8) -> bool {
        pin < 64 && self.tx_pins & (1u64 << pin) != 0
    }
}

pub struct UartInput {
    pub pin: u8,
    pub baud: u32,
    /// Completed characters; the ordinary 128-byte RX FIFO/overflow rules apply.
    pub data: Vec<u8>,
}

/// Chip wiring from the IDF GPIO signal and IO_MUX tables.
pub struct UartPins {
    inputs: u64,
    outputs: u64,
    input_select: u32,
    output_mask: u32,
    signals: &'static [usize],
    native: &'static [(u8, u8, u32)],
}
impl UartPins {
    pub const S3: Self = Self {
        inputs: ((1u64 << 49) - 1) & !(15 << 22),
        outputs: ((1u64 << 49) - 1) & !(15 << 22) & !(1 << 46),
        input_select: 0x80,
        output_mask: 0x3ff,
        signals: &[12, 15, 18],
        native: &[(43, 44, 0), (17, 18, 2)],
    };
    pub const C3: Self = Self {
        inputs: (1 << 22) - 1,
        outputs: (1 << 22) - 1,
        input_select: 0x40,
        output_mask: 0x1ff,
        signals: &[6, 9],
        native: &[(21, 20, 0)],
    };
    pub const C6: Self = Self {
        inputs: (1 << 31) - 1,
        outputs: (1 << 31) - 1,
        input_select: 0x80,
        output_mask: 0x3ff,
        signals: &[6, 9],
        native: &[(16, 17, 0)],
    };

    pub fn route(&self, port: usize, gpio: &Gpio, mux: &RegRam, baud: Option<u32>) -> UartRoute {
        let mut route = UartRoute {
            port,
            tx_pins: 0,
            rx_pin: None,
            baud,
        };
        let Some(&signal) = self.signals.get(port) else {
            return route;
        };
        let native = self.native.get(port);
        let function = |pin: u8| (mux.read(4 + pin as u32 * 4) >> 12) & 7;
        for pin in 0..49 {
            if self.outputs & (1u64 << pin) == 0 {
                continue;
            }
            let matrix =
                function(pin) == 1 && gpio.func_out_sel[pin as usize] & self.output_mask == signal as u32;
            let direct = native.is_some_and(|&(tx, _, f)| tx == pin && function(pin) == f);
            if direct || (gpio.enable & (1u64 << pin) != 0 && matrix) {
                route.tx_pins |= 1u64 << pin;
            }
        }
        let input = gpio.func_in_sel[signal];
        let invert = self.input_select >> 1;
        let pin = if input & self.input_select != 0 {
            let pin = (input & (invert - 1)) as u8;
            (input & invert == 0 && self.inputs & (1u64 << pin) != 0).then_some(pin)
        } else {
            native.and_then(|&(_, rx, f)| (function(rx) == f).then_some(rx))
        };
        route.rx_pin = pin.filter(|&pin| mux.read(4 + pin as u32 * 4) & (1 << 9) != 0);
        route
    }
}
