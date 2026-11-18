//! An AHT20 temperature/humidity sensor on I2C (0x38), as esp-idf-lib's `aht20` component drives
//! it: a `0xAC 0x33 0x00` write starts a conversion, the status byte reads 0x18 once the
//! calibration bit (bit 3) is set and the busy bit (bit 7) is clear, and a seven-byte read returns
//! that status followed by a 20-bit humidity and a 20-bit temperature. The temperature's top
//! nibble shares the humidity's last data byte, exactly as the datasheet packs them.
//!
//! Unlike a register-mapped part the AHT20 has no register pointer: each transaction is either a
//! command or a read of the sensor's byte stream, so this model keeps a byte cursor rather than a
//! memory map.
use esp_periph::i2c::I2cDevice;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::imu_motion::{sin, ImuMotion};

/// AHT10 and AHT20 share the fixed address.
pub const AHT20_ADDRESS: u8 = 0x38;

/// Calibrated and idle: bit 4 (calibrated) and bit 3 (ready) set, busy clear.
const STATUS_READY: u8 = 0x18;

const CMD_TRIGGER: u8 = 0xAC;
const CMD_RESET: u8 = 0xBE;

/// 2^20: both channels are 20-bit.
const FULL_SCALE: f32 = 1_048_576.0;

pub struct Aht20 {
    /// Bytes written in the current transaction: a trigger/reset is three, a status read one.
    cmd: [u8; 8],
    cmd_len: usize,
    /// Cursor into `sample` for the current read transaction.
    rx_pos: usize,
    /// Status byte then humidity and temperature, packed as the sensor sends them.
    sample: [u8; 7],
    pub status: u8,
    /// Conversions the model has completed (a trigger that packed a new reading).
    pub conversions: u64,
    temperature: f32,
    humidity: f32,
    /// Shared motion selector: mode 0 holds the reading still, mode 1 drifts it over conversions.
    motion: Arc<ImuMotion>,
}

impl Default for Aht20 {
    fn default() -> Self { Self::new() }
}

impl Aht20 {
    pub fn new() -> Self { Self::with_motion(Arc::new(ImuMotion::default())) }

    /// Share the board's motion selector so the same `--imu-motion` switch that moves an IMU also
    /// drifts this sensor. Deterministic by construction: the phase is the conversion count.
    pub fn with_motion(motion: Arc<ImuMotion>) -> Self {
        Self {
            cmd: [0; 8],
            cmd_len: 0,
            rx_pos: 0,
            sample: [STATUS_READY, 0, 0, 0, 0, 0, 0],
            status: STATUS_READY,
            conversions: 0,
            temperature: 25.0,
            humidity: 50.0,
            motion,
        }
    }

    /// The reading the next conversion publishes.
    pub fn set_reading(&mut self, temperature: f32, humidity: f32) {
        self.temperature = temperature;
        self.humidity = humidity;
    }

    /// The last conversion's reading, as a driver would decode it.
    pub fn reading(&self) -> (f32, f32) { (self.temperature, self.humidity) }

    /// A conversion finishes at once in the model: a real part needs ~80 ms, but tying that to wall
    /// clock would only slow a run down, so the sample is ready as soon as the trigger is taken.
    fn pack(&mut self) {
        // Mode 1 ("the 30 s handling script" switch) turns the fixed reading into a slow drift:
        // a warm room and its humidity wandering, sampled at the conversion count. Keeping the
        // phase on an integer counter rather than wall clock is what makes runs reproducible.
        if self.motion.mode.load(Ordering::Relaxed) != 0 {
            let t = self.conversions as f64 * 0.5;
            self.temperature = (25.0 + 2.5 * sin(t * 0.13)) as f32;
            self.humidity = (50.0 + 12.0 * sin(t * 0.07)) as f32;
        }
        let h = (self.humidity.clamp(0.0, 100.0) / 100.0 * FULL_SCALE).round() as u32;
        let t = ((self.temperature.clamp(-50.0, 150.0) + 50.0) / 200.0 * FULL_SCALE).round() as u32;
        self.sample[0] = STATUS_READY;
        self.sample[1] = (h >> 12) as u8;
        self.sample[2] = (h >> 4) as u8;
        self.sample[3] = ((h & 0x0f) << 4) as u8 | ((t >> 16) & 0x0f) as u8;
        self.sample[4] = (t >> 8) as u8;
        self.sample[5] = t as u8;
        self.sample[6] = 0;
        self.status = STATUS_READY;
        self.conversions += 1;
    }
}

