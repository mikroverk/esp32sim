//! SSD1306 Rev 1.1, sections 8.1.5 and 10.1; IDF 6.0.1 esp_lcd_panel_ssd1306.c.
//! https://cdn-shop.adafruit.com/datasheets/SSD1306.pdf
use esp_periph::i2c::I2cDevice;
use std::sync::{Arc, Mutex};

pub struct Ssd1306State {
    pub ram: [u8; 1024], pub version: u64, pub frames: u64,
    on: bool, inverse: bool, entire: bool, mirror_x: bool, mirror_y: bool,
    start_line: u8, offset: u8, multiplex: u8, mode: u8,
    column: u8, page: u8, columns: [u8; 2], pages: [u8; 2],
    command: u8, need: u8, len: u8, params: [u8; 2],
}
impl Default for Ssd1306State {
    fn default() -> Self {
        Self { ram: [0; 1024], version: 0, frames: 0, on: false, inverse: false, entire: false,
            mirror_x: false, mirror_y: false, start_line: 0, offset: 0, multiplex: 63, mode: 2,
            column: 0, page: 0, columns: [0, 127], pages: [0, 7], command: 0, need: 0, len: 0, params: [0; 2] }
    }
}
impl Ssd1306State {
    pub fn pixels(&self) -> Vec<u16> {
        (0..8192).map(|i| {
            let (x, y) = (i % 128, i / 128);
            let column = if self.mirror_x { 127 - x } else { x };
            let row = if self.mirror_y { 63 - y } else { y };
            let row = (row + 64 + self.start_line as usize - self.offset as usize) % 64;
            let bit = self.ram[row / 8 * 128 + column] & (1 << (row % 8)) != 0;
            let visible = self.on && y <= self.multiplex as usize && (self.entire || (bit ^ self.inverse));
            if visible { 0xffff } else { 0 }
        }).collect()
    }
    fn byte(&mut self, byte: u8, data: bool) {
        if data {
            let index = self.page as usize * 128 + self.column as usize;
            if self.ram[index] != byte { self.ram[index] = byte; self.version += 1; }
            if self.mode == 1 {
                if self.page >= self.pages[1] {
                    self.page = self.pages[0];
                    self.column = if self.column >= self.columns[1] { self.columns[0] } else { self.column + 1 };
                } else { self.page += 1; }
            } else if self.column >= self.columns[1] {
                self.column = self.columns[0];
                if self.mode == 0 { self.page = if self.page >= self.pages[1] { self.pages[0] } else { self.page + 1 }; }
            } else { self.column += 1; }
            return;
        }
        if self.need != 0 {
            self.params[self.len as usize] = byte; self.len += 1;
            if self.len != self.need { return; }
            self.need = 0;
            match self.command {
                0x20 => self.mode = self.params[0] & 3,
                0x21 => { self.columns = [self.params[0] & 127, self.params[1] & 127]; self.column = self.columns[0]; }
                0x22 => { self.pages = [self.params[0] & 7, self.params[1] & 7]; self.page = self.pages[0]; }
                0xa8 => self.multiplex = self.params[0] & 63,
                0xd3 => self.offset = self.params[0] & 63,
                // shortcut: analog drive settings do not affect binary pixels; add brightness if required.
                _ => {}
            }
        } else {
            self.command = byte; self.len = 0;
            match byte {
                0x00..=0x0f => self.column = (self.column & 0x70) | (byte & 15),
                0x10..=0x1f => self.column = (self.column & 15) | ((byte & 7) << 4),
                0x20 | 0x81 | 0x8d | 0xa8 | 0xd3 | 0xd5 | 0xd9 | 0xda | 0xdb => self.need = 1,
                0x21 | 0x22 => self.need = 2,
                0x40..=0x7f => self.start_line = byte & 63,
                0xa0 | 0xa1 => self.mirror_x = byte == 0xa1,
                0xa4 | 0xa5 => self.entire = byte == 0xa5,
                0xa6 | 0xa7 => self.inverse = byte == 0xa7,
                0xae | 0xaf => self.on = byte == 0xaf,
                0xb0..=0xb7 => self.page = byte & 7,
                0xc0 | 0xc8 => self.mirror_y = byte == 0xc8,
                _ => {}
            }
        }
        self.version += 1;
    }
}

