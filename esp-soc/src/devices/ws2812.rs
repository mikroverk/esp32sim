// WS2812-class decoder policy, not hardware calibration.
pub const MIN_PULSE_NS: u64 = 150;
pub const MAX_HIGH_NS: u64 = 1100;
pub const ONE_HIGH_NS: u64 = 550;
pub const RESET_US: u64 = 50;

/// A chain of WS2812-class LEDs fed by a decoded RMT bit stream: 24 bits per LED, G then R then
/// B, MSB first, in chain order. `leds` holds the colours in *physical* order when the chain was
/// made with a map (a ring wired out of sequence, a grid wired serpentine), so everything above
/// the board — the page, the PNG, the report, the tests — sees the module as the eye does.
pub struct Ws2812Chain {
    pub leds: Vec<[u8; 3]>,
    pub updates: u64,
    physical: Option<&'static [usize]>,
    gpio: GpioStream,
    gpio_hz: u32,
}

#[derive(Default)]
struct GpioStream {
    high_since: Option<u64>,
    low_since: Option<u64>,
    bits: Vec<bool>,
    invalid: bool,
}

impl Ws2812Chain {
    /// `n` LEDs in chain order.
    pub fn new(n: usize) -> Self { Ws2812Chain { leds: vec![[0; 3]; n], updates: 0, gpio: GpioStream::default(), gpio_hz: 0, physical: None } }

    /// A chain whose LED `i` sits at physical position `map[i]`. `map` must be a permutation.
    pub fn mapped(map: &'static [usize]) -> Self {
        let mut seen = vec![false; map.len()];
        for &p in map { assert!(p < map.len() && !seen[p], "WS2812 physical map is not a permutation: {:?}", map); seen[p] = true; }
        Ws2812Chain { leds: vec![[0; 3]; map.len()], updates: 0, gpio: GpioStream::default(), gpio_hz: 0, physical: Some(map) }
    }

    /// Enable GPIO decoding at the board's nonzero cycle frequency. External board models and
    /// the shared CLI/WASM regression feed `gpio_output_at` into `gpio_drive`, expose
    /// `gpio_deadline` through `next_deadline`, and call `advance_gpio` from `advance_to`.
    pub fn with_gpio_clock(mut self, hz: u32) -> Self {
        assert!(hz != 0, "GPIO waveform clock must be nonzero");
        self.gpio_hz = hz;
        self
    }

    /// WS2812 pulse framing: high pulses encode bits; >=50 us low latches GRB.
    /// Accepts 150..1100 ns high pulses, splitting zero and one at 550 ns.
    /// https://cdn.sparkfun.com/assets/e/6/1/f/4/WS2812B-LED-datasheet.pdf
    pub fn gpio_drive(&mut self, cycle: u64, enabled: bool, high: bool) {
        let hz = self.gpio_hz;
        if !enabled || hz == 0 {
            self.gpio = GpioStream::default();
            return;
        }
        self.advance_gpio(cycle);
        if high {
            if self.gpio.high_since.is_none() {
                if let Some(start) = self.gpio.low_since {
                    let ns =
                        cycle.saturating_sub(start).saturating_mul(1_000_000_000) / u64::from(hz);
                    // Revision-specific maximum LOW windows are not modeled.
                    if ns < MIN_PULSE_NS {
                        self.gpio.invalid = true;
                    }
                }
                self.gpio.high_since = Some(cycle);
                self.gpio.low_since = None;
            }
        } else if let Some(start) = self.gpio.high_since.take() {
            let ns = cycle.saturating_sub(start).saturating_mul(1_000_000_000) / u64::from(hz);
            if !(MIN_PULSE_NS..=MAX_HIGH_NS).contains(&ns) {
                self.gpio.invalid = true;
            }
            if !self.gpio.invalid && self.gpio.bits.len() < self.leds.len() * 24 {
                self.gpio.bits.push(ns >= ONE_HIGH_NS);
            }
            self.gpio.low_since = Some(cycle);
        }
    }

