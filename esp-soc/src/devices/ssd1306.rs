//! A Solomon SSD1306 128x64 monochrome OLED controller on I2C (0x3C or 0x3D), from its datasheet
//! (rev 1.1, sections 8.1.5, 9 and 10). Each transfer starts with control bytes: Co (bit 7) clear
//! means every following byte is of the kind D/C# (bit 6) names; Co set means one byte follows, then
//! another control byte. Commands with arguments take them from the following command bytes. GDDRAM
//! is 8 pages of 128 columns, one byte a vertical strip of 8 pixels, LSB on top. The addressing modes
//! (20h: horizontal, vertical, page), the column and page windows (21h, 22h), page-mode start
//! addresses (B0h-B7h, 00h-1Fh), display start line (40h-7Fh), multiplex ratio (A8h: the visible
//! rows), segment and COM remap (A0h/A1h, C0h/C8h), inversion (A6h/A7h), entire-display-on (A4h/A5h)
//! and display on/off (AEh/AFh) are modelled. Accepted without effect: contrast, charge pump,
//! timing, scrolling (26h-2Fh, A3h) and COM pin configuration (so a 128x32 glass wired to every
//! other COM shows rows 0..31 directly, as Adafruit's driver expects with DAh 02h).
use std::sync::{Arc, Mutex};
use esp_periph::i2c::I2cDevice;

pub const SSD1306_ADDRESS: u8 = 0x3C;

/// GDDRAM and the display state, shared between the controller and the board that shows it.
pub struct Ssd1306State { pub gram: [[u8; 128]; 8], pub on: bool, pub invert: bool, pub all_on: bool, pub rows: u8, pub start_line: u8, pub seg_remap: bool, pub com_reverse: bool, pub version: u64 }
impl Default for Ssd1306State {
    fn default() -> Self { Ssd1306State { gram: [[0; 128]; 8], on: false, invert: false, all_on: false, rows: 64, start_line: 0, seg_remap: false, com_reverse: false, version: 0 } }
}
impl Ssd1306State {
    /// The glass as RGB565 (`width` x visible rows), white on black; dark while the display is off.
    /// Modules wire SEG127 to the glass's left column and COM0 to its bottom row, so the remaps
    /// drivers set (A1h, C8h: Adafruit_SSD1306, U8g2's default rotation) show GDDRAM upright.
    pub fn rgb565(&self, width: usize) -> Vec<u16> {
        let rows = self.rows as usize;
        let mut px = vec![0u16; width * rows];
        if !self.on { return px; }
        for y in 0..rows {
            let com = if self.com_reverse { y } else { rows - 1 - y };
            let line = (com + self.start_line as usize) & 63;
            for x in 0..width.min(128) {
                let seg = if self.seg_remap { x } else { 127 - x };
                let lit = self.all_on || (self.gram[line / 8][seg] >> (line % 8)) & 1 != 0;
                if lit != self.invert { px[y * width + x] = 0xffff; }
            }
        }
        px
    }
}
pub type Ssd1306Shared = Arc<Mutex<Ssd1306State>>;

