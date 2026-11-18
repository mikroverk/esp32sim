//! Page-addressed monochrome LCD controllers, ST7565 family: the SH1106 on I2C (0x3C) and the
//! NT7534 on SPI. Both drive 128×64 glass from a 132-column × 8-page GDDRAM laid out one byte per
//! column, bit 0 the top row of the page, and both take the same command set:
//!
//! * `0x00..=0x0F` column address low nibble, `0x10..=0x1F` high nibble
//! * `0xB0..=0xB7` page, `0x40..=0x7F` display start line
//! * `0xA0`/`0xA1` segment remap, `0xC0`/`0xC8` COM scan direction
//! * `0xA6`/`0xA7` normal/inverse, `0xA4`/`0xA5` RAM/all-on, `0xAE`/`0xAF` display off/on
//! * `0x81` contrast (one argument), `0xA8`/`0xD3`/`0xD5`/`0xD9`/`0xDA`/`0xDB` (one argument)
//! * `0x2F` power on, `0xE2` software reset
//!
//! Only the entry point differs: the SH1106 frames every I2C write with a `0x00` (command) or
//! `0x40` (data) control byte, while the NT7534 uses its RS pin. Both feed the same `MonoPanel`.
//!
//! The column pointer advances within the page and wraps to the page's column 0 — it never crosses
//! into the next page, which is why a driver sets the page and column before each 128-byte row.
use esp_periph::i2c::I2cDevice;
use std::sync::{Arc, Mutex, MutexGuard};

/// GDDRAM columns in the controller (the glass shows 128 of them).
pub const MONO_COLS: usize = 132;
/// GDDRAM pages, eight rows each.
pub const MONO_PAGES: usize = 8;
/// Wide of the glass.
pub const MONO_WIDTH: usize = 128;
/// Tall of the glass.
pub const MONO_HEIGHT: usize = 64;
/// Leftmost GDDRAM column the glass shows (SH1106/SSD1306 convention).
const COL_OFFSET: usize = 2;

pub struct MonoPanel {
    ram: [u8; MONO_COLS * MONO_PAGES],
    page: usize,
    col: usize,
    start_line: u8,
    seg_remap: bool,
    com_reverse: bool,
    inverted: bool,
    all_on: bool,
    on: bool,
    /// Command arguments still to swallow (contrast, multiplex ratio, timing, …).
    pending_args: u8,
    pub contrast: u8,
    pub pixels_written: u64,
    pub commands: u64,
}

impl Default for MonoPanel {
    fn default() -> Self { Self::new() }
}

impl MonoPanel {
    pub fn new() -> Self {
        Self {
            ram: [0; MONO_COLS * MONO_PAGES],
            page: 0,
            col: 0,
            start_line: 0,
            seg_remap: false,
            com_reverse: false,
            inverted: false,
            all_on: false,
            on: false,
            pending_args: 0,
            contrast: 0x80,
            pixels_written: 0,
            commands: 0,
        }
    }

    fn reset(&mut self) {
        // A software reset returns the controller to power-on but, as on silicon, the GDDRAM stays.
        let ram = self.ram;
        let (pixels_written, commands) = (self.pixels_written, self.commands);
        *self = Self::new();
        self.ram = ram;
        self.pixels_written = pixels_written;
        self.commands = commands;
    }

