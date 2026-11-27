//! A TDK InvenSense ICM-20948 9-axis IMU on I2C (0x68 with AD0 low, 0x69 high) with its AK09916
//! magnetometer behind the auxiliary I2C master, as SparkFun's ICM-20948 Arduino library v1.3.2
//! (ICM_20948.cpp / util/ICM_20948_C.c, built with ICM_20948_USE_DMP) drives it in DMP mode.
//! Register numbers from the library's util/ICM_20948_REGISTERS.h, reset values from the
//! datasheet (DS-000189 rev 1.3, section 7 "Register map"):
//!
//! - Four user banks of 128 registers, REG_BANK_SEL 0x7F bits 5:4 (visible in every bank).
//!   Bank 0: WHO_AM_I 0x00 = 0xEA, USER_CTRL 0x03 (DMP_EN 7, FIFO_EN 6, I2C_MST_EN 5, DMP_RST 3,
//!   SRAM_RST 2, I2C_MST_RST 1: the resets self-clear), LP_CONFIG 0x05 = 0x40, PWR_MGMT_1 0x06 =
//!   0x41 (DEVICE_RESET 7 restores every bank, SLEEP 6), PWR_MGMT_2 0x07 (accel disable 5:3, gyro
//!   disable 2:0). The library's `set_sample_mode` / `set_full_scale` / `enable_dlpf` read their
//!   writes back, which plain storage satisfies.
//! - ACCEL_XOUT_H 0x2D..GYRO_ZOUT_L 0x38, TEMP_OUT 0x39/0x3A are big-endian and sampled from the
//!   pose input at the start of each read: accel 16384 >> ACCEL_FS_SEL LSB/g (bank 2 ACCEL_CONFIG
//!   0x14 bits 2:1), gyro 131 >> GYRO_FS_SEL LSB/dps (bank 2 GYRO_CONFIG_1 0x01 bits 2:1), temp
//!   333.87 LSB/C around 21 C (we report 25 C). INT_STATUS_1 0x1A bit 0 RAW_DATA_0_RDY is set.
//! - DMP memory (undocumented; the library's comments on AGB0_REG_MEM_*): MEM_BANK_SEL 0x7E picks
//!   a 256-byte page, MEM_START_ADDR 0x7C the offset in it, and MEM_R_W 0x7D bursts read or write
//!   from there, the offset advancing per byte within the page while the register pointer stays
//!   on MEM_R_W. `inv_icm20948_firmware_load` uploads the 14301-byte DMP image at 0x90 in 16-byte
//!   chunks and reads it back to compare, so memory that keeps what was written is all it needs.
//! - Auxiliary I2C master (bank 3): I2C_SLV4_ADDR 0x13 (bit 7 read), I2C_SLV4_REG 0x14,
//!   I2C_SLV4_CTRL 0x15 (EN bit 7, REG_DIS bit 5), I2C_SLV4_DO 0x16, I2C_SLV4_DI 0x17. Setting
//!   EN with USER_CTRL.I2C_MST_EN on runs one byte transfer to the aux bus at once, clears EN and
//!   sets I2C_MST_STATUS 0x17 (bank 0) I2C_SLV4_DONE (bit 6), plus I2C_SLV4_NACK (bit 4) when no
//!   device answers; the status clears on read. `ICM_20948_i2c_controller_periph4_txn` polls
//!   exactly that bit (up to 1000 times) and then reads DI. SLV0-3 (ADDR/REG/CTRL/DO at
//!   0x03 + 4n) run when the host reads EXT_SLV_SENS_DATA_00..23 (0x3B-0x52): enabled reads are
//!   packed there in slot order (CTRL LENG 3:0; BYTE_SW bit 6 swaps byte pairs, which GRP bit 4
//!   starts at the second byte, as the library's `initializeDMP` comment on its SLV0 setup
//!   reads it), enabled writes send DO.
//! - FIFO: FIFO_COUNTH/L 0x70/0x71 (reading COUNTH latches the low byte, as the datasheet
//!   describes), FIFO_R_W 0x72 (bursts stay on it), FIFO_RST 0x68 (any of bits 4:0 empties it).
//!   With DMP_EN and FIFO_EN set and the chip awake, the "DMP" appends one packet per sample at
//!   1.1 kHz / (1 + GYRO_SMPLRT_DIV) (bank 2 0x00; `initializeDMP` sets 19, 55 Hz) of guest time,
//!   in the layout `inv_icm20948_read_dmp_data` parses: a big-endian header, then raw accel (3 x
//!   i16), raw gyro (3 x i16 + 3 x i16 bias, 0), compass (3 x i16), Quat6 (3 x i32), Quat9 (3 x i32,
//!   i16 heading accuracy), all big endian, and a 2-byte footer. Which blocks are produced
//!   follows DATA_OUT_CTL1 (DMP address 0x40) as `inv_icm20948_enable_dmp_sensor` wrote it, each
//!   on its ODR register (ODR_ACCEL 0xBE, ODR_GYRO 0xBA, ODR_CPASS 0xB6, ODR_QUAT6 0xAC,
//!   ODR_QUAT9 0xA8: emitted every interval + 1 samples). Header2, the accuracy words, the
//!   calibrated outputs and the remaining blocks are never emitted, so the parser stays aligned.
//!   A packet that would not fit the 512-byte FIFO is dropped whole instead of overwriting the
//!   oldest bytes as silicon does (which would only desynchronise the parser).
//!
//! Outputs in the FIFO: raw accel and gyro in the configured full scales (`initializeDMP`: +-4 g,
//! 2000 dps), compass in the chip's axes at the AK09916's 0.15 uT/LSB, quaternions scaled by 2^30
//! with Q0 >= 0 (the library rebuilds Q0 as sqrt(1 - Q1^2 - Q2^2 - Q3^2)). The DMP's own sensor
//! fusion and calibration are not modelled: the quaternion is the pose input.
//!
//! AK09916 (datasheet AKM 016015392-E-02, section 9): WIA1 0x00 = 0x48, WIA2 0x01 = 0x09
//! (`MAG_AK09916_WHO_AM_I` 0x4809), ST1 0x10 (DRDY bit 0), HXL..HZH 0x11-0x16 little endian at
//! 0.15 uT/LSB, ST2 0x18 (reading it ends the measurement: DRDY clears), CNTL2 0x31 (MODE 4:0:
//! single measurement 0x01 returns to power-down after one conversion, continuous modes 0x02-0x08
//! convert on every read), CNTL3 0x32 (SRST bit 0, self-clearing). In DMP mode the library has
//! SLV0 read 10 bytes from the reserved RSV2 0x03 and says the data there is big endian from
//! 0x04; we serve ST1 at 0x03, HX/HY/HZ big endian at 0x04-0x09 and ST2 at 0x0B. The AK09916's
//! Y and Z axes point opposite the ICM's (the library's compass mount matrix, from InvenSense's
//! Nucleo example, negates them), so it measures (x, -y, -z) of the field in the chip's axes.
//!
//! Pose input (`Icm20948Input`): orientation as a unit quaternion (w, x, y, z) rotating chip axes
//! into a z-up world frame, angular rate in chip axes (rad/s), linear acceleration in the world
//! frame without gravity (m/s^2), and the geomagnetic field in the world frame (uT). The default
//! is level and at rest: the accelerometer reads +1 g on z.
use super::imu_motion::ImuMotion;
use esp_periph::i2c::I2cDevice;
use std::collections::VecDeque;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

