//! Classic ESP32 adapter for the shared I2C controller.
use esp_periph::device::{Device, WriteEffect};
use esp_periph::i2c::{I2cDevice, INT_TIMEOUT};

pub struct I2c {
    pub inner: esp_periph::i2c::I2c,
    lines_blocked: bool,
}

impl I2c {
    pub fn new() -> Self {
        Self { inner: esp_periph::i2c::I2c::new_classic(), lines_blocked: false }
    }
    pub fn attach(&mut self, addr: u8, device: Box<dyn I2cDevice>) { self.inner.attach(addr, device); }
    pub fn has_device(&self, addr: u8) -> bool { self.inner.has_device(addr) }
    pub fn set_lines(&mut self, scl: Option<bool>, sda: Option<bool>) {
        self.lines_blocked = scl != Some(true) || sda != Some(true);
    }
}

impl Default for I2c { fn default() -> Self { Self::new() } }

impl Device for I2c {
    fn read(&mut self, off: u32) -> u32 { self.inner.read(off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        if off == 0x04 && v & (1 << 5) != 0 && self.lines_blocked {
            self.inner.write(off, v & !(1 << 5));
            self.inner.int_raw |= INT_TIMEOUT;
        } else {
            self.inner.write(off, v);
        }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 { self.inner.irq() as u64 }
    fn debug(&mut self, on: bool) { self.inner.log = on; }
}
