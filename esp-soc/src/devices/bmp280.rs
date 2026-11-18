//! A Bosch BMP280 barometric pressure and temperature sensor on I2C (0x76 or 0x77), as
//! esp-idf-lib's `bmp280` component drives it: chip ID 0x58 at 0xD0, 24 bytes of calibration at
//! 0x88 (T1..T3 then P1..P9, little-endian 16-bit), 20-bit raw pressure at 0xF7 and temperature
//! at 0xFA, each three bytes MSB-first with the low nibble of the third byte unused. `ctrl_meas`
//! at 0xF4 starts a measurement and `status` at 0xF3 bit 3 reads back as measuring.
//!
//! The compensation below is the datasheet's integer path verbatim, so the model and the driver
//! agree to the last digit. The calibration words are the datasheet's worked example, and the
//! default raw values are the ones it compensates to 25.08 °C / 100653 Pa — `compensate` is
//! asserted against those, which is what makes the model trustworthy rather than merely plausible.
use esp_periph::i2c::I2cDevice;

/// SDO strapped high (the common module default). SDO low is 0x76.
pub const BMP280_ADDRESS: u8 = 0x77;

const CHIP_ID: u8 = 0x58;
const REG_CALIB: u8 = 0x88;
const REG_ID: u8 = 0xD0;
const REG_STATUS: u8 = 0xF3;
const REG_CTRL_MEAS: u8 = 0xF4;
const REG_PRESS: u8 = 0xF7;
const REG_TEMP: u8 = 0xFA;

/// Datasheet worked-example calibration: T1,T2,T3 then P1..P9 at 0x88, one little-endian word each.
/// The signed entries are stored as their 16-bit two's complement, exactly as the sensor holds them.
const CALIBRATION: [u16; 12] = [
    27504,  // 0x88 T1
    26435,  // 0x8A T2
    64536,  // 0x8C T3 = -1000
    36477,  // 0x8E P1
    54851,  // 0x90 P2 = -10685
    3024,   // 0x92 P3
    2855,   // 0x94 P4
    140,    // 0x96 P5
    65529,  // 0x98 P6 = -7
    15500,  // 0x9A P7
    50936,  // 0x9C P8 = -14600
    6000,   // 0x9E P9
];

/// Raw ADC values the datasheet compensates to 25.08 °C and 100653 Pa.
const ADC_T: u32 = 519888;
const ADC_P: u32 = 415148;

pub struct Bmp280 {
    regs: [u8; 256],
    ptr: u8,
    first: bool,
    /// Raw 20-bit samples a forced measurement publishes at 0xF7/0xFA.
    raw_t: u32,
    raw_p: u32,
    pub reads: u64,
}

impl Default for Bmp280 { fn default() -> Self { Self::new() } }

impl Bmp280 {
    pub fn new() -> Self {
        let mut regs = [0u8; 256];
        for (i, &w) in CALIBRATION.iter().enumerate() {
            let r = REG_CALIB as usize + 2 * i;
            regs[r] = w as u8;
            regs[r + 1] = (w >> 8) as u8;
        }
        regs[REG_ID as usize] = CHIP_ID;
        let mut d = Bmp280 { regs, ptr: 0, first: true, raw_t: ADC_T, raw_p: ADC_P, reads: 0 };
        d.publish();
        d
    }

    /// Replace what the next forced measurement reports. `publish` makes it visible immediately,
    /// so a driver that skips the status poll still reads a sane sample.
    pub fn set_raw(&mut self, adc_t: u32, adc_p: u32) {
        self.raw_t = adc_t & 0x000F_FFFF;
        self.raw_p = adc_p & 0x000F_FFFF;
        self.publish();
    }

    /// Write the latched samples into the data registers, MSB first, low nibble of xlsb clear.
    fn publish(&mut self) {
        let p = self.raw_p;
        let t = self.raw_t;
        self.regs[REG_PRESS as usize] = (p >> 12) as u8;
        self.regs[REG_PRESS as usize + 1] = (p >> 4) as u8;
        self.regs[REG_PRESS as usize + 2] = ((p & 0x0F) << 4) as u8;
        self.regs[REG_TEMP as usize] = (t >> 12) as u8;
        self.regs[REG_TEMP as usize + 1] = (t >> 4) as u8;
        self.regs[REG_TEMP as usize + 2] = ((t & 0x0F) << 4) as u8;
    }

    /// Calibration word at `reg`, little-endian, as the raw 16 bits the sensor holds.
    fn word(&self, reg: u8) -> u16 {
        let r = reg as usize;
        self.regs[r] as u16 | (self.regs[r + 1] as u16) << 8
    }

    /// A signed calibration word. T1 and P1 are unsigned; everything else is two's complement.
    fn sword(&self, reg: u8) -> i32 { self.word(reg) as i16 as i32 }

    /// Read a 20-bit sample back out of the data registers the way the driver assembles it.
    fn sample(&self, reg: u8) -> i32 {
        let r = reg as usize;
        ((self.regs[r] as i32) << 12) | ((self.regs[r + 1] as i32) << 4) | ((self.regs[r + 2] as i32) >> 4)
    }