/// AD0 low, as on the Waveshare General Driver for Robots (`ROS_Driver.ino` AD0_VAL 0).
pub const ICM20948_ADDRESS: u8 = 0x68;
pub const ICM20948_WHO_AM_I: u8 = 0xea;
pub const AK09916_ADDRESS: u8 = 0x0c;
const FIFO_SIZE: usize = 512;
const GRAVITY: f64 = 9.80665;
/// AK09916 sensitivity, datasheet section 8.2: 0.15 uT/LSB, +-32752 LSB.
const AK_UT_PER_LSB: f64 = 0.15;

// Bank 0.
const USER_CTRL: usize = 0x03;
const LP_CONFIG: usize = 0x05;
const PWR_MGMT_1: usize = 0x06;
const PWR_MGMT_2: usize = 0x07;
const I2C_MST_STATUS: usize = 0x17;
const INT_STATUS_1: usize = 0x1a;
const ACCEL_XOUT_H: usize = 0x2d;
const TEMP_OUT_H: usize = 0x39;
const EXT_SLV_SENS_DATA_00: usize = 0x3b;
const FIFO_RST: usize = 0x68;
const FIFO_COUNTH: usize = 0x70;
const FIFO_COUNTL: usize = 0x71;
const FIFO_R_W: usize = 0x72;
const MEM_START_ADDR: usize = 0x7c;
const MEM_R_W: usize = 0x7d;
const MEM_BANK_SEL: usize = 0x7e;
const REG_BANK_SEL: usize = 0x7f;
// Bank 1, 2, 3.
const TIMEBASE_CORRECTION_PLL: usize = 0x28;
const GYRO_SMPLRT_DIV: usize = 0x00;
const GYRO_CONFIG_1: usize = 0x01;
const ACCEL_CONFIG: usize = 0x14;
const I2C_SLV0_ADDR: usize = 0x03;
const I2C_SLV4_ADDR: usize = 0x13;
const I2C_SLV4_REG: usize = 0x14;
const I2C_SLV4_CTRL: usize = 0x15;
const I2C_SLV4_DO: usize = 0x16;
const I2C_SLV4_DI: usize = 0x17;
// DMP memory keys, util/ICM_20948_DMP.h.
const DATA_OUT_CTL1: usize = 4 * 16;
const ODR_QUAT9: usize = 10 * 16 + 8;
const ODR_QUAT6: usize = 10 * 16 + 12;
const ODR_CPASS: usize = 11 * 16 + 6;
const ODR_GYRO: usize = 11 * 16 + 10;
const ODR_ACCEL: usize = 11 * 16 + 14;
// DMP_header_bitmap / DMP_Data_Output_Control_1 bits.
const OUT_ACCEL: u16 = 0x8000;
const OUT_GYRO: u16 = 0x4000;
const OUT_COMPASS: u16 = 0x2000;
const OUT_QUAT6: u16 = 0x0800;
const OUT_QUAT9: u16 = 0x0400;

