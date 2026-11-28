//! Page-addressed monochrome LCD controllers, ST7565 family: the SH1106 on I2C (0x3C) and the
//! NT7534 on SPI. Both drive 128×64 glass from a 132-column × 8-page GDDRAM laid out one byte per
//! column, bit 0 the top row of the page, and both take the same command set:
//!
//! * `0x00..=0x0F` column address low nibble, `0x10..=0x1F` high nibble
//! * `0xB0..=0xB7` page 0..7, `0xB8` page 8 (the icon page, one bit per column), `0x40..=0x7F`
//!   display start line
//! * `0xA0`/`0xA1` segment remap, `0xC0..=0xC7`/`0xC8..=0xCF` COM scan direction (bit 3)
//! * `0xA6`/`0xA7` normal/inverse, `0xA4`/`0xA5` RAM/all-on, `0xAE`/`0xAF` display off/on
//! * `0x81` contrast (one argument), `0xA8`/`0xD3`/`0xD5`/`0xD9`/`0xDA`/`0xDB` (one argument)
//! * `0x2F` power on, `0xE2` software reset
//!
//! Only the entry point differs: the SH1106 frames every I2C write with a control byte (`0x00`
//! command, `0x40` data, and with Co set a new control byte after each one), while the NT7534 uses
//! its RS pin. Both feed the same `MonoPanel`.
//!
//! The two modules wire their glass to different GDDRAM columns: the SH1106 shows columns 2..129
//! (u8g2's `sh1106` setup uses 2) and the NT7534 TG12864R 1..128 or 3..130, of which this model
//! takes 1. Treating them alike shifts one panel's picture two pixels sideways.
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
/// Leftmost GDDRAM column the glass shows: the SH1106's glass sits at column 2, which is what
/// u8g2's `sh1106` setup compensates for.
pub const SH1106_COL_OFFSET: usize = 2;
/// The NT7534 TG12864R pair is 1 or 3 depending on the module; this model wires the glass at 1.
/// (A plain 128×64 ST7565 is 0 or 4 — the offset is a property of the glass, not of the command
/// set, so it lives on the panel rather than in a shared constant.)
pub const NT7534_COL_OFFSET: usize = 1;

pub struct MonoPanel {
    ram: [u8; MONO_COLS * MONO_PAGES],
    /// Page 8, the icon page: one bit (D0) per column, outside the glass's 64 rows.
    icon: [bool; MONO_COLS],
    page: usize,
    col: usize,
    start_line: u8,
    seg_remap: bool,
    com_reverse: bool,
    inverted: bool,
    all_on: bool,
    on: bool,
    /// GDDRAM column the glass shows, a per-module property (see the constants above).
    col_offset: usize,
    /// Command arguments still to swallow (contrast, multiplex ratio, timing, …).
    pending_args: u8,
    /// The command that opened that argument, for the ones whose value is kept (`0x81`).
    pending_cmd: u8,
    pub contrast: u8,
    pub pixels_written: u64,
    pub commands: u64,
}

impl Default for MonoPanel {
    fn default() -> Self { Self::new() }
}

impl MonoPanel {
    /// An SH1106's panel: the glass starts at GDDRAM column 2.
    pub fn new() -> Self {
        Self {
            ram: [0; MONO_COLS * MONO_PAGES],
            icon: [false; MONO_COLS],
            page: 0,
            col: 0,
            start_line: 0,
            seg_remap: false,
            com_reverse: false,
            inverted: false,
            all_on: false,
            on: false,
            col_offset: SH1106_COL_OFFSET,
            pending_args: 0,
            pending_cmd: 0,
            contrast: 0x80,
            pixels_written: 0,
            commands: 0,
        }
    }

    /// The same controller behind a glass wired at a different GDDRAM column (the NT7534).
    pub fn with_col_offset(mut self, col_offset: usize) -> Self {
        self.col_offset = col_offset.min(MONO_COLS - 1);
        self
    }

    /// A 128×64 panel whose glass starts at column 1, as the NT7534 TG12864R modules do.
    pub fn nt7534() -> Self { Self::new().with_col_offset(NT7534_COL_OFFSET) }

    fn reset(&mut self) {
        // A software reset returns the controller to power-on but, as on silicon, the GDDRAM stays.
        let ram = self.ram;
        let (icon, col_offset) = (self.icon, self.col_offset);
        let (pixels_written, commands) = (self.pixels_written, self.commands);
        *self = Self::new();
        self.ram = ram;
        self.icon = icon;
        self.col_offset = col_offset;
        self.pixels_written = pixels_written;
        self.commands = commands;
    }

