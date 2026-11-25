//! A Bosch BMP280 barometric pressure and temperature sensor on I2C (0x76 or 0x77), modelled from
//! datasheet BST-BMP280-DS001 revision 1.26 (October 2021). Section and table numbers below are
//! that revision:
//!
//! - 4.3.1 and Table 18: chip ID 0x58 at 0xD0, the register map, and which registers are read-only.
//! - Table 17: the 24 calibration bytes at 0x88 (T1..T3 then P1..P9, little-endian 16-bit words;
//!   T1 and P1 unsigned, everything else two's complement).
//! - Tables 21, 22, 24 and 25: 20-bit samples at 0xF7 (pressure) and 0xFA (temperature), three
//!   bytes MSB-first with the low nibble of the third byte unused, and 0x80000 for a channel the
//!   measurement skipped.
//! - 3.6.2: `ctrl_meas` at 0xF4 starts a measurement, and a forced one returns to sleep when it
//!   finishes. `status` at 0xF3 bit 3 is the measuring flag; here it only ever reads clear,
//!   because the measurement completes in the write that starts it.
//! - 4.3.2: 0xB6 written to 0xE0 runs the full power-on reset.
//! - 5.2.1: a write transaction is pairs of register address and data.
//! - 3.11.3: the integer compensation path, reproduced verbatim (including its note that the
//!   integer result may deviate slightly from the printed table).
//!
//! The calibration words and the staged raw samples are the datasheet's worked example, which
//! compensates to 25.08 °C / 100653.25 Pa; `compensate` returns those as the integer values the
//! datasheet's own functions return, so the tests assert them exactly rather than within a
//! tolerance. The write sequences esp-idf-lib's `bmp280` component sends are replayed in the
//! tests; Linux's `bmp280-core`, Adafruit's library and Bosch's BME280_SensorAPI were checked
//! against the same register rules but are not replayed.
use esp_periph::i2c::I2cDevice;

/// SDO strapped high (the common module default). SDO low is 0x76.
pub const BMP280_ADDRESS: u8 = 0x77;

const CHIP_ID: u8 = 0x58;
const REG_CALIB: u8 = 0x88;
const REG_ID: u8 = 0xD0;
const REG_STATUS: u8 = 0xF3;
const REG_CTRL_MEAS: u8 = 0xF4;
const REG_CONFIG: u8 = 0xF5;
const REG_PRESS: u8 = 0xF7;
const REG_TEMP: u8 = 0xFA;
const REG_RESET: u8 = 0xE0;

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

/// Raw ADC values the datasheet compensates to 25.08 °C and 100653.25 Pa.
const ADC_T: u32 = 519888;
const ADC_P: u32 = 415148;

pub struct Bmp280 {
    regs: [u8; 256],
    ptr: u8,
    first: bool,
    /// Raw 20-bit samples the next measurement publishes at 0xF7/0xFA.
    raw_t: u32,
    raw_p: u32,
}

impl Default for Bmp280 { fn default() -> Self { Self::new() } }

impl Bmp280 {
    pub fn new() -> Self {
        let mut d = Bmp280 {
            regs: [0u8; 256], ptr: 0, first: true,
            raw_t: ADC_T, raw_p: ADC_P,
        };
        d.reset();
        d
    }

    /// Power-on / soft-reset state (datasheet 4.3.2): calibration and chip ID restored, control,
    /// status and config cleared, and both data channels reading 0x80 0x00 0x00 — the reset value
    /// of Table 18, and the value a channel with oversampling 000 reports (Tables 21, 22).
    fn reset(&mut self) {
        self.regs = [0u8; 256];
        for (i, &w) in CALIBRATION.iter().enumerate() {
            let r = REG_CALIB as usize + 2 * i;
            self.regs[r] = w as u8;
            self.regs[r + 1] = (w >> 8) as u8;
        }
        self.regs[REG_ID as usize] = CHIP_ID;
        // im_update (bit 0) and measuring (bit 3) only ever read clear here: a measurement
        // completes inside the write that starts it (3.6.2), so no conversion is ever pending.
        self.regs[REG_STATUS as usize] = 0;
        self.regs[REG_PRESS as usize] = 0x80;
        self.regs[REG_TEMP as usize] = 0x80;
    }