/// What the chip experiences; see the module comment for frames and units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Icm20948Pose { pub orientation: [f64; 4], pub angular_rate: [f64; 3], pub linear_accel: [f64; 3], pub mag_field: [f64; 3] }
impl Default for Icm20948Pose {
    /// Level, at rest, a northern-hemisphere field (north along +x, dipping down).
    fn default() -> Self { Icm20948Pose { orientation: [1.0, 0.0, 0.0, 0.0], angular_rate: [0.0; 3], linear_accel: [0.0; 3], mag_field: [20.0, 0.0, -40.0] } }
}

/// Shared handle on the pose, kept by whoever drives the motion (a physics model, a board page)
/// after the device itself has gone onto the I2C bus.
#[derive(Clone, Default, Debug)]
pub struct Icm20948Input(Arc<Mutex<Icm20948Pose>>);
impl Icm20948Input {
    pub fn get(&self) -> Icm20948Pose { *self.0.lock().unwrap_or_else(|e| e.into_inner()) }
    fn update(&self, f: impl FnOnce(&mut Icm20948Pose)) { f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner())) }
    pub fn set(&self, pose: Icm20948Pose) { self.update(|p| *p = pose) }
    /// Unit quaternion (w, x, y, z) taking chip axes to the world frame; normalised on use.
    pub fn set_orientation(&self, q: [f64; 4]) { self.update(|p| p.orientation = q) }
    /// Body rates about the chip's x, y, z axes in rad/s.
    pub fn set_angular_rate(&self, w: [f64; 3]) { self.update(|p| p.angular_rate = w) }
    /// World-frame acceleration of the chip in m/s^2, gravity excluded.
    pub fn set_linear_accel(&self, a: [f64; 3]) { self.update(|p| p.linear_accel = a) }
    /// World-frame geomagnetic field in uT.
    pub fn set_mag_field(&self, b: [f64; 3]) { self.update(|p| p.mag_field = b) }
}

/// Rotate `v` by the unit quaternion `q` (w, x, y, z).
fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let u = [q[1], q[2], q[3]];
    let t = cross(u, v).map(|c| 2.0 * c);
    let c = cross(u, t);
    [v[0] + q[0] * t[0] + c[0], v[1] + q[0] * t[1] + c[1], v[2] + q[0] * t[2] + c[2]]
}
fn normalized(q: [f64; 4]) -> [f64; 4] {
    let n = q.iter().map(|c| c * c).sum::<f64>().sqrt();
    if n.is_finite() && n > 1e-9 { q.map(|c| c / n) } else { [1.0, 0.0, 0.0, 0.0] }
}
/// Round and saturate to the i16 a converter would report.
fn sat16(v: f64) -> i16 { v.round().clamp(-32768.0, 32767.0) as i16 }

/// The pose seen from the chip: accelerometer in g, gyro in dps, field in uT, all chip axes.
struct Sample { accel: [f64; 3], gyro: [f64; 3], field: [f64; 3], q: [f64; 4] }
fn sample(p: &Icm20948Pose) -> Sample {
    let q = normalized(p.orientation);
    let inv = [q[0], -q[1], -q[2], -q[3]];
    // The accelerometer reports the reaction to gravity plus the chip's own acceleration.
    let f = [p.linear_accel[0], p.linear_accel[1], p.linear_accel[2] + GRAVITY];
    Sample { accel: rotate(inv, f).map(|a| a / GRAVITY), gyro: p.angular_rate.map(f64::to_degrees), field: rotate(inv, p.mag_field), q }
}

/// The AK09916 on the auxiliary bus.
struct Ak09916 { cntl2: u8, drdy: bool, data: [i16; 3], ptr: u8 }
impl Ak09916 {
    fn new() -> Self { Ak09916 { cntl2: 0, drdy: false, data: [0; 3], ptr: 0 } }
    fn measure(&mut self, field: [f64; 3]) {
        // AK09916 Y and Z are opposite the ICM's.
        let raw = [field[0], -field[1], -field[2]].map(|b| (b / AK_UT_PER_LSB).round().clamp(-32752.0, 32752.0) as i16);
        self.data = raw;
        self.drdy = true;
    }
    /// Continuous modes convert on every access; single mode converted when it was written.
    fn refresh(&mut self, field: [f64; 3]) { if matches!(self.cntl2 & 0x1f, 0x02 | 0x04 | 0x06 | 0x08) { self.measure(field); } }
    fn read(&mut self, field: [f64; 3]) -> u8 {
        let r = self.ptr;
        self.ptr = self.ptr.wrapping_add(1);
        if r == 0x03 || r == 0x10 { self.refresh(field); }
        let [x, y, z] = self.data.map(i16::to_le_bytes);
        match r {
            0x00 => 0x48,
            0x01 => 0x09,
            0x03 | 0x10 => u8::from(self.drdy),
            0x04 => x[1], 0x05 => x[0], 0x06 => y[1], 0x07 => y[0], 0x08 => z[1], 0x09 => z[0],
            0x11 => x[0], 0x12 => x[1], 0x13 => y[0], 0x14 => y[1], 0x15 => z[0], 0x16 => z[1],
            0x0b | 0x18 => { self.drdy = false; 0 }
            0x31 => self.cntl2,
            _ => 0,
        }
    }
    fn write(&mut self, reg: u8, b: u8, field: [f64; 3]) {
        self.ptr = reg.wrapping_add(1);
        match reg {
            0x31 => { self.cntl2 = b & 0x1f; if self.cntl2 == 0x01 { self.measure(field); self.cntl2 = 0; } }
            0x32 if b & 1 != 0 => *self = Ak09916::new(),
            _ => {}
        }
    }
}