    /// One command byte. An argument that follows `0x81`/`0xA8`/`0xD3`/… is consumed here so it
    /// cannot be mistaken for the next command.
    pub fn command(&mut self, c: u8) {
        if self.pending_args > 0 {
            self.pending_args -= 1;
            return;
        }
        self.commands += 1;
        match c {
            0x00..=0x0F => self.col = (self.col & 0xF0) | (c as usize & 0x0F),
            0x10..=0x1F => self.col = (self.col & 0x0F) | ((c as usize & 0x0F) << 4),
            0x40..=0x7F => self.start_line = c & 0x3F,
            0xA0 => self.seg_remap = false,
            0xA1 => self.seg_remap = true,
            0xC0 => self.com_reverse = false,
            0xC8 => self.com_reverse = true,
            0xA4 => self.all_on = false,
            0xA5 => self.all_on = true,
            0xA6 => self.inverted = false,
            0xA7 => self.inverted = true,
            0xAE => self.on = false,
            0xAF => self.on = true,
            0xB0..=0xB7 => self.page = (c & 0x07) as usize,
            // One-argument commands: contrast, multiplex ratio, display offset, clock, pre-charge,
            // COM pins, VCOM, and the SH1106 DC-DC enable. The argument is consumed, not decoded.
            0x81 | 0xA8 | 0xD3 | 0xD5 | 0xD9 | 0xDA | 0xDB | 0xAD => self.pending_args = 1,
            0xE2 => self.reset(),
            // Single-byte commands with nothing to remember: power control, bias, VPP, RMW, NOP.
            0x20..=0x33 | 0xA2 | 0xA3 | 0xE0 | 0xE3 | 0xEE => {}
            _ => {}
        }
    }

    /// One data byte into GDDRAM at the current page/column, then advance within the page.
    pub fn data(&mut self, b: u8) {
        let idx = self.page * MONO_COLS + self.col.min(MONO_COLS - 1);
        self.ram[idx] = b;
        self.pixels_written += 8;
        self.col += 1;
        if self.col >= MONO_COLS { self.col = 0; }
    }

    pub fn is_on(&self) -> bool { self.on }

    /// The glass's size, without building the frame `visible()` returns.
    pub fn size(&self) -> (u32, u32) { (MONO_WIDTH as u32, MONO_HEIGHT as u32) }

    /// What the glass shows, as RGB565 (black/white), honouring remap, scan direction and inversion.
    pub fn visible(&self) -> (u32, u32, Vec<u16>) {
        let mut px = vec![0u16; MONO_WIDTH * MONO_HEIGHT];
        for y in 0..MONO_HEIGHT {
            let src_y = if self.com_reverse { MONO_HEIGHT - 1 - y } else { y };
            let row = (src_y + self.start_line as usize) % MONO_HEIGHT;
            let page = row / 8;
            let bit = 1u8 << (row % 8);
            for x in 0..MONO_WIDTH {
                let src_x = if self.seg_remap { MONO_WIDTH - 1 - x } else { x };
                let col = (COL_OFFSET + src_x).min(MONO_COLS - 1);
                let lit = if self.all_on {
                    true
                } else {
                    self.ram[page * MONO_COLS + col] & bit != 0
                };
                px[y * MONO_WIDTH + x] = if lit ^ self.inverted { 0xFFFF } else { 0x0000 };
            }
        }
        (MONO_WIDTH as u32, MONO_HEIGHT as u32, px)
    }
}

/// The SH1106 on I2C: every write transaction starts with a `0x00`/`0x40` control byte.
pub struct Sh1106 {
    panel: Arc<Mutex<MonoPanel>>,
    data_mode: bool,
    first: bool,
}

/// SH1106 and SSD1306 share the fixed address; SA0 high is 0x3D.
pub const SH1106_ADDRESS: u8 = 0x3C;