impl I2cDevice for Aht20 {
    fn start(&mut self, read: bool) -> bool {
        if read { self.rx_pos = 0; } else { self.cmd_len = 0; }
        true
    }

    fn write(&mut self, b: u8) -> bool {
        if self.cmd_len < self.cmd.len() {
            self.cmd[self.cmd_len] = b;
            self.cmd_len += 1;
        }
        if self.cmd_len == 3 && self.cmd[0] == CMD_TRIGGER {
            self.pack();
        } else if self.cmd_len == 1 && self.cmd[0] == CMD_RESET {
            // Recalibration is instantaneous here; the next status read already reports ready.
            self.status = STATUS_READY;
        }
        true
    }

    fn read(&mut self) -> u8 {
        let v = if self.rx_pos < self.sample.len() { self.sample[self.rx_pos] } else { 0 };
        self.rx_pos += 1;
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trigger(d: &mut Aht20) {
        d.start(false);
        for b in [0xAC, 0x33, 0x00] { d.write(b); }
    }
    fn read_bytes(d: &mut Aht20, n: usize) -> Vec<u8> {
        d.start(true);
        (0..n).map(|_| d.read()).collect()
    }

    #[test]
    fn status_reports_calibrated_and_idle() {
        let mut d = Aht20::new();
        let s = read_bytes(&mut d, 1)[0];
        assert_eq!(s & 0x80, 0, "busy must be clear");
        assert_eq!(s & 0x08, 0x08, "calibration bit must be set");
        assert_eq!(s, STATUS_READY);
    }

    #[test]
    fn a_trigger_publishes_the_configured_reading() {
        let mut d = Aht20::new();
        // The values from the project's boot log: 31.9 °C / 63.9 %.
        d.set_reading(31.9, 63.9);
        trigger(&mut d);
        assert_eq!(d.conversions, 1);

        let b = read_bytes(&mut d, 7);
        let raw_h = ((b[1] as u32) << 16 | (b[2] as u32) << 8 | b[3] as u32) >> 4;
        let raw_t = ((b[3] as u32 & 0x0f) << 16 | (b[4] as u32) << 8 | b[5] as u32);
        let humidity = raw_h as f32 * 100.0 / FULL_SCALE;
        let temperature = raw_t as f32 * 200.0 / FULL_SCALE - 50.0;
        assert!((humidity - 63.9).abs() < 0.1, "humidity {humidity}");
        assert!((temperature - 31.9).abs() < 0.1, "temperature {temperature}");
    }

    #[test]
    fn the_seven_byte_read_starts_with_the_status_byte() {
        let mut d = Aht20::new();
        trigger(&mut d);
        let b = read_bytes(&mut d, 7);
        assert_eq!(b[0], STATUS_READY);
        assert_eq!(b.len(), 7);
    }

    #[test]
    fn a_reset_clears_the_trigger_and_reads_ready() {
        let mut d = Aht20::new();
        d.start(false);
        for b in [0xBE, 0x08, 0x00] { d.write(b); }
        assert_eq!(read_bytes(&mut d, 1)[0], STATUS_READY);
        assert_eq!(d.conversions, 0, "a reset is not a conversion");
    }

    #[test]
    fn motion_mode_one_drifts_the_reading() {
        let motion = Arc::new(ImuMotion::default());
        motion.mode.store(1, Ordering::Relaxed);
        let mut d = Aht20::with_motion(motion);
        let mut temps = Vec::new();
        for _ in 0..12 {
            trigger(&mut d);
            temps.push(d.reading().0);
        }
        assert!(temps.iter().any(|&t| (t - temps[0]).abs() > 0.05), "temperature must drift: {temps:?}");
        assert!(temps.iter().all(|&t| (15.0..35.0).contains(&t)), "drift stays plausible: {temps:?}");
    }

    #[test]
    fn motion_mode_zero_keeps_the_reading_still() {
        let motion = Arc::new(ImuMotion::default());
        let mut d = Aht20::with_motion(motion);
        let mut temps = Vec::new();
        for _ in 0..5 {
            trigger(&mut d);
            temps.push(d.reading().0);
        }
        assert!(temps.iter().all(|&t| (t - 25.0).abs() < f32::EPSILON), "mode 0 must not move: {temps:?}");
    }
}