pub struct Icm20948 {
    regs: [[u8; 128]; 4],
    /// DMP memory: 256 pages of 256 bytes (MEM_BANK_SEL, MEM_START_ADDR).
    mem: Vec<u8>,
    fifo: VecDeque<u8>,
    count_latch: Option<u16>,
    ak: Ak09916,
    ptr: u8,
    first: bool,
    input: Icm20948Input,
    motion: Arc<ImuMotion>,
    /// Rate of `motion.cycle` (ticks per second of guest time).
    clock_hz: u64,
    /// Guest tick of the next DMP sample while the DMP runs, and the samples since it started.
    next_sample: Option<u64>,
    samples: u64,
    /// Packets dropped on a full FIFO.
    pub dropped: u64,
}

impl Icm20948 {
    pub fn new(motion: Arc<ImuMotion>, clock_hz: u64) -> Self { Self::with_input(motion, clock_hz, Icm20948Input::default()) }
    /// A chip on an existing pose handle: a board keeps one across guest resets.
    pub fn with_input(motion: Arc<ImuMotion>, clock_hz: u64, input: Icm20948Input) -> Self {
        let mut d = Icm20948 { regs: [[0; 128]; 4], mem: vec![0; 0x10000], fifo: VecDeque::new(), count_latch: None, ak: Ak09916::new(), ptr: 0, first: true,
                               input, motion, clock_hz, next_sample: None, samples: 0, dropped: 0 };
        d.reset();
        d
    }
    /// The shared pose handle; clone it before attaching the device to the bus.
    pub fn input(&self) -> Icm20948Input { self.input.clone() }
    pub fn set_orientation(&self, q: [f64; 4]) { self.input.set_orientation(q) }
    pub fn set_angular_rate(&self, w: [f64; 3]) { self.input.set_angular_rate(w) }
    pub fn set_linear_accel(&self, a: [f64; 3]) { self.input.set_linear_accel(a) }
    pub fn set_mag_field(&self, b: [f64; 3]) { self.input.set_mag_field(b) }
    /// Reset values, datasheet section 7. DMP memory is left alone.
    fn reset(&mut self) {
        self.regs = [[0; 128]; 4];
        self.regs[0][0] = ICM20948_WHO_AM_I;
        self.regs[0][LP_CONFIG] = 0x40;
        self.regs[0][PWR_MGMT_1] = 0x41;     // sleep, auto clock
        self.regs[1][TIMEBASE_CORRECTION_PLL] = 0x18;   // per part; the library's "typical value"
        self.regs[2][GYRO_CONFIG_1] = 0x01;
        self.regs[2][ACCEL_CONFIG] = 0x01;
        self.fifo.clear();
        self.count_latch = None;
        self.next_sample = None;
        self.ak = Ak09916::new();
    }
    fn bank(&self) -> usize { usize::from(self.regs[0][REG_BANK_SEL] >> 4 & 3) }
    fn mem_addr(&self) -> usize { usize::from(self.regs[0][MEM_BANK_SEL]) << 8 | usize::from(self.regs[0][MEM_START_ADDR]) }
    fn mem16(&self, at: usize) -> u16 { u16::from_be_bytes([self.mem[at & 0xffff], self.mem[(at + 1) & 0xffff]]) }
    fn accel_lsb(&self) -> f64 { f64::from(16384u32 >> (self.regs[2][ACCEL_CONFIG] >> 1 & 3)) }
    fn gyro_lsb(&self) -> f64 { 131.0 / f64::from(1u32 << (self.regs[2][GYRO_CONFIG_1] >> 1 & 3)) }

    /// Data registers, INT_STATUS_1, then the DMP's packets up to now.
    fn latch(&mut self) {
        let s = sample(&self.input.get());
        let pwr2 = self.regs[0][PWR_MGMT_2];
        let (al, gl) = (self.accel_lsb(), self.gyro_lsb());
        for axis in 0..3 {
            let a = if pwr2 & 0x38 != 0x38 { sat16(s.accel[axis] * al) } else { 0 };
            let g = if pwr2 & 0x07 != 0x07 { sat16(s.gyro[axis] * gl) } else { 0 };
            self.regs[0][ACCEL_XOUT_H + 2 * axis..ACCEL_XOUT_H + 2 * axis + 2].copy_from_slice(&a.to_be_bytes());
            self.regs[0][ACCEL_XOUT_H + 6 + 2 * axis..ACCEL_XOUT_H + 8 + 2 * axis].copy_from_slice(&g.to_be_bytes());
        }
        self.regs[0][TEMP_OUT_H..TEMP_OUT_H + 2].copy_from_slice(&sat16(4.0 * 333.87).to_be_bytes());
        self.regs[0][INT_STATUS_1] = 1;
        self.pump();
    }

