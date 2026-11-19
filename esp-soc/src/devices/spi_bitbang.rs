/// A 4-wire SPI slave's view of three GPIOs driven by software: bytes are assembled MSB first on
/// SCLK rising edges while CS is low. The D/C line of a display is not SPI and stays with the
/// board.
pub struct SpiBitBang {
    sclk_pin: u8, mosi_pin: u8, cs_pin: u8,
    sclk: bool, mosi: bool, cs: bool,
    shift: u8, nbits: u8,
    /// Decoded transactions: one Vec per CS-low pulse, bytes in clock order.
    frames: Vec<Vec<u8>>,
    /// Cap on recorded frames; decoding keeps working past this, logging does not.
    frame_limit: usize,
}

impl SpiBitBang {
    pub fn new(sclk_pin: u8, mosi_pin: u8, cs_pin: u8) -> Self {
        SpiBitBang { sclk_pin, mosi_pin, cs_pin, sclk: false, mosi: false, cs: true, shift: 0, nbits: 0, frames: Vec::new(), frame_limit: 512 }
    }
    /// Decoded transactions, each one CS-low pulse.
    pub fn frames(&self) -> &[Vec<u8>] { &self.frames }

    /// Feed one GPIO output change, in order. Returns a byte when its last bit was clocked in.
    pub fn pin(&mut self, pin: u8, level: bool) -> Option<u8> {
        if pin == self.mosi_pin { self.mosi = level; }
        else if pin == self.cs_pin {
            if self.cs && !level && self.frames.len() < self.frame_limit { self.frames.push(Vec::new()); }
            self.cs = level;
            if level { self.nbits = 0; self.shift = 0; }
        }
        else if pin == self.sclk_pin {
            let rising = level && !self.sclk;
            self.sclk = level;
            if rising && !self.cs {
                self.shift = (self.shift << 1) | self.mosi as u8;
                self.nbits += 1;
                if self.nbits == 8 {
                    let b = self.shift;
                    self.nbits = 0;
                    self.shift = 0;
                    if let Some(frame) = self.frames.last_mut() { frame.push(b); }
                    return Some(b);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Clock one byte in on MOSI 1 / SCLK 2; the byte the slave assembled, if any.
    fn clock_in(s: &mut SpiBitBang, byte: u8) -> Option<u8> {
        let mut out = None;
        for i in 0..8 {
            s.pin(1, byte & (0x80 >> i) != 0);
            if let Some(b) = s.pin(2, true) { out = Some(b); }
            s.pin(2, false);
        }
        out
    }

    #[test]
    fn bytes_assemble_msb_first_on_rising_clock_while_selected() {
        let mut s = SpiBitBang::new(2, 1, 41);
        assert_eq!(clock_in(&mut s, 0xa5), None, "CS idles high: nothing is clocked in");
        s.pin(41, false);
        assert_eq!(clock_in(&mut s, 0xa5), Some(0xa5));
        for _ in 0..3 { s.pin(1, true); s.pin(2, true); s.pin(2, false); }
        s.pin(41, true); s.pin(41, false);
        assert_eq!(clock_in(&mut s, 0x3c), Some(0x3c), "a CS pulse discards the partial byte");
    }
}