    /// One command byte. An argument that follows `0x81`/`0xA8`/`0xD3`/… is consumed here so it
    /// cannot be mistaken for the next command.
    pub fn command(&mut self, c: u8) {
        if self.pending_args > 0 {
            self.pending_args -= 1;
            // The electronic volume is the only argument with a visible effect in this model:
            // report() prints it, so swallowing it left the report showing 0x80 forever.
            if self.pending_cmd == 0x81 { self.contrast = c; }
            return;
        }
        self.commands += 1;
        match c {
            0x00..=0x0F => self.col = (self.col & 0xF0) | (c as usize & 0x0F),
            0x10..=0x1F => self.col = (self.col & 0x0F) | ((c as usize & 0x0F) << 4),
            0x40..=0x7F => self.start_line = c & 0x3F,
            0xA0 => self.seg_remap = false,
            0xA1 => self.seg_remap = true,
            // Bit 3 of the COM scan command is the direction: 0xC0..=0xC7 normal, 0xC8..=0xCF
            // reversed. Matching only 0xC8 left 0xC9..=0xCF ignored, and some drivers send one.
            0xC0..=0xC7 => self.com_reverse = false,
            0xC8..=0xCF => self.com_reverse = true,
            0xA4 => self.all_on = false,
            0xA5 => self.all_on = true,
            0xA6 => self.inverted = false,
            0xA7 => self.inverted = true,
            0xAE => self.on = false,
            0xAF => self.on = true,
            0xB0..=0xB7 => self.page = (c & 0x07) as usize,
            // Page 8 addresses the icon row (only D0 valid, one bit per column), not the glass.
            0xB8 => self.page = MONO_PAGES,
            // One-argument commands: contrast, multiplex ratio, display offset, clock, pre-charge,
            // COM pins, VCOM, and the SH1106 DC-DC enable. The argument is consumed, not decoded.
            0x81 | 0xA8 | 0xD3 | 0xD5 | 0xD9 | 0xDA | 0xDB | 0xAD => {
                self.pending_args = 1;
                self.pending_cmd = c;
            }
            0xE2 => self.reset(),
            // Single-byte commands with nothing to remember: power control, bias, VPP, RMW, NOP.
            // 0xB9..=0xBF would name pages 9..15, which the controller does not have.
            0x20..=0x33 | 0xA2 | 0xA3 | 0xB9..=0xBF | 0xE0 | 0xE3 | 0xEE => {}
            _ => {}
        }
    }

    /// One data byte into GDDRAM at the current page/column, then advance within the page.
    pub fn data(&mut self, b: u8) {
        let col = self.col.min(MONO_COLS - 1);
        if self.page >= MONO_PAGES {
            // The icon page keeps one bit per column: writing a byte there lights or clears one
            // icon and leaves the glass alone.
            self.icon[col] = b & 0x01 != 0;
        } else {
            self.ram[self.page * MONO_COLS + col] = b;
        }
        self.pixels_written += 8;
        self.col += 1;
        if self.col >= MONO_COLS { self.col = 0; }
    }

    /// The icon row the driver last wrote (page 8), for tests and captions.
    pub fn icon(&self) -> &[bool; MONO_COLS] { &self.icon }

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
                let col = (self.col_offset + src_x).min(MONO_COLS - 1);
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
    /// A control byte is expected next: at the start of a transaction, and after every byte when
    /// the previous control byte had Co set.
    control_next: bool,
    /// Co bit of the control byte in force: with Co = 1 the driver interleaves a control byte
    /// before each single command or data byte, which is how u8g2 sends a mixed stream.
    co: bool,
}

/// SH1106 and SSD1306 share the fixed address; SA0 high is 0x3D.
pub const SH1106_ADDRESS: u8 = 0x3C;