    /// Run the DMP's sample clock to the current guest time.
    fn pump(&mut self) {
        let running = self.regs[0][USER_CTRL] & 0xc0 == 0xc0 && self.regs[0][PWR_MGMT_1] & 0x40 == 0;
        if !running { self.next_sample = None; return; }
        let now = self.motion.cycle.load(Relaxed);
        let period = (self.clock_hz.saturating_mul(1 + u64::from(self.regs[2][GYRO_SMPLRT_DIV])) / 1100).max(1);
        let mut next = *self.next_sample.get_or_insert(now.saturating_add(period));
        let mut emitted = 0;
        while next <= now {
            // After a long stall only the latest few samples matter (the FIFO holds ~12 packets).
            if emitted == 64 { next = now.saturating_add(period); break; }
            self.emit();
            next = next.saturating_add(period);
            emitted += 1;
        }
        self.next_sample = Some(next);
    }

    /// One DMP sample: the blocks DATA_OUT_CTL1 enables whose ODR divider is due.
    fn emit(&mut self) {
        let index = self.samples;
        self.samples += 1;
        let ctl1 = self.mem16(DATA_OUT_CTL1);
        let due = |odr: usize| index.is_multiple_of(u64::from(self.mem16(odr)) + 1);
        let mut header = 0;
        for (bit, odr) in [(OUT_ACCEL, ODR_ACCEL), (OUT_GYRO, ODR_GYRO), (OUT_COMPASS, ODR_CPASS), (OUT_QUAT6, ODR_QUAT6), (OUT_QUAT9, ODR_QUAT9)] {
            if ctl1 & bit != 0 && due(odr) { header |= bit; }
        }
        if header == 0 { return; }
        let s = sample(&self.input.get());
        let mut p = header.to_be_bytes().to_vec();
        let push16 = |p: &mut Vec<u8>, v: [f64; 3]| for c in v { p.extend_from_slice(&sat16(c).to_be_bytes()); };
        if header & OUT_ACCEL != 0 { let l = self.accel_lsb(); push16(&mut p, s.accel.map(|a| a * l)); }
        if header & OUT_GYRO != 0 { let l = self.gyro_lsb(); push16(&mut p, s.gyro.map(|g| g * l)); push16(&mut p, [0.0; 3]); }
        if header & OUT_COMPASS != 0 { push16(&mut p, s.field.map(|b| b / AK_UT_PER_LSB)); }
        // Q0 >= 0: the library recomputes it as the positive root.
        let q = if s.q[0] < 0.0 { s.q.map(|c| -c) } else { s.q };
        let q30 = |c: f64| ((c * 1073741824.0).round().clamp(-2147483648.0, 2147483647.0) as i32).to_be_bytes();
        for (bit, accuracy) in [(OUT_QUAT6, false), (OUT_QUAT9, true)] {
            if header & bit != 0 {
                for c in &q[1..] { p.extend_from_slice(&q30(*c)); }
                if accuracy { p.extend_from_slice(&[0, 0]); }
            }
        }
        p.extend_from_slice(&[0, 0]);   // footer
        if self.fifo.len() + p.len() > FIFO_SIZE { self.dropped += 1; return; }
        self.fifo.extend(p);
    }

    /// SLV4 single transfer, started by setting I2C_SLV4_CTRL.EN.
    fn slv4(&mut self) {
        let r3 = self.regs[3];
        self.regs[3][I2C_SLV4_CTRL] &= !0x80;
        if self.regs[0][USER_CTRL] & 0x20 == 0 { return; }    // master off: nothing runs
        let mut status = 0x40;                                  // I2C_SLV4_DONE
        if r3[I2C_SLV4_ADDR] & 0x7f != AK09916_ADDRESS { status |= 0x10; } else {
            let field = sample(&self.input.get()).field;
            let reg_dis = r3[I2C_SLV4_CTRL] & 0x20 != 0;
            if r3[I2C_SLV4_ADDR] & 0x80 != 0 {
                if !reg_dis { self.ak.ptr = r3[I2C_SLV4_REG]; }
                self.regs[3][I2C_SLV4_DI] = self.ak.read(field);
            } else {
                let reg = if reg_dis { self.ak.ptr } else { r3[I2C_SLV4_REG] };
                self.ak.write(reg, r3[I2C_SLV4_DO], field);
            }
        }
        self.regs[0][I2C_MST_STATUS] |= status;
    }

    /// SLV0-3 as the master's sample cycle runs them, filling EXT_SLV_SENS_DATA.
    fn slaves(&mut self) {
        if self.regs[0][USER_CTRL] & 0x20 == 0 { return; }
        let field = sample(&self.input.get()).field;
        let mut ext = [0u8; 24];
        let mut at = 0;
        let mut nack = 0;
        for n in 0..4 {
            let [addr, reg, ctrl, dout] = [0, 1, 2, 3].map(|i| self.regs[3][I2C_SLV0_ADDR + 4 * n + i]);
            if ctrl & 0x80 == 0 { continue; }
            if addr & 0x7f != AK09916_ADDRESS { nack |= 1 << n; continue; }
            let reg_dis = ctrl & 0x20 != 0;
            if addr & 0x80 == 0 { self.ak.write(if reg_dis { self.ak.ptr } else { reg }, dout, field); continue; }
            if !reg_dis { self.ak.ptr = reg; }
            let len = usize::from(ctrl & 0x0f).min(24 - at);
            let mut bytes: Vec<u8> = (0..len).map(|_| self.ak.read(field)).collect();
            // BYTE_SW swaps each pair; GRP 1 starts the pairs at the second byte.
            if ctrl & 0x40 != 0 {
                let mut i = usize::from(ctrl >> 4 & 1);
                while i + 1 < len { bytes.swap(i, i + 1); i += 2; }
            }
            ext[at..at + len].copy_from_slice(&bytes);
            at += len;
        }
        self.regs[0][EXT_SLV_SENS_DATA_00..EXT_SLV_SENS_DATA_00 + 24].copy_from_slice(&ext);
        self.regs[0][I2C_MST_STATUS] |= nack;
    }