pub struct Ssd1306 {
    state: Arc<Mutex<Ssd1306State>>, pins: (u8, u8),
    control: bool, data: bool, continuation: bool, wrote_data: bool,
}
impl Ssd1306 {
    pub fn new(state: Arc<Mutex<Ssd1306State>>, pins: (u8, u8)) -> Self {
        Self { state, pins, control: true, data: false, continuation: false, wrote_data: false }
    }
}
impl I2cDevice for Ssd1306 {
    fn pins(&self) -> Option<(u8, u8)> { Some(self.pins) }
    fn start(&mut self, read: bool) -> bool { self.control = true; self.wrote_data = false; !read }
    fn write(&mut self, byte: u8) -> bool {
        if self.control {
            self.data = byte & 0x40 != 0; self.continuation = byte & 0x80 != 0; self.control = false;
        } else {
            self.state.lock().unwrap().byte(byte, self.data);
            self.wrote_data |= self.data;
            self.control = self.continuation;
        }
        true
    }
    fn read(&mut self) -> u8 { 0xff }
    fn stop(&mut self) { if self.wrote_data { self.state.lock().unwrap().frames += 1; self.wrote_data = false; } }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tx(device: &mut Ssd1306, bytes: &[u8]) {
        assert!(device.start(false)); for &byte in bytes { assert!(device.write(byte)); } device.stop();
    }
    #[test]
    fn address_modes_control_bytes_and_visible_pixels() {
        let state = Arc::new(Mutex::new(Ssd1306State::default()));
        let mut device = Ssd1306::new(state.clone(), (41, 42));
        assert!(!device.start(true)); assert_eq!(device.pins(), Some((41, 42)));
        tx(&mut device, &[0, 0xaf, 0x20, 0, 0x21, 2, 3, 0x22, 1, 2]);
        tx(&mut device, &[0x40, 1, 2, 4, 8]);
        { let s = state.lock().unwrap();
            assert_eq!([s.ram[130], s.ram[131], s.ram[258], s.ram[259]], [1, 2, 4, 8]);
            assert_eq!(s.pixels()[8 * 128 + 2], 0xffff);
            assert_eq!(s.pixels()[8 * 128 + 3], 0);
        }
        tx(&mut device, &[0x80, 0xa1, 0, 0xc8]);
        assert_eq!(state.lock().unwrap().pixels()[55 * 128 + 125], 0xffff);
        tx(&mut device, &[0, 0xae]);
        assert!(state.lock().unwrap().pixels().iter().all(|&p| p == 0));
        tx(&mut device, &[0, 0x20, 1, 0x21, 4, 5, 0x22, 0, 1]);
        tx(&mut device, &[0xc0, 11, 0xc0, 22, 0xc0, 33, 0x40, 44]);
        { let s = state.lock().unwrap(); assert_eq!([s.ram[4], s.ram[132], s.ram[5], s.ram[133]], [11, 22, 33, 44]); }
        tx(&mut device, &[0, 0x20, 2, 0xb3, 0x07, 0x10]);
        tx(&mut device, &[0x40, 0x55]);
        assert_eq!(state.lock().unwrap().ram[3 * 128 + 7], 0x55);
        tx(&mut device, &[0, 0xaf, 0xa5]);
        assert!(state.lock().unwrap().pixels().iter().all(|&p| p == 0xffff));
        tx(&mut device, &[0, 0xa7]);
        assert!(state.lock().unwrap().pixels().iter().all(|&p| p == 0xffff));
        tx(&mut device, &[0, 0xa4]);
        assert_eq!(state.lock().unwrap().pixels().iter().filter(|&&p| p == 0xffff).count(), 8192 - 19);
    }
}