    /// Stage the samples the next measurement reports. In sleep and forced mode nothing reaches
    /// the data registers until a measurement runs, so a guest that has not configured
    /// `ctrl_meas` still reads 0x80000. Normal mode (mode bits 0b11) measures again every
    /// `t_standby` by itself (3.6.3), so there the staged sample becomes visible at once.
    pub fn set_raw(&mut self, adc_t: u32, adc_p: u32) {
        self.raw_t = adc_t & 0x000F_FFFF;
        self.raw_p = adc_p & 0x000F_FFFF;
        if self.regs[REG_CTRL_MEAS as usize] & 0x03 == 0x03 { self.publish(); }
    }

    /// Write the sample a measurement produced into the data registers, MSB first, low nibble of
    /// xlsb clear. A channel whose oversampling is 000 was skipped by that measurement and reports
    /// 0x80000 as silicon does (Tables 21, 22); `ctrl_meas` still holds the rates just written.
    fn publish(&mut self) {
        let ctrl = self.regs[REG_CTRL_MEAS as usize];
        let p = if ctrl & 0x1C != 0 { self.raw_p } else { 0x8_0000 };
        let t = if ctrl & 0xE0 != 0 { self.raw_t } else { 0x8_0000 };
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

    /// Temperature in 0.01 °C and pressure in Q24.8 (Pa × 256), by the datasheet's integer path
    /// (3.11.3): the same values `bmp280_compensate_T_int32` and `bmp280_compensate_P_int64`
    /// return. Keeping the integers means the worked example can be asserted exactly.
    pub fn compensate(&self) -> (i32, u32) {
        let adc_t = self.sample(REG_TEMP);
        let adc_p = self.sample(REG_PRESS);

        let dig_t1 = self.word(0x88) as i32;
        let dig_t2 = self.sword(0x8A);
        let dig_t3 = self.sword(0x8C);
        let var1 = (((adc_t >> 3) - (dig_t1 << 1)) * dig_t2) >> 11;
        let var2 = (((((adc_t >> 4) - dig_t1) * ((adc_t >> 4) - dig_t1)) >> 12) * dig_t3) >> 14;
        let t_fine = var1 + var2;
        let t = (t_fine * 5 + 128) >> 8;

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
        if var1 == 0 { return (t, 0); }
        let mut p = 1048576 - adc_p as i64;
        p = (((p << 31) - var2) * 3125) / var1;
        let var1 = (dig_p9 * (p >> 13) * (p >> 13)) >> 25;
        let var2 = (dig_p8 * p) >> 19;
        p = ((p + var1 + var2) >> 8) + (dig_p7 << 4);
        (t, p as u32)
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
        match r {
            REG_CTRL_MEAS => {
                self.regs[r as usize] = b;
                let mode = b & 0x03;
                if mode != 0 {
                    // A device model completes a command when the guest issues it: publish
                    // the sample on the write, not on a later status read (decisions.md).
                    self.publish();
                }
                if mode == 0b01 || mode == 0b10 {
                    // Forced mode returns to sleep when conversion finishes (datasheet 3.6.2).
                    // esp-idf-lib's is_measuring() polls (ctrl & 3) == FORCED, so leaving the
                    // mode bits set would hang it forever. status stays clear throughout.
                    self.regs[r as usize] = b & !0x03;
                }
                // mode 0b11 (normal, i.e. 0x03) keeps its mode bits and measures again every
                // t_standby, which is why set_raw publishes at once in that mode. mode 0b00 does
                // nothing above.
            }
            REG_CONFIG => self.regs[r as usize] = b,
            // Datasheet 4.3.2: 0xB6 triggers a full power-on reset; other values ignored.
            REG_RESET if b == 0xB6 => self.reset(),
            REG_RESET => {}
            // Table 18: calibration, chip ID, status and data registers are read-only.
            // A writable calibration lets a guest make the integer compensation overflow.
            _ => {}
        }

        // Multiple-byte writes on this part are address/data *pairs* (datasheet 5.2.1),
        // not a start address followed by a run of data. After each data byte the bus
        // expects an address again; ptr stays put so a following read starts at this register.
        self.ptr = r;
        self.first = true;
        true
    }

    fn read(&mut self) -> u8 {
        let r = self.ptr;
        let v = self.regs[r as usize];
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
    fn default_samples_compensate_to_the_datasheet_example_exactly() {
        let mut d = Bmp280::new();
        // The page-23 worked example is the raw pair in ADC_T/ADC_P with both channels
        // oversampled; force one measurement so it reaches the data registers.
        d.start(false); d.write(REG_CTRL_MEAS); d.write(0x6D);
        // bmp280_compensate_T_int32 -> 2508 (25.08 °C) and bmp280_compensate_P_int64 ->
        // 25767233 (Q24.8, 100653.25 Pa). The printed table says 25767236; 3.11.3 allows the
        // integer result to deviate. Asserting the exact integers catches an off-by-a-shift.
        assert_eq!(d.compensate(), (2508, 25_767_233));
    }

    /// Table 18 gives 0x80 0x00 0x00 for both channels until a measurement has run, and
    /// Tables 21 and 22 give 0x80000 for a channel whose oversampling is 000.
    #[test]
    fn data_registers_hold_the_reset_value_until_a_measurement_covers_a_channel() {
        let mut d = Bmp280::new();
        assert_eq!(read_at(&mut d, REG_PRESS, 3), [0x80, 0x00, 0x00]);
        assert_eq!(read_at(&mut d, REG_TEMP, 3), [0x80, 0x00, 0x00]);
        // osrs_t = x4, osrs_p = skipped, mode 01: only temperature is measured.
        d.start(false); d.write(REG_CTRL_MEAS); d.write(0x61);
        assert_eq!(read_at(&mut d, REG_PRESS, 3), [0x80, 0x00, 0x00]);
        assert_eq!(read_at(&mut d, REG_TEMP, 3), [0x7E, 0xED, 0x00]);
        d.start(false); d.write(REG_RESET); d.write(0xB6);
        assert_eq!(read_at(&mut d, REG_TEMP, 3), [0x80, 0x00, 0x00]);
    }

    #[test]
    fn forced_measurement_completes_on_write_and_data_is_contiguous() {
        let mut d = Bmp280::new();
        // esp-idf-lib writes 0x6C at init and 0x6D to force: osrs_t = 011 (x4), osrs_p = 011
        // (x4), mode 00 at init and 01 when forcing.
        d.start(false); d.write(REG_CTRL_MEAS); d.write(0x6D);
        // The measurement is finished by the write: mode bits return to 00, status stays clear.
        assert_eq!(read_at(&mut d, REG_CTRL_MEAS, 1)[0], 0x6C);
        assert_eq!(read_at(&mut d, REG_STATUS, 1)[0] & 0x08, 0);
        // The driver reads six bytes from 0xF7 and splits them pressure 0..3 / temperature 3..6.
        let six = read_at(&mut d, REG_PRESS, 6);
        let p = ((six[0] as i32) << 12) | ((six[1] as i32) << 4) | ((six[2] as i32) >> 4);
        let t = ((six[3] as i32) << 12) | ((six[4] as i32) << 4) | ((six[5] as i32) >> 4);
        assert_eq!((t, p), (ADC_T as i32, ADC_P as i32));
    }

    /// esp-idf-lib's `bmp280_is_measuring` sends [0xF3, 0xF4] before a two-byte read.
    /// With pair-wise writes that is a write to read-only status, then a read at 0xF3,
    /// yielding status and ctrl — never the old "status overwritten with 0xF4" bug.
    #[test]
    fn esp_idf_lib_is_measuring_never_reports_busy() {
        let mut d = Bmp280::new();
        d.start(false);
        d.write(REG_RESET);   d.write(0xB6);   // init soft reset
        d.write(REG_CTRL_MEAS); d.write(0x6C); // init normal config, mode 00
        d.write(REG_CTRL_MEAS); d.write(0x6D); // forced
        d.write(REG_STATUS);  d.write(REG_CTRL_MEAS); // the [0xF3, 0xF4] pair
        d.start(true);
        let status = d.read();
        let ctrl = d.read();
        assert_eq!(status & 0x08, 0, "status must not report busy");
        assert_eq!(ctrl & 0x03, 0, "forced mode must have returned to sleep");
    }

    #[test]
    fn read_only_registers_ignore_writes_and_soft_reset_restores() {
        let mut d = Bmp280::new();
        d.start(false); d.write(REG_ID); d.write(0x00);
        assert_eq!(read_at(&mut d, REG_ID, 1)[0], CHIP_ID);

        d.start(false); d.write(REG_CTRL_MEAS); d.write(0x6D);
        assert_eq!(read_at(&mut d, REG_CTRL_MEAS, 1)[0], 0x6C);

        d.start(false); d.write(REG_RESET); d.write(0xB6);
        assert_eq!(read_at(&mut d, REG_ID, 1)[0], CHIP_ID);
        assert_eq!(read_at(&mut d, REG_CTRL_MEAS, 1)[0], 0x00);
    }

    #[test]
    fn interleaved_config_and_ctrl_writes_land_in_their_own_registers() {
        let mut d = Bmp280::new();
        d.start(false);
        d.write(REG_CTRL_MEAS); d.write(0x6D);
        d.write(REG_CONFIG); d.write(0xA0);
        assert_eq!(read_at(&mut d, REG_CONFIG, 1)[0], 0xA0);
        assert_eq!(read_at(&mut d, REG_CTRL_MEAS, 1)[0] & 0x03, 0);
    }

    #[test]
    fn a_sample_staged_in_normal_mode_is_visible_without_another_write() {
        let mut d = Bmp280::new();
        // osrs_t = x4, osrs_p = x4, mode = normal (0b11).
        d.start(false); d.write(REG_CTRL_MEAS); d.write(0x6F);
        assert_eq!(read_at(&mut d, REG_CTRL_MEAS, 1)[0], 0x6F, "normal mode keeps its bits");
        // Normal mode measures again every t_standby (3.6.3), so the staged sample shows up
        // without a second ctrl_meas write. Adafruit's setSampling() defaults to MODE_NORMAL
        // and Linux's bmp280-core switches to it for buffered capture; both just read the data.
        d.set_raw(ADC_T - 100_000, ADC_P);
        let six = read_at(&mut d, REG_PRESS, 6);
        let p = ((six[0] as i32) << 12) | ((six[1] as i32) << 4) | ((six[2] as i32) >> 4);
        let t = ((six[3] as i32) << 12) | ((six[4] as i32) << 4) | ((six[5] as i32) >> 4);
        assert_eq!((t, p), ((ADC_T - 100_000) as i32, ADC_P as i32));
    }

    #[test]
    fn a_non_magic_reset_write_and_read_only_writes_change_nothing() {
        let mut d = Bmp280::new();
        d.start(false); d.write(REG_CTRL_MEAS); d.write(0x6F);
        // Only 0xB6 resets (4.3.2): any other value at 0xE0 is an ordinary ignored write.
        d.start(false); d.write(REG_RESET); d.write(0x00);
        assert_eq!(read_at(&mut d, REG_CTRL_MEAS, 1)[0], 0x6F);

        // Calibration word T1 at 0x88 stays 27504 (0x6B70) whatever a guest writes.
        d.start(false); d.write(REG_CALIB); d.write(0x00);
        d.start(false); d.write(REG_CALIB + 1); d.write(0x00);
        assert_eq!(read_at(&mut d, REG_CALIB, 2), [0x70, 0x6B]);

        // The data registers keep what the last measurement published (adc_P = 415148).
        let before = read_at(&mut d, REG_PRESS, 3);
        assert_eq!(before, [0x65, 0x5A, 0xC0]);
        d.start(false); d.write(REG_PRESS); d.write(0x00);
        d.start(false); d.write(REG_PRESS + 1); d.write(0x00);
        assert_eq!(read_at(&mut d, REG_PRESS, 3), before);
    }

    #[test]
    fn a_different_sample_changes_the_reported_value() {
        let mut d = Bmp280::new();
        d.start(false); d.write(REG_CTRL_MEAS); d.write(0x6D);
        let (t0, _) = d.compensate();
        d.set_raw(ADC_T - 200_000, ADC_P);
        d.start(false); d.write(REG_CTRL_MEAS); d.write(0x6D);
        let (t1, _) = d.compensate();
        assert!(t1 < t0, "cooler raw sample should read cooler: {t0} -> {t1}");
    }
}