    fn store(&mut self, b: u8) {
        let (bank, r) = (self.bank(), usize::from(self.ptr));
        match (bank, r) {
            (_, REG_BANK_SEL) => self.regs[0][REG_BANK_SEL] = b & 0x30,
            (0, USER_CTRL) => {
                self.regs[0][USER_CTRL] = b & !0x0e;
                if b & 0x08 != 0 || b & 0x80 == 0 { self.next_sample = None; self.samples = 0; }  // DMP_RST or DMP off
            }
            (0, PWR_MGMT_1) => if b & 0x80 != 0 { self.reset(); } else { self.regs[0][PWR_MGMT_1] = b; },
            (0, FIFO_RST) => { self.regs[0][FIFO_RST] = b; if b & 0x1f != 0 { self.fifo.clear(); self.count_latch = None; } }
            (0, MEM_R_W) => {
                let at = self.mem_addr();
                self.mem[at] = b;
                self.regs[0][MEM_START_ADDR] = self.regs[0][MEM_START_ADDR].wrapping_add(1);
                return;
            }
            (0, FIFO_R_W) => return,
            // Read-only: WHO_AM_I, status, data, external data, FIFO count.
            (0, 0x00 | 0x17..=0x1c | 0x2d..=0x52 | FIFO_COUNTH | FIFO_COUNTL) => {}
            (3, I2C_SLV4_DI) => {}
            (3, I2C_SLV4_CTRL) => { self.regs[3][I2C_SLV4_CTRL] = b; if b & 0x80 != 0 { self.slv4(); } }
            _ => self.regs[bank][r] = b,
        }
        self.ptr = (self.ptr + 1) & 0x7f;
    }

    fn load(&mut self) -> u8 {
        let (bank, r) = (self.bank(), usize::from(self.ptr));
        let v = match (bank, r) {
            (_, REG_BANK_SEL) => self.regs[0][REG_BANK_SEL],
            (0, I2C_MST_STATUS) => std::mem::take(&mut self.regs[0][I2C_MST_STATUS]),
            (0, EXT_SLV_SENS_DATA_00) => { self.slaves(); self.regs[0][r] }
            (0, FIFO_COUNTH) => { let n = self.fifo.len() as u16; self.count_latch = Some(n); (n >> 8) as u8 & 0x1f }
            (0, FIFO_COUNTL) => (self.count_latch.take().unwrap_or(self.fifo.len() as u16) & 0xff) as u8,
            (0, FIFO_R_W) => return self.fifo.pop_front().unwrap_or(0xff),
            (0, MEM_R_W) => {
                let v = self.mem[self.mem_addr()];
                self.regs[0][MEM_START_ADDR] = self.regs[0][MEM_START_ADDR].wrapping_add(1);
                return v;
            }
            _ => self.regs[bank][r],
        };
        self.ptr = (self.ptr + 1) & 0x7f;
        v
    }
}

impl I2cDevice for Icm20948 {
    fn start(&mut self, read: bool) -> bool {
        if read { self.latch(); } else { self.first = true; }
        true
    }
    fn write(&mut self, b: u8) -> bool {
        if self.first { self.ptr = b & 0x7f; self.first = false; } else { self.store(b); }
        true
    }
    fn read(&mut self) -> u8 { self.load() }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wr(d: &mut Icm20948, reg: usize, data: &[u8]) { d.start(false); d.write(reg as u8); for &b in data { d.write(b); } d.stop(); }
    fn rd(d: &mut Icm20948, reg: usize, n: usize) -> Vec<u8> { d.start(false); d.write(reg as u8); d.start(true); let v = (0..n).map(|_| d.read()).collect(); d.stop(); v }
    fn bank(d: &mut Icm20948, b: u8) { wr(d, REG_BANK_SEL, &[b << 4]); }
    fn mems_write(d: &mut Icm20948, at: usize, data: &[u8]) {
        bank(d, 0);
        wr(d, MEM_BANK_SEL, &[(at >> 8) as u8]);
        for (i, chunk) in data.chunks(16).enumerate() { wr(d, MEM_START_ADDR, &[(at + 16 * i) as u8]); wr(d, MEM_R_W, chunk); }
    }
    fn mems_read(d: &mut Icm20948, at: usize, n: usize) -> Vec<u8> {
        bank(d, 0);
        wr(d, MEM_BANK_SEL, &[(at >> 8) as u8]);
        (0..n).step_by(16).flat_map(|i| { wr(d, MEM_START_ADDR, &[(at + i) as u8]); rd(d, MEM_R_W, 16.min(n - i)) }).collect()
    }
    /// `ICM_20948_i2c_controller_periph4_txn` for one byte.
    fn slv4(d: &mut Icm20948, addr: u8, reg: u8, write: Option<u8>) -> Result<u8, ()> {
        bank(d, 3);
        wr(d, I2C_SLV4_ADDR, &[addr | if write.is_none() { 0x80 } else { 0 }]);
        wr(d, I2C_SLV4_REG, &[reg]);
        if let Some(b) = write { wr(d, I2C_SLV4_DO, &[b]); }
        wr(d, I2C_SLV4_CTRL, &[0x80]);
        bank(d, 0);
        let status = (0..1000).map(|_| rd(d, I2C_MST_STATUS, 1)[0]).find(|s| s & 0x40 != 0).ok_or(())?;
        if status & 0x10 != 0 { return Err(()); }
        bank(d, 3);
        Ok(rd(d, I2C_SLV4_DI, 1)[0])
    }
    fn i2c_master_on(d: &mut Icm20948) { bank(d, 0); let u = rd(d, USER_CTRL, 1)[0]; wr(d, USER_CTRL, &[u | 0x20]); }

