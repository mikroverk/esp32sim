use esp_soc::board::BoardModel;
use esp_soc::devices::ssd1306::{Ssd1306, Ssd1306State};
use std::sync::{Arc, Mutex};

/// PAO voice board: SSD1306 on I2C0 SDA41/SCL42, status LED on GPIO48.
#[derive(Default)]
pub struct Pao {
    display: Arc<Mutex<Ssd1306State>>,
    led: [[u8; 3]; 1], updates: u64, edges: u64,
}
impl BoardModel for Pao {
    fn name(&self) -> &'static str { "pao" }
    fn gpio_changes(&mut self, changes: &[(u8, bool)]) {
        self.edges += changes.len() as u64;
        for &(pin, high) in changes { if pin == 48 { self.led[0] = [if high { 255 } else { 0 }; 3]; self.updates += 1; } }
    }
    fn gpio_events(&self) -> u64 { self.edges }
    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn esp_periph::i2c::I2cDevice>)> {
        vec![(0, 0x3c, Box::new(Ssd1306::new(self.display.clone(), (41, 42))))]
    }
    fn display(&self) -> Option<(u32, u32, Vec<u16>, u64)> {
        let state = self.display.lock().unwrap();
        // The module's visible origin is opposite the controller's SEG0/COM0 origin.
        Some((128, 64, state.pixels().into_iter().rev().collect(), state.version))
    }
    fn display_version(&self) -> u64 { self.display.lock().unwrap().version }
    fn display_frames(&self) -> u64 { self.display.lock().unwrap().frames }
    fn leds(&self) -> Option<(&[[u8; 3]], u64)> { Some((&self.led, self.updates)) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oled_writes_reach_the_glass_and_gpio48_drives_the_status_led() {
        let mut board = Pao::default();
        let mut device = board.i2c_devices().remove(0).2;
        assert!(device.start(false));
        for byte in [0, 0xaf, 0xa1, 0xc8] { assert!(device.write(byte)); } device.stop();
        assert!(device.start(false));
        for byte in [0x40, 1] { assert!(device.write(byte)); } device.stop();
        let (_, _, pixels, _) = board.display().unwrap();
        assert_eq!(pixels[0], 0xffff);
        assert_eq!(pixels.iter().filter(|&&p| p != 0).count(), 1);
        assert_eq!(board.display_frames(), 1);
        board.gpio_changes(&[(48, true), (21, true)]);
        assert_eq!(board.leds().unwrap().0, &[[255; 3]]);
        board.gpio_changes(&[(48, false)]);
        assert_eq!(board.leds().unwrap().0, &[[0; 3]]);
    }
}
