//! A TI INA219 current/power monitor on I2C, from its datasheet (SBOS448G, section 8.6): six 16-bit
//! big-endian registers behind a pointer (the first byte of a write; later bytes are MSB then LSB),
//! read from the pointer. CONFIGURATION 0x00 (reset 0x399F, RST bit 15), SHUNT_VOLTAGE 0x01 (signed,
//! 10 uV LSB), BUS_VOLTAGE 0x02 (bits 15:3 at 4 mV, CNVR bit 1, OVF bit 0), POWER 0x03 (20 current
//! LSBs; current * bus / 5000), CURRENT 0x04 (shunt * calibration / 4096) and CALIBRATION 0x05 (bit 0
//! reads 0). Conversions are continuous and always complete: CNVR reads set. The board sets the bus
//! voltage and the current through the shunt; the PGA range clamps the shunt reading (OVF is not
//! raised by math overflow). Not modelled: averaging, triggered and power-down modes, conversion time.
use std::sync::{Arc, Mutex};
use esp_periph::i2c::I2cDevice;

/// What the monitored circuit shows now: bus volts, shunt amps, shunt ohms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ina219Input { pub bus_v: f64, pub current_a: f64, pub shunt_ohm: f64 }
pub type Ina219Shared = Arc<Mutex<Ina219Input>>;

pub struct Ina219 { config: u16, calibration: u16, ptr: u8, bytes: u8, msb: u8, input: Ina219Shared, out: u16, pub reads: u64 }
impl Ina219 {
    pub fn new(input: Ina219Shared) -> Self { Ina219 { config: 0x399f, calibration: 0, ptr: 0, bytes: 0, msb: 0, input, out: 0, reads: 0 } }
    /// The shunt register: 10 uV per LSB, clamped to the PGA's range (CONFIG bits 12:11: 40, 80, 160, 320 mV).
    fn shunt(&self) -> i16 {
        let i = *self.input.lock().unwrap();
        let limit = 4000i32 << ((self.config >> 11) & 3);
        let raw = (i.current_a * i.shunt_ohm / 10e-6).round();
        (if raw.is_finite() { raw as i64 } else { 0 }).clamp(-(limit as i64), limit as i64) as i16
    }
    fn current(&self) -> i16 { (i32::from(self.shunt()) * i32::from(self.calibration) / 4096).clamp(i16::MIN.into(), i16::MAX.into()) as i16 }
    fn register(&self, r: u8) -> u16 {
        match r {
            0 => self.config,
            1 => self.shunt() as u16,
            2 => {
                let v = self.input.lock().unwrap().bus_v;
                // BRNG (bit 13) selects 16 or 32 V full scale; the field holds up to 8191 * 4 mV
                let mv = if v.is_finite() { (v * 1000.0).round().clamp(0.0, 32760.0) as u32 } else { 0 };
                let full = if self.config & (1 << 13) != 0 { 32000 } else { 16000 };
                ((mv.min(full) / 4) as u16) << 3 | 2
            }
            3 => {
                let bus = (self.register(2) >> 3) as i64;   // 4 mV units
                (i64::from(self.current()).abs() * bus / 5000).min(0xffff) as u16
            }
            4 => self.current() as u16,
            5 => self.calibration,
            _ => 0,
        }
    }
}
impl I2cDevice for Ina219 {
    fn start(&mut self, read: bool) -> bool {
        if read { self.out = self.register(self.ptr); self.bytes = 0; } else { self.bytes = 0; }
        true
    }
    fn write(&mut self, b: u8) -> bool {
        match self.bytes {
            0 => self.ptr = b,
            1 => self.msb = b,
            _ => {
                let v = u16::from(self.msb) << 8 | u16::from(b);
                match self.ptr {
                    0 => { self.config = if v & 0x8000 != 0 { self.calibration = 0; 0x399f } else { v }; }
                    5 => self.calibration = v & 0xfffe,
                    _ => {}
                }
            }
        }
        self.bytes = self.bytes.saturating_add(1);
        true
    }
    fn read(&mut self) -> u8 {
        let b = if self.bytes & 1 == 0 { (self.out >> 8) as u8 } else { self.out as u8 };
        self.bytes = self.bytes.wrapping_add(1);
        self.reads += 1;
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reg(d: &mut Ina219, r: u8) -> u16 {
        d.start(false); d.write(r); d.start(true);
        let hi = d.read(); let lo = d.read();
        u16::from(hi) << 8 | u16::from(lo)
    }
    fn set(d: &mut Ina219, r: u8, v: u16) { d.start(false); d.write(r); d.write((v >> 8) as u8); d.write(v as u8); }

    #[test]
    fn bus_shunt_current_and_power_follow_the_datasheet() {
        let input = Arc::new(Mutex::new(Ina219Input { bus_v: 12.0, current_a: 0.5, shunt_ohm: 0.01 }));
        let mut d = Ina219::new(input.clone());
        assert_eq!(reg(&mut d, 0), 0x399f);
        assert_eq!(reg(&mut d, 2), (3000 << 3) | 2, "12 V at 4 mV per LSB, CNVR set");
        assert_eq!(reg(&mut d, 1), 500, "5 mV across 0.01 ohm at 10 uV per LSB");
        assert_eq!(reg(&mut d, 4), 0, "no calibration, no current");
        set(&mut d, 5, 4096);
        assert_eq!(reg(&mut d, 5), 4096);
        assert_eq!(reg(&mut d, 4), 500, "shunt * cal / 4096");
        assert_eq!(reg(&mut d, 3), 300, "current * bus / 5000");
        input.lock().unwrap().current_a = -0.2;
        assert_eq!(reg(&mut d, 1) as i16, -200);
        input.lock().unwrap().current_a = 100.0;
        set(&mut d, 0, 0x0000);   // PGA /1: 40 mV
        assert_eq!(reg(&mut d, 1), 4000, "clamped to the PGA range");
        set(&mut d, 0, 0x8000);
        assert_eq!((reg(&mut d, 0), reg(&mut d, 5)), (0x399f, 0), "RST restores the defaults");
    }
}