    #[test]
    fn who_am_i_and_banks() {
        let mut d = Icm20948::new(Arc::default(), 1_000_000);
        bank(&mut d, 0);
        assert_eq!(rd(&mut d, 0x00, 1), [0xea]);
        assert_eq!(rd(&mut d, PWR_MGMT_1, 1), [0x41]);
        bank(&mut d, 2);
        assert_eq!(rd(&mut d, 0x00, 2), [0x00, 0x01], "bank 2: GYRO_SMPLRT_DIV, GYRO_CONFIG_1");
        wr(&mut d, ACCEL_CONFIG, &[0x02]);
        assert_eq!(rd(&mut d, ACCEL_CONFIG, 1), [0x02]);
        assert_eq!(rd(&mut d, REG_BANK_SEL, 1), [0x20]);
        bank(&mut d, 0);
        assert_eq!(rd(&mut d, 0x00, 1), [0xea]);
        // swReset: read-modify-write of PWR_MGMT_1 with DEVICE_RESET.
        wr(&mut d, PWR_MGMT_1, &[0x01]);
        wr(&mut d, PWR_MGMT_1, &[0x81]);
        assert_eq!(rd(&mut d, PWR_MGMT_1, 1), [0x41]);
        bank(&mut d, 2);
        assert_eq!(rd(&mut d, ACCEL_CONFIG, 1), [0x01], "reset restores bank 2");
    }

    #[test]
    fn dmp_memory_upload_reads_back() {
        let mut d = Icm20948::new(Arc::default(), 1_000_000);
        // `inv_icm20948_firmware_load`: 16-byte chunks from 0x90 across several pages.
        let image: Vec<u8> = (0..1000u32).map(|i| (i * 7 + 3) as u8).collect();
        let mut at = 0x90;
        for chunk in image.chunks(16) { mems_write(&mut d, at, chunk); at += chunk.len(); }
        let mut at = 0x90;
        for chunk in image.chunks(16) { assert_eq!(mems_read(&mut d, at, chunk.len()), chunk, "at {at:#x}"); at += chunk.len(); }
        assert_eq!(rd(&mut d, MEM_BANK_SEL, 1), [0x04]);
    }

    #[test]
    fn ak09916_through_slv4() {
        let mut d = Icm20948::new(Arc::default(), 1_000_000);
        assert_eq!(slv4(&mut d, AK09916_ADDRESS, 0x01, None), Err(()), "master off: SLV4 never completes");
        i2c_master_on(&mut d);
        assert_eq!(slv4(&mut d, AK09916_ADDRESS, 0x00, None), Ok(0x48));
        assert_eq!(slv4(&mut d, AK09916_ADDRESS, 0x01, None), Ok(0x09));
        assert_eq!(slv4(&mut d, 0x0d, 0x01, None), Err(()), "no device: NACK");
        // Single measurement, then ST1 / HX..HZ / ST2.
        slv4(&mut d, AK09916_ADDRESS, 0x31, Some(0x01)).unwrap();
        assert_eq!(slv4(&mut d, AK09916_ADDRESS, 0x31, None), Ok(0), "single mode returns to power-down");
        assert_eq!(slv4(&mut d, AK09916_ADDRESS, 0x10, None), Ok(1));
        let hx = i16::from_le_bytes([slv4(&mut d, AK09916_ADDRESS, 0x11, None).unwrap(), slv4(&mut d, AK09916_ADDRESS, 0x12, None).unwrap()]);
        let hz = i16::from_le_bytes([slv4(&mut d, AK09916_ADDRESS, 0x15, None).unwrap(), slv4(&mut d, AK09916_ADDRESS, 0x16, None).unwrap()]);
        assert_eq!((hx, hz), (133, 267), "20 uT on x, -40 uT on z with the AK's z flipped, 0.15 uT/LSB");
        slv4(&mut d, AK09916_ADDRESS, 0x18, None).unwrap();
        assert_eq!(slv4(&mut d, AK09916_ADDRESS, 0x10, None), Ok(0), "ST2 read ends the measurement");
        // initializeDMP's SLV0: 10 bytes from RSV2 with GRP and BYTE_SW: little endian again.
        bank(&mut d, 3);
        wr(&mut d, I2C_SLV0_ADDR, &[0x80 | AK09916_ADDRESS, 0x03, 0x80 | 0x40 | 0x10 | 10]);
        slv4(&mut d, AK09916_ADDRESS, 0x31, Some(0x01)).unwrap();
        bank(&mut d, 0);
        let ext = rd(&mut d, EXT_SLV_SENS_DATA_00, 10);
        assert_eq!(ext[0], 1, "ST1 DRDY");
        assert_eq!([i16::from_le_bytes([ext[1], ext[2]]), i16::from_le_bytes([ext[5], ext[6]])], [133, 267]);
    }