    /// Deadline for a board's `next_deadline` callback, so quiet frames latch on C3 too.
    pub fn gpio_deadline(&self) -> Option<u64> {
        self.gpio
            .low_since
            .map(|start| start.saturating_add((RESET_US * u64::from(self.gpio_hz)).div_ceil(1_000_000)))
    }

    pub fn advance_gpio(&mut self, cycle: u64) {
        if self.gpio.low_since.is_some_and(|start| {
            cycle.saturating_sub(start).saturating_mul(1_000_000) >= RESET_US * u64::from(self.gpio_hz)
        }) {
            let bits = std::mem::take(&mut self.gpio.bits);
            if !self.gpio.invalid {
                self.from_bits(&bits);
            }
            self.gpio.invalid = false;
            self.gpio.low_since = None;
        }
    }

    /// Decode one transmission. A frame shorter than one LED (a lone reset pulse, a truncated
    /// stream) changes nothing and does not count as an update.
    pub fn from_bits(&mut self, bits: &[bool]) {
        let n = bits.len() / 24;
        if n == 0 { return; }
        for i in 0..n.min(self.leds.len()) {
            let mut v = 0u32;
            for b in 0..24 { v = (v << 1) | bits[i * 24 + b] as u32; }
            let at = match self.physical { Some(m) => m[i], None => i };
            self.leds[at] = [((v >> 8) & 0xff) as u8, ((v >> 16) & 0xff) as u8, (v & 0xff) as u8];   // GRB -> RGB
        }
        self.updates += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bits(bytes: &[u8]) -> Vec<bool> { bytes.iter().flat_map(|&v| (0..8).map(move |i| v & (0x80 >> i) != 0)).collect() }

    #[test]
    #[should_panic(expected = "GPIO waveform clock must be nonzero")]
    fn gpio_clock_must_be_nonzero() {
        Ws2812Chain::new(1).with_gpio_clock(0);
    }

    #[test]
    fn gpio_trailing_glitch_discards_even_complete_pixels() {
        let mut c = Ws2812Chain::new(1).with_gpio_clock(1_000_000_000);
        for i in 0..24 {
            c.gpio_drive(i * 1250, true, true);
            c.gpio_drive(i * 1250 + 800, true, false);
        }
        c.gpio_drive(30_000, true, true);
        c.gpio_drive(30_001, true, false);
        c.advance_gpio(100_000);
        assert_eq!(c.updates, 0);
    }

    #[test]
    fn gpio_stream_bounds_storage_and_repeated_levels() {
        let mut c = Ws2812Chain::new(1).with_gpio_clock(1_000_000_000);
        for i in 0..48 {
            let at = i * 1250;
            c.gpio_drive(at, true, true);
            c.gpio_drive(at + 100, true, true);
            c.gpio_drive(at + 550, true, false);
        }
        assert_eq!(c.gpio.bits, vec![true; 24]);
        let deadline = c.gpio_deadline().unwrap();
        assert_eq!(deadline, 47 * 1250 + 550 + 50_000);
        c.advance_gpio(deadline);
        assert_eq!(c.leds, [[255; 3]]);
        assert_eq!(c.gpio_deadline(), None);
        c.gpio_drive(deadline + 1, true, true);
        c.gpio_drive(deadline + 400, false, false);
        assert!(c.gpio.high_since.is_none());
    }

    #[test]
    fn gpio_timing_limits_and_long_uptime_do_not_accept_glitches_or_overflow() {
        for (high, low, valid) in [
            (400, 0, false),
            (400, 149, false),
            (400, 150, true),
            (149, 850, false),
            (150, 850, true),
            (1100, 450, true),
            (1101, 450, false),
            (400, 3800, true),
        ] {
            let mut c = Ws2812Chain::new(1).with_gpio_clock(1_000_000_000);
            let mut at = 0;
            for _ in 0..24 {
                c.gpio_drive(at, true, true);
                at += high;
                c.gpio_drive(at, true, false);
                at += low;
            }
            c.advance_gpio(u64::MAX);
            assert_eq!(c.updates, u64::from(valid), "high={high}, low={low}");
            c.gpio_drive(0, true, true);
            c.gpio_drive(u64::MAX, true, false);
            assert!(
                c.gpio.invalid,
                "an indefinitely high line is not a data bit"
            );
        }
    }

    #[test]
    fn gpio_pulses_require_output_and_complete_reset_interval() {
        for hz in [160_000_000, 240_000_000] {
            let mut c = Ws2812Chain::new(1).with_gpio_clock(hz);
            let tick = |ns: u64| ns * u64::from(hz) / 1_000_000_000;
            let mut at = 0;
            for bit in bits(&[0x12, 0x34, 0x56]) {
                c.gpio_drive(at, true, true);
                at += tick(if bit { 800 } else { 400 });
                c.gpio_drive(at, true, false);
                at += tick(if bit { 450 } else { 850 });
            }
            let falling = c.gpio.low_since.unwrap();
            assert_eq!(c.gpio_deadline(), Some(falling + tick(50_000)));
            c.advance_gpio(falling + tick(50_000) - 1);
            assert_eq!(c.updates, 0, "reset must last >=50 us");
            c.advance_gpio(falling + tick(50_000));
            assert_eq!((c.leds[0], c.updates), ([0x34, 0x12, 0x56], 1));
            c.advance_gpio(at + tick(100_000));
            assert_eq!(c.updates, 1);
            at += tick(200_000);
            for bit in bits(&[0xff, 0xff, 0xff]) {
                c.gpio_drive(at, false, true);
                at += tick(if bit { 800 } else { 400 });
                c.gpio_drive(at, false, false);
                at += tick(850);
            }
            c.advance_gpio(at + tick(50_000));
            assert_eq!(c.updates, 1, "input pins cannot transmit");
            at += tick(100_000);
            c.gpio_drive(at, true, true);
            at += tick(2000);
            c.gpio_drive(at, true, false);
            for _ in 0..24 {
                at += tick(850);
                c.gpio_drive(at, true, true);
                at += tick(400);
                c.gpio_drive(at, true, false);
            }
            c.advance_gpio(at + tick(50_000));
            assert_eq!(c.updates, 1, "invalid high pulse invalidates frame");
            at += tick(100_000);
            c.gpio_drive(at, true, true);
            c.gpio_drive(at + tick(400), true, false);
            c.advance_gpio(at + tick(100_000));
            assert_eq!(c.updates, 1, "partial pixel is ignored");
        }
    }

    #[test]
    fn grb_on_the_wire_becomes_rgb_in_chain_order() {
        let mut c = Ws2812Chain::new(2);
        c.from_bits(&bits(&[0x10, 0xab, 0x03, 1, 2, 3]));
        assert_eq!((c.leds[0], c.leds[1], c.updates), ([0xab, 0x10, 0x03], [2, 1, 3], 1));
        c.from_bits(&bits(&[0, 0, 0])[..20]);
        assert_eq!(c.updates, 1, "a short frame is not an update");
        c.from_bits(&bits(&[9, 9, 9, 9, 9, 9, 9, 9, 9]));
        assert_eq!((c.leds.len(), c.updates), (2, 2), "extra LEDs on the wire fall off the end");
    }

    #[test]
    fn a_map_places_each_chain_led_physically() {
        static REVERSED: [usize; 3] = [2, 1, 0];
        let mut c = Ws2812Chain::mapped(&REVERSED);
        c.from_bits(&bits(&[0, 1, 0, 0, 2, 0, 0, 3, 0]));
        assert_eq!(c.leds, [[3, 0, 0], [2, 0, 0], [1, 0, 0]]);
    }

    #[test]
    #[should_panic(expected = "not a permutation")]
    fn a_map_must_be_a_permutation() { static BAD: [usize; 3] = [0, 0, 2]; Ws2812Chain::mapped(&BAD); }
}