impl Sh1106 {
    pub fn new(panel: Arc<Mutex<MonoPanel>>) -> Self {
        Self { panel, data_mode: false, control_next: true, co: false }
    }
    pub fn panel(&self) -> Arc<Mutex<MonoPanel>> { self.panel.clone() }
    fn lock(&self) -> MutexGuard<'_, MonoPanel> {
        self.panel.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl I2cDevice for Sh1106 {
    fn start(&mut self, read: bool) -> bool {
        if !read { self.control_next = true; }
        true
    }

    fn write(&mut self, b: u8) -> bool {
        if self.control_next {
            // Control byte: bit 7 (Co) says a control byte follows every byte, bit 6 (D/C#) says
            // the bytes that follow are GDDRAM data.
            self.co = b & 0x80 != 0;
            self.data_mode = b & 0x40 != 0;
            self.control_next = false;
            return true;
        }
        let data_mode = self.data_mode;
        {
            let mut p = self.lock();
            if data_mode { p.data(b); } else { p.command(b); }
        }
        self.control_next = self.co;
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
    fn the_two_modules_start_their_glass_at_different_columns() {
        // The same commands and data: the SH1106's glass begins at GDDRAM column 2, the NT7534's
        // at column 1, so column 1 is off-screen on one and the left edge on the other.
        let mut sh = MonoPanel::new();
        for c in [0xB0, 0x01, 0x10] { sh.command(c); }
        sh.data(0x01);
        assert!(!lit(&sh, 0, 0), "column 1 is left of the SH1106's glass");
        let mut nt = MonoPanel::nt7534();
        for c in [0xB0, 0x01, 0x10] { nt.command(c); }
        nt.data(0x01);
        assert!(lit(&nt, 0, 0), "column 1 is the NT7534's left edge");
    }

    #[test]
    fn com_scan_direction_uses_bit_three() {
        let mut p = MonoPanel::new();
        for c in [0xB0, 0x02, 0x10] { p.command(c); }
        p.data(0x01);
        assert!(lit(&p, 0, 0));
        p.command(0xCF);
        assert!(lit(&p, 0, MONO_HEIGHT - 1), "0xC9..=0xCF reverse like 0xC8");
        p.command(0xC7);
        assert!(lit(&p, 0, 0), "0xC0..=0xC7 are the normal direction");
    }

    #[test]
    fn page_eight_is_the_icon_row_not_the_glass() {
        let mut p = MonoPanel::new();
        for c in [0xB8, 0x02, 0x10] { p.command(c); }
        p.data(0x01);
        assert!(p.icon()[2], "the icon page keeps D0 per column");
        assert!(!lit(&p, 0, 0), "an icon write does not paint the glass");
        p.data(0x00);
        assert!(!p.icon()[3]);
    }

    #[test]
    fn the_contrast_argument_is_kept() {
        let mut p = MonoPanel::new();
        p.command(0x81);
        p.command(0x3F);
        assert_eq!(p.contrast, 0x3F, "0x81 takes the electronic volume as its argument");
        assert_eq!(p.commands, 1, "the argument is not a command of its own");
        p.command(0xD9);
        p.command(0xF1);
        assert_eq!(p.contrast, 0x3F, "only 0x81 changes the contrast");
    }

    #[test]
    fn sh1106_honours_the_co_bit_between_bytes() {
        let ram = Arc::new(Mutex::new(MonoPanel::new()));
        let mut d = Sh1106::new(ram.clone());
        d.start(false);
        // Co = 1: a control byte before every single byte, as u8g2 sends a mixed stream.
        for (ctrl, byte) in [(0x80, 0xB0), (0x80, 0x02), (0x80, 0x10), (0xC0, 0xFF)] {
            d.write(ctrl);
            d.write(byte);
        }
        let p = ram.lock().unwrap();
        assert_eq!(p.commands, 3, "0xB0 and the two column commands");
        for y in 0..8 { assert!(lit(&p, 0, y), "all eight rows of page 0, row {y}"); }
        assert!(!lit(&p, 1, 0), "one data byte is one column");
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

    /// A reset restores the command state, but the glass's column offset belongs to the module and
    /// the icon row lives in the driver's RAM, so both survive. Nothing covered either: clearing
    /// them on `0xE2` left the suite green.
    #[test]
    fn a_reset_keeps_the_glass_offset_and_the_icon_row() {
        let mut p = MonoPanel::nt7534();
        for c in [0xB8, 0x01, 0x10] { p.command(c); }
        p.data(0x01);
        for c in [0xB0, 0x01, 0x10] { p.command(c); }
        p.data(0xFF);
        assert!(lit(&p, 0, 0), "the NT7534 glass starts at GDDRAM column 1");

        p.command(0xE2);
        assert!(p.icon()[1], "the icon row is RAM: a reset keeps it");
        assert!(lit(&p, 0, 0), "the offset survives, so column 1 is still the left edge");
    }
}