    #[test]
    fn raw_registers_follow_the_pose() {
        let mut d = Icm20948::new(Arc::default(), 1_000_000);
        let input = d.input();
        bank(&mut d, 0);
        let v = rd(&mut d, ACCEL_XOUT_H, 12);
        assert_eq!(i16::from_be_bytes([v[4], v[5]]), 16384, "+1 g on z at the reset +-2 g");
        input.set_angular_rate([1.0, 0.0, 0.0]);
        bank(&mut d, 2);
        wr(&mut d, GYRO_CONFIG_1, &[0x00]);
        bank(&mut d, 0);
        let v = rd(&mut d, ACCEL_XOUT_H, 12);
        assert_eq!(i16::from_be_bytes([v[6], v[7]]), 7506, "1 rad/s = 57.3 dps at 131 LSB/dps");
        // Rolled 90 degrees about x: gravity's reaction now along +y.
        let h = std::f64::consts::FRAC_1_SQRT_2;
        input.set_orientation([h, h, 0.0, 0.0]);
        let v = rd(&mut d, ACCEL_XOUT_H, 6);
        assert_eq!([i16::from_be_bytes([v[2], v[3]]), i16::from_be_bytes([v[4], v[5]])], [16384, 0]);
    }

    #[test]
    fn dmp_fifo_packets() {
        let motion = Arc::new(ImuMotion::default());
        let mut d = Icm20948::new(motion.clone(), 1_000_000);
        let input = d.input();
        // What the firmware's setup leaves: +-4 g, 2000 dps, 55 Hz, Quat9 + raw accel/gyro +
        // uncalibrated compass (DATA_OUT_CTL1 0xE448 incl. Gyro_Calibr and Header2 bits).
        bank(&mut d, 0);
        wr(&mut d, PWR_MGMT_1, &[0x01]);
        bank(&mut d, 2);
        wr(&mut d, GYRO_SMPLRT_DIV, &[19]);
        wr(&mut d, GYRO_CONFIG_1, &[0x07]);
        wr(&mut d, ACCEL_CONFIG, &[0x02]);
        mems_write(&mut d, DATA_OUT_CTL1, &[0xe4, 0x48]);
        mems_write(&mut d, ODR_ACCEL, &[0x00, 0x03]);
        wr(&mut d, USER_CTRL, &[0xc0]);
        wr(&mut d, USER_CTRL, &[0xc8]);
        assert_eq!(rd(&mut d, FIFO_COUNTH, 2), [0, 0]);
        let h = std::f64::consts::FRAC_1_SQRT_2;
        input.set_orientation([h, 0.0, 0.0, h]);    // yawed 90 degrees left
        motion.cycle.store(1_000_000 * 20 / 1100, Relaxed);
        let c = rd(&mut d, FIFO_COUNTH, 1)[0];
        let n = usize::from(c) << 8 | usize::from(rd(&mut d, FIFO_COUNTL, 1)[0]);
        assert_eq!(n, 42, "sample 0: accel + gyro + compass + quat9 + header + footer");
        let mut take = |k| rd(&mut d, FIFO_R_W, k);
        let header = u16::from_be_bytes(take(2).try_into().unwrap());
        assert_eq!(header, 0xe400);
        let be = |b: &[u8]| -> Vec<i16> { b.chunks(2).map(|c| i16::from_be_bytes([c[0], c[1]])).collect() };
        assert_eq!(be(&take(6)), [0, 0, 8192], "raw accel at +-4 g");
        assert_eq!(be(&take(12)), [0; 6], "raw gyro and bias");
        assert_eq!(be(&take(6)), [0, -133, -267], "north now along -y");
        let q: Vec<i32> = take(12).chunks(4).map(|c| i32::from_be_bytes(c.try_into().unwrap())).collect();
        assert_eq!(q, [0, 0, (h * 1073741824.0).round() as i32]);
        assert_eq!(take(4), [0; 4], "heading accuracy, footer");
        // The next sample has the accel divider (interval 3) not due: no accel block.
        motion.cycle.store(1_000_000 * 40 / 1100, Relaxed);
        assert_eq!(rd(&mut d, FIFO_COUNTH, 2), [0, 36]);
        assert_eq!(rd(&mut d, FIFO_R_W, 2), [0x64, 0x00]);
        // A long stall fills the FIFO with whole packets only; FIFO_RST empties it.
        motion.cycle.store(1_000_000, Relaxed);
        let n = u16::from_be_bytes(rd(&mut d, FIFO_COUNTH, 2).try_into().unwrap());
        assert!(n > 400 && n <= 512, "{n}");
        assert!(d.dropped > 0);
        wr(&mut d, FIFO_RST, &[0x1f]);
        wr(&mut d, FIFO_RST, &[0x1e]);
        assert_eq!(rd(&mut d, FIFO_COUNTH, 2), [0, 0]);
        assert_eq!(rd(&mut d, FIFO_R_W, 1), [0xff], "empty FIFO");
    }
}
