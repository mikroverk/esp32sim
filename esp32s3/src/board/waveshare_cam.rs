use esp_soc::board::BoardModel;
use esp_soc::picture;

/// Waveshare ESP32-S3-CAM-OV5640: OV5640 on the LCD_CAM DVP port (SCCB on I2C0 GPIO 7/8),
/// CH32V003 IO expander, ES8311 speaker codec + ES7210 mic ADC on I2C0, audio on I2S0
/// (MCLK 10, BCLK 11, LRCLK 12, DIN 13, DOUT 14), buttons GPIO 0 / 15.
pub struct WaveshareCam { pub gpio_events: u64, pub preview_dirty: bool, sensor: std::sync::Arc<std::sync::Mutex<crate::i2c::SensorState>>, picture: Option<picture::Picture>, frame: Option<(u32, u32, std::sync::Arc<Vec<u8>>)>, pub frames: u64, frame_config: Option<crate::i2c::SensorState> }
impl Default for WaveshareCam { fn default() -> Self { Self::new() } }

impl WaveshareCam { pub fn new() -> Self { WaveshareCam { gpio_events: 0, preview_dirty: false, sensor: Default::default(), picture: None, frame: None, frames: 0, frame_config: None } } }
impl BoardModel for WaveshareCam {
    fn name(&self) -> &'static str { "waveshare-cam" }
    fn gpio_changes(&mut self, changes: &[(u8, bool)]) { self.gpio_events += changes.len() as u64; }
    fn gpio_events(&self) -> u64 { self.gpio_events }
    fn set_camera_picture(&mut self, p: picture::Picture) -> Result<(), &'static str> {
        if !p.valid() { return Err("invalid camera picture or frame exceeds 8 MiB"); }
        self.picture = Some(p); self.frame = None; self.preview_dirty = true; Ok(())
    }

    fn camera_preview(&self, w: u32, h: u32) -> Option<Vec<u8>> {
        let p = self.picture.as_ref()?;
        let mut out = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h { let sy = (y as u64 * p.h as u64 / h as u64) as usize; for x in 0..w { let sx = (x as u64 * p.w as u64 / w as u64) as usize; let o = (sy * p.w as usize + sx) * 3; out.extend_from_slice(&p.rgb[o..o + 3]); } }
        Some(out)
    }
    fn camera_frame(&mut self) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> {
        let config = *self.sensor.lock().unwrap();
        let (w, h) = (config.width, config.height);
        if w == 0 || h == 0 || !config.streaming { return None; }
        if self.frame.is_none() || self.frame_config != Some(config) {
            let black;
            let p = match self.picture.as_ref() {
                Some(p) => p,
                None => { black = picture::Picture { w: 1, h: 1, rgb: vec![0; 3] }; &black }
            };
            // Window coordinates cover the 2624 x 1952 array in esp32-camera v2.1.4's ratio table.
            let cropped;
            let p = if let Some(window) = config.window {
                cropped = picture::crop(p, window, 2624, 1952)?;
                &cropped
            } else { p };
            // esp32-camera v2.1.4 ov5640_settings.h FORMAT_CTRL00 (0x4300).
            let format = match (config.jpeg, config.format) {
                (false, 0x61) => Some(picture::PixelFormat::Rgb565),
                (false, 0x30) if w.is_multiple_of(2) => Some(picture::PixelFormat::Yuyv),
                (false, 0x10) => Some(picture::PixelFormat::Grayscale),
                _ => None,
            };
            let Some(format) = format else {
                if self.frame_config != Some(config) { eprintln!("[camera] unsupported sensor output: format {:#x}, JPEG {}, {}x{}", config.format, config.jpeg, w, h); }
                self.frame = None; self.frame_config = Some(config); return None;
            };
            let data = picture::sensor_frame(p, w, h, format)?;
            self.frame = Some((w, h, std::sync::Arc::new(data)));
            self.frame_config = Some(config);
        }
        self.frames += 1;
        self.frame.clone()
    }
    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn crate::i2c::I2cDevice>)> {
        use crate::i2c::*;
        vec![
            (0, 0x24, Box::new(Ch32v003::new())),
            (0, 0x3c, Box::new(Ov5640::new(self.sensor.clone()))),
            (0, 0x18, Box::new(Reg8Device::new("es8311", &[(0xfd, 0x83), (0xfe, 0x11)]))),
            (0, 0x40, Box::new(Reg8Device::new("es7210", &[(0x3d, 0x72), (0x3e, 0x10)]))),
        ]
    }
}
