/// A 4-wire SPI slave's view of three GPIOs driven by software: bytes are assembled MSB first on
/// SCLK rising edges while CS is low. The D/C line of a display is not SPI and stays with the
/// board.
pub struct SpiBitBang {
    sclk_pin: u8, mosi_pin: u8, cs_pin: u8,
    sclk: bool, mosi: bool, cs: bool,
    shift: u8, nbits: u8,
    /// Decoded transactions: one Vec per CS-low pulse, bytes in clock order.
    frames: Vec<Vec<u8>>,
    /// Bytes kept across every frame, so the cap cannot be walked past by opening new frames.
    recorded: usize,
    /// Caps on what is kept; decoding keeps working past both, recording does not.
    frame_limit: usize,
    byte_limit: usize,
    /// Whether decoded bytes are kept at all. Off unless the board turns it on for `--buslog`: a
    /// display writes megabytes of pixels and nothing else reads them, so keeping them for the
    /// whole run is unbounded memory in exchange for nothing.
    recording: bool,
}

impl SpiBitBang {
    pub fn new(sclk_pin: u8, mosi_pin: u8, cs_pin: u8) -> Self {
        SpiBitBang {
            sclk_pin, mosi_pin, cs_pin, sclk: false, mosi: false, cs: true, shift: 0, nbits: 0,
            frames: Vec::new(), recorded: 0, frame_limit: 512, byte_limit: 4096, recording: false,
        }
    }
    /// Keep the decoded bytes, for `--buslog`. Never changes what the panel receives.
    pub fn record(&mut self, on: bool) {
        self.recording = on;
        if !on { self.frames.clear(); self.recorded = 0; }
    }
    /// Decoded transactions, each one CS-low pulse.
    pub fn frames(&self) -> &[Vec<u8>] { &self.frames }

    /// Feed one GPIO output change, in order. Returns a byte when its last bit was clocked in.
    pub fn pin(&mut self, pin: u8, level: bool) -> Option<u8> {
        if pin == self.mosi_pin { self.mosi = level; }
        else if pin == self.cs_pin {
            if self.recording && self.cs && !level && self.frames.len() < self.frame_limit { self.frames.push(Vec::new()); }
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
                    if self.recording && self.recorded < self.byte_limit {
                        if let Some(frame) = self.frames.last_mut() { frame.push(b); }
                        self.recorded += 1;
                    }
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

    /// Choosing the board must not cost memory: nothing is kept unless `--buslog` asks, while the
    /// panel still receives every byte.
    #[test]
    fn nothing_is_recorded_until_the_board_asks() {
        let mut s = SpiBitBang::new(2, 1, 41);
        s.pin(41, false);
        for i in 0..64 { assert_eq!(clock_in(&mut s, i as u8), Some(i as u8)); }
        assert!(s.frames().is_empty(), "recording is off by default");

        s.record(true);
        s.pin(41, true); s.pin(41, false);
        clock_in(&mut s, 0x5a);
        s.pin(41, true);
        assert_eq!(s.frames(), [vec![0x5a]], "recorded once asked");
    }

    /// The frame cap alone let `pin` append to the last frame for the whole run; the byte cap has
    /// to hold too, or a guest that clocks a megabyte of pixels keeps all of it.
    #[test]
    fn recorded_bytes_stop_at_the_cap_and_decoding_continues() {
        let mut s = SpiBitBang::new(2, 1, 41);
        s.record(true);
        s.byte_limit = 8;
        s.pin(41, false);
        for i in 0..20 { assert_eq!(clock_in(&mut s, i as u8), Some(i as u8), "byte {i} still decodes"); }
        s.pin(41, true);
        let kept: usize = s.frames().iter().map(|f| f.len()).sum();
        assert_eq!(kept, 8, "kept {:?}", s.frames());
        assert_eq!(s.frames()[0], [0, 1, 2, 3, 4, 5, 6, 7]);
    }
}