pub struct Ssd1306 {
    state: Ssd1306Shared,
    /// Next byte: a control byte (`None`), or one or more of data (`Some(true)`) / commands (`Some(false)`).
    kind: Option<bool>, single: bool,
    cmd: Vec<u8>,
    mode: u8, col: u8, page: u8, cols: (u8, u8), pages: (u8, u8),
    pub data_bytes: u64,
}
impl Ssd1306 {
    pub fn new(state: Ssd1306Shared) -> Self { Ssd1306 { state, kind: None, single: false, cmd: Vec::new(), mode: 2, col: 0, page: 0, cols: (0, 127), pages: (0, 7), data_bytes: 0 } }
    /// Argument bytes the command `c` takes.
    fn args(c: u8) -> usize {
        match c {
            0x20 | 0x81 | 0x8d | 0xa8 | 0xd3 | 0xd5 | 0xd9 | 0xda | 0xdb => 1,
            0x21 | 0x22 | 0xa3 => 2,
            0x29 | 0x2a => 5,
            0x26 | 0x27 => 6,
            _ => 0,
        }
    }
    fn command(&mut self, b: u8) {
        self.cmd.push(b);
        if self.cmd.len() <= Self::args(self.cmd[0]) { return; }
        let c = std::mem::take(&mut self.cmd);
        let mut s = self.state.lock().unwrap();
        match c[0] {
            0x00..=0x0f => self.col = (self.col & 0xf0) | c[0],
            0x10..=0x1f => self.col = (self.col & 0x0f) | (c[0] & 0x07) << 4,
            0x20 => self.mode = c[1] & 3,
            0x21 => { self.cols = (c[1] & 0x7f, c[2] & 0x7f); self.col = self.cols.0; }
            0x22 => { self.pages = (c[1] & 7, c[2] & 7); self.page = self.pages.0; }
            0x40..=0x7f => s.start_line = c[0] & 0x3f,
            0xa0 | 0xa1 => s.seg_remap = c[0] & 1 != 0,
            0xa4 | 0xa5 => s.all_on = c[0] & 1 != 0,
            0xa6 | 0xa7 => s.invert = c[0] & 1 != 0,
            0xa8 => { let m = c[1] & 0x3f; if m >= 15 { s.rows = m + 1; } }
            0xae | 0xaf => s.on = c[0] & 1 != 0,
            0xb0..=0xb7 => self.page = c[0] & 7,
            0xc0 | 0xc8 => s.com_reverse = c[0] & 8 != 0,
            _ => return,
        }
        s.version += 1;
    }
    fn data(&mut self, b: u8) {
        let mut s = self.state.lock().unwrap();
        s.gram[self.page as usize & 7][self.col as usize & 127] = b;
        s.version += 1;
        self.data_bytes += 1;
        let (c0, c1, p0, p1) = (self.cols.0, self.cols.1.max(self.cols.0), self.pages.0, self.pages.1.max(self.pages.0));
        match self.mode {
            0 => { if self.col >= c1 { self.col = c0; self.page = if self.page >= p1 { p0 } else { self.page + 1 }; } else { self.col += 1; } }
            1 => { if self.page >= p1 { self.page = p0; self.col = if self.col >= c1 { c0 } else { self.col + 1 }; } else { self.page += 1; } }
            _ => { self.col = if self.col >= 127 { 0 } else { self.col + 1 }; }   // page mode: wraps within the page
        }
    }
}
impl I2cDevice for Ssd1306 {
    fn start(&mut self, read: bool) -> bool { self.kind = None; self.single = false; !read }
    fn write(&mut self, b: u8) -> bool {
        match self.kind {
            None => { self.kind = Some(b & 0x40 != 0); self.single = b & 0x80 != 0; }
            Some(data) => {
                if data { self.data(b) } else { self.command(b) }
                if self.single { self.kind = None; }
            }
        }
        true
    }
    fn read(&mut self) -> u8 { 0xff }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn send(d: &mut Ssd1306, bytes: &[u8]) { assert!(d.start(false)); for &b in bytes { assert!(d.write(b)); } d.stop(); }

    #[test]
    fn adafruit_init_and_a_horizontal_frame_reach_the_glass() {
        let state: Ssd1306Shared = Default::default();
        let mut d = Ssd1306::new(state.clone());
        // Adafruit_SSD1306 2.5 begin() for 128x32: display off, clock, multiplex 31, offset, start line,
        // charge pump, horizontal addressing, remaps, COM pins 02h, contrast, precharge, Vcomh, display on
        send(&mut d, &[0x00, 0xae, 0xd5, 0x80, 0xa8, 0x1f, 0xd3, 0x00, 0x40, 0x8d, 0x14, 0x20, 0x00, 0xa1, 0xc8]);
        send(&mut d, &[0x00, 0xda, 0x02, 0x81, 0x8f, 0xd9, 0xf1, 0xdb, 0x40, 0xa4, 0xa6, 0x2e, 0xaf]);
        // display(): page and column windows, then the 512 data bytes in chunks
        send(&mut d, &[0x00, 0x22, 0x00, 0xff, 0x21, 0x00, 0x7f]);
        let mut frame = vec![0u8; 512];
        frame[0] = 0x01;          // page 0, column 0: top pixel
        frame[128 + 5] = 0x80;    // page 1, column 5: row 15
        for chunk in frame.chunks(31) { let mut t = vec![0x40]; t.extend_from_slice(chunk); send(&mut d, &t); }
        let s = state.lock().unwrap();
        assert!(s.on);
        assert_eq!(s.rows, 32);
        assert_eq!((s.gram[0][0], s.gram[1][5]), (0x01, 0x80));
        let px = s.rgb565(128);
        assert_eq!(px.len(), 128 * 32);
        // A1h and C8h show GDDRAM upright: (0, 0) at the top left, column 5 row 15 at (5, 15)
        assert_eq!(px[0], 0xffff);
        assert_eq!(px[15 * 128 + 5], 0xffff);
        assert_eq!(px.iter().filter(|&&p| p != 0).count(), 2);
    }

    #[test]
    fn single_control_bytes_and_page_mode() {
        let state: Ssd1306Shared = Default::default();
        let mut d = Ssd1306::new(state.clone());
        // Co set: command, then data, each behind its own control byte
        send(&mut d, &[0x80, 0xb2, 0x80, 0x03, 0x80, 0x11, 0xc0, 0xaa]);
        assert_eq!(state.lock().unwrap().gram[2][0x13], 0xaa);
        assert!(!d.start(true), "the controller has nothing to read over I2C");
    }
}