impl Sh1106 {
    pub fn new(panel: Arc<Mutex<MonoPanel>>) -> Self {
        Self { panel, data_mode: false, first: false }
    }
    pub fn panel(&self) -> Arc<Mutex<MonoPanel>> { self.panel.clone() }
    fn lock(&self) -> MutexGuard<'_, MonoPanel> {
        self.panel.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl I2cDevice for Sh1106 {
    fn start(&mut self, read: bool) -> bool {
        if !read { self.first = true; }
        true
    }

    fn write(&mut self, b: u8) -> bool {
        if self.first {
            // Control byte: bit 6 set means the bytes that follow are GDDRAM data.
            self.data_mode = b & 0x40 != 0;
            self.first = false;
            return true;
        }
        let mut p = self.lock();
        if self.data_mode { p.data(b); } else { p.command(b); }
        true
    }

    fn read(&mut self) -> u8 { 0 }
}

/// The NT7534 on SPI: the RS pin, not a control byte, says command or data.
pub struct Nt7534 {
    panel: Arc<Mutex<MonoPanel>>,
    data_mode: bool,
}

impl Nt7534 {
    pub fn new(panel: Arc<Mutex<MonoPanel>>) -> Self { Self { panel, data_mode: false } }
    /// RS low selects a command, high GDDRAM data — the same split the SH1106 encodes in a byte.
    pub fn set_rs(&mut self, data: bool) { self.data_mode = data; }
    /// One byte clocked out of the bit-banged SPI.
    pub fn byte(&mut self, b: u8) {
        let mut p = self.panel.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.data_mode { p.data(b); } else { p.command(b); }
    }
    pub fn panel(&self) -> Arc<Mutex<MonoPanel>> { self.panel.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(panel: &MonoPanel, x: usize, y: usize) -> bool {
        let (w, _, px) = panel.visible();
        px[y * w as usize + x] != 0
    }

    #[test]
    fn sh1106_takes_a_control_byte_then_commands_and_data() {
        let ram = Arc::new(Mutex::new(MonoPanel::new()));
        let mut d = Sh1106::new(ram.clone());
        // Page 0, column 2 (the glass's left edge), then 8 bytes that light the top row.
        d.start(false);
        // Control byte, then page 0 and column 2 (the glass's left edge): 0x02 low, 0x10 high.
        for b in [0x00, 0xB0, 0x02, 0x10] { d.write(b); }
        d.start(false);
        d.write(0x40);
        for _ in 0..8 { d.write(0xFF); }
        let p = ram.lock().unwrap();
        assert_eq!(p.commands, 3, "0xB0 and the two column commands");
        for x in 0..8 { assert!(lit(&p, x, 0), "pixel {x},0 must be lit"); }
        assert!(!lit(&p, 8, 0), "the ninth column was never written");
    }

    #[test]
    fn the_column_pointer_wraps_within_its_page() {
        let mut p = MonoPanel::new();
        p.command(0xB0);
        p.command(0x00);
        p.command(0x10);
        for _ in 0..MONO_COLS { p.data(0xAA); }
        p.data(0x55);  // wraps to column 0 of page 0, not into page 1
        assert_eq!(p.ram[0], 0x55);
        assert_eq!(p.ram[MONO_COLS], 0, "page 1 stays untouched");
    }

    #[test]
    fn inverse_and_segment_remap_reach_the_glass() {
        let mut p = MonoPanel::new();
        p.command(0xB0); p.command(0x02); p.command(0x10);
        p.data(0x01);  // column 2, top row of page 0
        assert!(lit(&p, 0, 0), "column 2 is the glass x=0");
        p.command(0xA7);
        assert!(!lit(&p, 0, 0), "inverted: the lit pixel goes dark");
        p.command(0xA6);
        p.command(0xA1);
        assert!(lit(&p, MONO_WIDTH - 1, 0), "segment remap mirrors x");
    }

    #[test]
    fn nt7534_uses_rs_instead_of_a_control_byte() {
        let ram = Arc::new(Mutex::new(MonoPanel::new()));
        let mut d = Nt7534::new(ram.clone());
        d.set_rs(false);
        for c in [0xB0, 0x02, 0x10] { d.byte(c); }
        d.set_rs(true);
        for _ in 0..4 { d.byte(0xFF); }
        let p = ram.lock().unwrap();
        assert_eq!(p.commands, 3);
        for x in 0..4 { assert!(lit(&p, x, 0)); }
    }

    #[test]
    fn a_reset_returns_to_power_on_but_keeps_the_ram() {
        let mut p = MonoPanel::new();
        p.command(0xAF); p.command(0xA7);
        p.command(0xB0); p.command(0x02); p.command(0x10);
        p.data(0xFF);
        p.command(0xE2);
        assert!(!p.is_on(), "software reset turns the display off");
        assert!(lit(&p, 0, 0), "the GDDRAM survives a reset, as on silicon");
        assert_eq!(p.ram[2], 0xFF);
    }
}