    /// Temperature in °C and pressure in Pa, by the datasheet's integer path.
    pub fn compensate(&self) -> (f32, f32) {
        let adc_t = self.sample(REG_TEMP);
        let adc_p = self.sample(REG_PRESS);

        let dig_t1 = self.word(0x88) as i32;
        let dig_t2 = self.sword(0x8A);
        let dig_t3 = self.sword(0x8C);
        let var1 = (((adc_t >> 3) - (dig_t1 << 1)) * dig_t2) >> 11;
        let var2 = (((((adc_t >> 4) - dig_t1) * ((adc_t >> 4) - dig_t1)) >> 12) * dig_t3) >> 14;
        let t_fine = var1 + var2;
        let t = ((t_fine * 5 + 128) >> 8) as f32 / 100.0;

        let dig_p1 = self.word(0x8E) as i64;
        let dig_p2 = self.sword(0x90) as i64;
        let dig_p3 = self.sword(0x92) as i64;
        let dig_p4 = self.sword(0x94) as i64;
        let dig_p5 = self.sword(0x96) as i64;
        let dig_p6 = self.sword(0x98) as i64;
        let dig_p7 = self.sword(0x9A) as i64;
        let dig_p8 = self.sword(0x9C) as i64;
        let dig_p9 = self.sword(0x9E) as i64;

        let var1 = t_fine as i64 - 128000;
        let var2 = var1 * var1 * dig_p6;
        let var2 = var2 + ((var1 * dig_p5) << 17);
        let var2 = var2 + (dig_p4 << 35);
        let mut var1 = ((var1 * var1 * dig_p3) >> 8) + ((var1 * dig_p2) << 12);
        var1 = (((1i64 << 47) + var1) * dig_p1) >> 33;
        if var1 == 0 { return (t, 0.0); }
        let mut p = 1048576 - adc_p as i64;
        p = (((p << 31) - var2) * 3125) / var1;
        let var1 = (dig_p9 * (p >> 13) * (p >> 13)) >> 25;
        let var2 = (dig_p8 * p) >> 19;
        p = ((p + var1 + var2) >> 8) + (dig_p7 << 4);
        (t, p as f32 / 256.0)
    }
}

impl I2cDevice for Bmp280 {
    fn start(&mut self, read: bool) -> bool {
        if !read { self.first = true; }
        true
    }
    fn write(&mut self, b: u8) -> bool {
        if self.first {
            self.ptr = b;
            self.first = false;
            return true;
        }
        let r = self.ptr;
        self.regs[r as usize] = b;
        if r == REG_CTRL_MEAS && b != 0 {
            // Forced-mode trigger: measuring until a driver looks at status. The conversion is not
            // timed (a 2 ms wait would only pin the run to wall clock); the first status read
            // finishes it and publishes the samples.
            self.regs[REG_STATUS as usize] |= 0x08;
        }
        self.ptr = r.wrapping_add(1);
        true
    }
    fn read(&mut self) -> u8 {
        let r = self.ptr;
        let v = self.regs[r as usize];
        if r == REG_STATUS {
            self.reads += 1;
            if v & 0x08 != 0 {
                self.regs[REG_STATUS as usize] &= !0x08;
                self.publish();
            }
        }
        self.ptr = r.wrapping_add(1);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn read_at(d: &mut Bmp280, reg: u8, n: usize) -> Vec<u8> {
        d.start(false); d.write(reg); d.start(true);
        (0..n).map(|_| d.read()).collect()
    }

    #[test]
    fn chip_id_and_calibration_read_back() {
        let mut d = Bmp280::new();
        assert_eq!(read_at(&mut d, REG_ID, 1), [CHIP_ID]);
        // T1 at 0x88 comes back as the datasheet's 27504, little-endian.
        assert_eq!(read_at(&mut d, 0x88, 2), [0x70, 0x6B]);
        // T3 is signed: -1000 stored as 0xFC18.
        assert_eq!(read_at(&mut d, 0x8C, 2), [0x18, 0xFC]);
    }

    #[test]
    fn default_samples_compensate_to_the_datasheet_example() {
        let d = Bmp280::new();
        let (t, p) = d.compensate();
        assert!((t - 25.08).abs() < 0.05, "temperature {t}");
        assert!((p - 100_653.0).abs() < 20.0, "pressure {p}");
    }

    #[test]
    fn status_poll_finishes_measurement_then_data_is_contiguous() {
        let mut d = Bmp280::new();
        // Forced mode, oversampling x1, as esp-idf-lib writes it.
        d.start(false); d.write(REG_CTRL_MEAS); d.write(0b0110_0001);
        assert_eq!(read_at(&mut d, REG_STATUS, 1)[0] & 0x08, 0x08);
        assert_eq!(read_at(&mut d, REG_STATUS, 1)[0] & 0x08, 0x00);
        // The driver reads six bytes from 0xF7 and splits them pressure 0..3 / temperature 3..6.
        let six = read_at(&mut d, REG_PRESS, 6);
        let p = ((six[0] as i32) << 12) | ((six[1] as i32) << 4) | ((six[2] as i32) >> 4);
        let t = ((six[3] as i32) << 12) | ((six[4] as i32) << 4) | ((six[5] as i32) >> 4);
        assert_eq!((t, p), (ADC_T as i32, ADC_P as i32));
    }

    #[test]
    fn a_different_sample_changes_the_reported_value() {
        let mut d = Bmp280::new();
        let (t0, _) = d.compensate();
        d.set_raw(ADC_T - 200_000, ADC_P);
        let (t1, _) = d.compensate();
        assert!(t1 < t0, "cooler raw sample should read cooler: {t0} -> {t1}");
    }
}
