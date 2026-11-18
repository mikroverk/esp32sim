//! An ST LSM6DS3 six-axis IMU on I2C (0x6A with SA0 low; 0x6B with SA0 high), as ST's driver and
//! esp-idf-lib's `lsm6ds3` component use it: WHO_AM_I at 0x0F reads 0x69, CTRL3_C at 0x12 takes a
//! software reset (0x01) or BDU|IF_INC (0x44), CTRL1_XL/CTRL2_G at 0x10/0x11 enable the
//! accelerometer and gyroscope, STATUS_REG at 0x1E reports both data-ready flags, and the six out
//! registers from 0x22 (gyro) and 0x28 (accel) hold little-endian 16-bit samples. The die
//! temperature at 0x20 is 25 °C plus 1/16 °C per count, as the datasheet defines it.
use esp_periph::i2c::I2cDevice;

/// SA0 strapped low, the common breakout default. SA0 high is 0x6B.
pub const LSM6DS3_ADDRESS: u8 = 0x6A;
pub const LSM6DS3_WHO_AM_I_VALUE: u8 = 0x69;

const REG_OUT_TEMP_L: u8 = 0x20;
const REG_WHO_AM_I: u8 = 0x0F;
const REG_CTRL3_C: u8 = 0x12;
const REG_STATUS: u8 = 0x1E;
const REG_OUTX_L_G: u8 = 0x22;
const REG_OUTX_L_XL: u8 = 0x28;

const CTRL3_SW_RESET: u8 = 0x01;

/// Both data-ready flags (XLDA | GDA) set while the outputs hold a fresh sample.
const STATUS_READY: u8 = 0x03;

/// 16 384 LSB per g at the default ±2 g full scale; the gyro sits at rest.
const ONE_G: i16 = 16_384;

pub struct Lsm6ds3 {
    regs: [u8; 128],
    ptr: u8,
    first: bool,
    /// Accelerometer and gyro counts published at 0x28 and 0x22, LSB at ±2 g / ±250 dps.
    accel: [i16; 3],
    gyro: [i16; 3],
    pub reads: u64,
}

impl Default for Lsm6ds3 {
    fn default() -> Self { Self::new() }
}

impl Lsm6ds3 {
    /// At rest, face up: gravity on +Z, the gyro still.
    pub fn new() -> Self {
        let mut d = Self {
            regs: [0; 128],
            ptr: 0,
            first: false,
            accel: [0, 0, ONE_G],
            gyro: [0, 0, 0],
            reads: 0,
        };
        d.regs[REG_WHO_AM_I as usize] = LSM6DS3_WHO_AM_I_VALUE;
        d.publish();
        d
    }

    /// A board can hand the model another posture or motion; counts stay LSB at ±2 g / ±250 dps.
    pub fn set_samples(&mut self, accel: [i16; 3], gyro: [i16; 3]) {
        self.accel = accel;
        self.gyro = gyro;
        self.publish();
    }

    /// The accelerometer and gyro samples a driver would decode.
    pub fn samples(&self) -> ([i16; 3], [i16; 3]) { (self.accel, self.gyro) }

    fn publish(&mut self) {
        for (i, v) in self.gyro.iter().enumerate() {
            let [lo, hi] = v.to_le_bytes();
            self.regs[REG_OUTX_L_G as usize + i * 2] = lo;
            self.regs[REG_OUTX_L_G as usize + i * 2 + 1] = hi;
        }
        for (i, v) in self.accel.iter().enumerate() {
            let [lo, hi] = v.to_le_bytes();
            self.regs[REG_OUTX_L_XL as usize + i * 2] = lo;
            self.regs[REG_OUTX_L_XL as usize + i * 2 + 1] = hi;
        }
        // 25 °C at reset, in the datasheet's 1/16 °C counts, little-endian.
        let temp = (25.0 * 16.0) as i16;
        let [lo, hi] = temp.to_le_bytes();
        self.regs[REG_OUT_TEMP_L as usize] = lo;
        self.regs[REG_OUT_TEMP_L as usize + 1] = hi;
        self.regs[REG_STATUS as usize] = STATUS_READY;
    }
}

impl I2cDevice for Lsm6ds3 {
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
        if r == REG_CTRL3_C && b & CTRL3_SW_RESET != 0 {
            // Software reset: control registers return to power-on, the sample registers stay live.
            self.regs = [0; 128];
            self.regs[REG_WHO_AM_I as usize] = LSM6DS3_WHO_AM_I_VALUE;
            self.publish();
        } else {
            self.regs[r as usize & 0x7f] = b;
        }
        self.ptr = r.wrapping_add(1);
        true
    }

    fn read(&mut self) -> u8 {
        let r = self.ptr;
        if (REG_OUTX_L_G..REG_OUTX_L_G + 6).contains(&r) || (REG_OUTX_L_XL..REG_OUTX_L_XL + 6).contains(&r) {
            self.reads += 1;
        }
        let v = self.regs[r as usize & 0x7f];
        self.ptr = r.wrapping_add(1);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_at(d: &mut Lsm6ds3, reg: u8, n: usize) -> Vec<u8> {
        d.start(false); d.write(reg); d.start(true);
        (0..n).map(|_| d.read()).collect()
    }

    #[test]
    fn who_am_i_reads_the_lsm6ds3_id() {
        let mut d = Lsm6ds3::new();
        assert_eq!(read_at(&mut d, REG_WHO_AM_I, 1), [LSM6DS3_WHO_AM_I_VALUE]);
    }

    #[test]
    fn status_reports_both_data_ready_flags() {
        let mut d = Lsm6ds3::new();
        let s = read_at(&mut d, REG_STATUS, 1)[0];
        assert_eq!(s & 0x01, 1, "XLDA: accelerometer data ready");
        assert_eq!(s & 0x02, 2, "GDA: gyroscope data ready");
    }

    #[test]
    fn accelerometer_reports_one_g_on_z() {
        let mut d = Lsm6ds3::new();
        let b = read_at(&mut d, REG_OUTX_L_XL, 6);
        let axis = |i: usize| i16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
        assert_eq!((axis(0), axis(1), axis(2)), (0, 0, ONE_G));
    }

    #[test]
    fn a_configured_sample_replaces_the_default_posture() {
        let mut d = Lsm6ds3::new();
        d.set_samples([1000, -2000, 3000], [10, 20, 30]);
        let b = read_at(&mut d, REG_OUTX_L_G, 6);
        assert_eq!(i16::from_le_bytes([b[0], b[1]]), 10);
        assert_eq!(i16::from_le_bytes([b[4], b[5]]), 30);
        let a = read_at(&mut d, REG_OUTX_L_XL, 6);
        assert_eq!(i16::from_le_bytes([a[2], a[3]]), -2000);
    }

    #[test]
    fn soft_reset_keeps_who_am_i_and_the_samples() {
        let mut d = Lsm6ds3::new();
        d.start(false); d.write(REG_CTRL3_C); d.write(CTRL3_SW_RESET);
        assert_eq!(read_at(&mut d, REG_WHO_AM_I, 1), [LSM6DS3_WHO_AM_I_VALUE]);
        let b = read_at(&mut d, REG_OUTX_L_XL, 6);
        assert_eq!(i16::from_le_bytes([b[4], b[5]]), ONE_G);
    }
}
