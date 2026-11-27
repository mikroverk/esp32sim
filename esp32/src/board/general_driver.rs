//! Waveshare's General Driver for Robots (ESP32-WROOM-32), as the UGV Rover carries it, with the
//! wiring of Waveshare's ugv_base_ros firmware (ROS_Driver/ugv_config.h at 2e7df97): a TB6612FNG
//! dual H-bridge (PWMA 25, AIN1 21, AIN2 17; PWMB 26, BIN1 22, BIN2 23) driving the left (A) and
//! right (B) gear motors, their Hall encoders (A: 35, 34; B: 27, 16), and on I2C0 (SDA 32, SCL 33)
//! an ICM-20948 IMU at 0x68 (AD0 low, AK09916 magnetometer behind it), an SSD1306 128x32 OLED at 0x3C
//! and an INA219 battery monitor at 0x42 with a 0.01 ohm shunt
//! (battery_ctrl.h). UART0 is the host link (the Jetson's 40-pin header, JSON lines at 115200).
//! Waveshare's schematic (ROS_Driver_for_Robots.pdf) agrees: INA219 at 0x42 across a 0.01 ohm
//! shunt, encoders on IO34/35 and IO27/16, and a second TB6612FNG on the same six control lines
//! for the rear motors, which have no encoder. Each side's two motors run in parallel, so one wheel
//! per side models both.
//!
//! The wheels are a first-order model, not a physics claim: each motor's target speed is
//! proportional to its PWM duty above a dead band, signed by the H-bridge inputs (TB6612FNG
//! datasheet table: IN1 high and IN2 low forward, the reverse backward, both equal stop/brake), and
//! the wheel follows it with a time constant. The encoders turn that into quadrature edges on the
//! encoder pins at 660 counts per wheel turn (halfQuad: both edges of A) and an 80 mm wheel, so the
//! firmware's odometry (movtion_module.h: plusesRate = pi * WHEEL_D / ONE_CIRCLE_PLUSES) reads the
//! modelled speed. Forward is the phase ESP32Encoder 0.11 counts up in halfQuad mode (B leads A).
use std::sync::{Arc, Mutex};
use esp_periph::i2c::I2cDevice;
use esp_soc::board::{BoardEdge, BoardModel};
use esp_soc::devices::{Icm20948, Icm20948Input, ICM20948_ADDRESS, imu_motion::ImuMotion, Ina219, Ina219Input, Ina219Shared, Ssd1306, Ssd1306Shared, SSD1306_ADDRESS};

const CPU_HZ: u64 = crate::periph::CPU_HZ;
/// The model's step: wheel speeds change once per millisecond.
const TICK: u64 = CPU_HZ / 1000;
pub const INA219_ADDRESS: u8 = 0x42;
/// ugv_config.h, mainType 02 (UGV Rover): WHEEL_D, ONE_CIRCLE_PLUSES.
pub const WHEEL_D: f64 = 0.08;
pub const COUNTS_PER_TURN: f64 = 660.0;
/// Encoder states (one edge on A or B) per wheel turn: halfQuad counts only A's edges, half of them.
const STEPS_PER_TURN: f64 = 2.0 * COUNTS_PER_TURN;
/// The speed at full duty is the product page's "Default Max speed: 1.3m/s"; model assumptions
/// (not from a datasheet): the duty below which the motor stalls (the firmware's THRESHOLD_PWM 23
/// of 255) and the time constant.
pub const FULL_SPEED: f64 = 1.3;
const DEAD_BAND: f64 = 23.0 / 255.0;
const TAU_S: f64 = 0.05;

/// One motor channel: its pins and the wheel's state.
#[derive(Clone, Copy, Debug)]
struct Wheel { pwm: u8, in1: u8, in2: u8, enc_a: u8, enc_b: u8, speed: f64, steps: f64, phase: u8, target: f64 }
impl Wheel {
    const fn new(pwm: u8, in1: u8, in2: u8, enc_a: u8, enc_b: u8) -> Self { Wheel { pwm, in1, in2, enc_a, enc_b, speed: 0.0, steps: 0.0, phase: 0, target: 0.0 } }
    /// Encoder levels (A, B) of quadrature phase 0..3; forward runs B up, A up, B down, A down.
    fn levels(phase: u8) -> (bool, bool) { match phase & 3 { 0 => (false, false), 1 => (false, true), 2 => (true, true), _ => (true, false) } }
    fn steps_per_s(&self) -> f64 { self.speed / (std::f64::consts::PI * WHEEL_D) * STEPS_PER_TURN }
}

/// The wheels as the rest of the simulation sees them: drive in, speeds and travel out. A physics
/// simulation replaces the built-in model by setting `external` and writing `speed_mps`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Drive {
    /// Signed duty, -1..1, per wheel (left, right), from the H-bridge inputs and PWM.
    pub duty: [f64; 2],
    /// Wheel surface speed in m/s (left, right).
    pub speed_mps: [f64; 2],
    /// Distance each wheel has rolled, in metres.
    pub travel_m: [f64; 2],
    /// Speeds come from outside (`speed_mps` as written) instead of the built-in motor model.
    pub external: bool,
}
pub type DriveShared = Arc<Mutex<Drive>>;

pub struct GeneralDriver {
    wheels: [Wheel; 2],
    output: u64,
    now: u64,
    next_tick: Option<u64>,
    /// Edges waiting for their moment (cycle-ordered per wheel).
    pending: Vec<BoardEdge>,
    edges: Vec<BoardEdge>,
    pub drive: DriveShared,
    pub battery: Ina219Shared,
    pub oled: Ssd1306Shared,
    /// The IMU's pose (level and at rest by default); a physics simulation sets it.
    pub imu: Option<Icm20948Input>,
    motion: Arc<ImuMotion>,
}
impl Default for GeneralDriver { fn default() -> Self { Self::new() } }
impl GeneralDriver {
    pub fn new() -> Self {
        GeneralDriver {
            wheels: [Wheel::new(25, 21, 17, 35, 34), Wheel::new(26, 22, 23, 27, 16)],
            output: 0, now: 0, next_tick: None, pending: Vec::new(), edges: Vec::new(),
            drive: Default::default(),
            // three 18650 cells, mostly charged; the electronics' own draw
            battery: Arc::new(Mutex::new(Ina219Input { bus_v: 11.8, current_a: 0.25, shunt_ohm: 0.01 })),
            oled: Default::default(),
            imu: None, motion: Default::default(),
        }
    }
    fn level(&self, pin: u8) -> bool { self.output >> pin & 1 != 0 }
    /// Signed duty of wheel `i` now: the H-bridge inputs give the sign, the PWM the magnitude.
    fn duty(&self, i: usize, clock: &dyn Fn(u8) -> Option<(f64, u32)>) -> f64 {
        let w = &self.wheels[i];
        let sign = match (self.level(w.in1), self.level(w.in2)) { (true, false) => 1.0, (false, true) => -1.0, _ => return 0.0 };
        let duty = clock(w.pwm).map_or(0.0, |(hz, d)| if hz.is_finite() && hz > 0.0 { f64::from(d) / 65535.0 } else { 0.0 });
        sign * duty
    }
    /// One model step at `at`: speeds move toward their targets, then the edges of the next step are queued.
    fn step(&mut self, at: u64) {
        let dt = TICK as f64 / CPU_HZ as f64;
        let mut d = self.drive.lock().unwrap();
        let k = 1.0 - (-dt / TAU_S).exp();
        for (i, w) in self.wheels.iter_mut().enumerate() {
            if d.external { w.speed = if d.speed_mps[i].is_finite() { d.speed_mps[i].clamp(-10.0, 10.0) } else { 0.0 }; } else { w.speed += (w.target - w.speed) * k; }
            if w.speed.abs() < 1e-4 && w.target == 0.0 { w.speed = 0.0; }
            d.speed_mps[i] = w.speed;
            // the encoder states this wheel crosses during the step, spread evenly across it
            // The phase is floor(steps) & 3: going forward it changes when steps reaches k, going
            // back when it drops below k, for each integer k crossed.
            let (s0, s1) = (w.steps, w.steps + w.steps_per_s() * dt);
            let (lo, hi) = (s0.min(s1).floor() as i64 + 1, s0.max(s1).floor() as i64);
            let forward = s1 > s0;
            let crossings: Vec<i64> = if forward { (lo..=hi).collect() } else { (lo..=hi).rev().collect() };
            for k in crossings.into_iter().take(1000) {
                let cycle = at + ((k as f64 - s0) / (s1 - s0) * TICK as f64).clamp(0.0, TICK as f64) as u64;
                let before = Wheel::levels(w.phase);
                w.phase = if forward { w.phase.wrapping_add(1) } else { w.phase.wrapping_sub(1) } & 3;
                let (a, b) = Wheel::levels(w.phase);
                if a != before.0 { self.pending.push(BoardEdge { cycle, pin: w.enc_a, level: a }); }
                if b != before.1 { self.pending.push(BoardEdge { cycle, pin: w.enc_b, level: b }); }
            }
            w.steps = s1;
            d.travel_m[i] = w.steps / STEPS_PER_TURN * std::f64::consts::PI * WHEEL_D;
        }
        self.pending.sort_by_key(|e| e.cycle);
        let moving = self.wheels.iter().any(|w| w.speed != 0.0 || w.target != 0.0) || d.external;
        self.next_tick = if moving { at.checked_add(TICK) } else { None };
    }
    fn release(&mut self, cycle: u64) {
        let due = self.pending.iter().take_while(|e| e.cycle <= cycle).count();
        self.edges.extend(self.pending.drain(..due));
    }
}

/// Shares one controller between the bus (as an I2C device) and the board.
struct Shared<T>(Arc<Mutex<T>>);
impl<T: I2cDevice> I2cDevice for Shared<T> {
    fn start(&mut self, read: bool) -> bool { self.0.lock().unwrap().start(read) }
    fn write(&mut self, b: u8) -> bool { self.0.lock().unwrap().write(b) }
    fn read(&mut self) -> u8 { self.0.lock().unwrap().read() }
    fn stop(&mut self) { self.0.lock().unwrap().stop() }
}

impl BoardModel for GeneralDriver {
    fn name(&self) -> &'static str { "waveshare-general-driver" }
    fn input_levels(&self) -> Vec<(u8, bool)> {
        self.wheels.iter().flat_map(|w| { let (a, b) = Wheel::levels(w.phase); [(w.enc_a, a), (w.enc_b, b)] }).collect()
    }
    fn gpio_output_at(&mut self, _: u64, _: &[(u8, bool)], _enabled: u64, output: u64) { self.output = output; }
    fn next_deadline(&self) -> Option<u64> { self.pending.first().map(|e| e.cycle).into_iter().chain(self.next_tick).min() }
    fn advance_to(&mut self, cycle: u64) {
        if cycle < self.now { return; }
        self.motion.cycle.store(cycle, std::sync::atomic::Ordering::Relaxed);
        while let Some(at) = self.next_tick.filter(|&at| at <= cycle) { self.release(at); self.step(at); }
        self.release(cycle);
        self.now = cycle;
    }
    fn advance_inputs_with_clock(&mut self, cycle: u64, clock: &dyn Fn(u8) -> Option<(f64, u32)>) {
        if cycle < self.now { return; }
        self.advance_to(cycle);
        let mut d = self.drive.lock().unwrap();
        let mut started = false;
        for i in 0..2 {
            let duty = self.duty(i, clock);
            d.duty[i] = duty;
            let target = if duty.abs() <= DEAD_BAND { 0.0 } else { duty.signum() * (duty.abs() - DEAD_BAND) / (1.0 - DEAD_BAND) * FULL_SPEED };
            started |= target != 0.0 && self.wheels[i].target == 0.0;
            self.wheels[i].target = target;
        }
        drop(d);
        if started && self.next_tick.is_none() { self.next_tick = cycle.checked_add(TICK); }
    }
    fn take_edges(&mut self) -> Vec<BoardEdge> { std::mem::take(&mut self.edges) }
    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn I2cDevice>)> {
        let oled = Arc::new(Mutex::new(Ssd1306::new(self.oled.clone())));
        let ina = Arc::new(Mutex::new(Ina219::new(self.battery.clone())));
        // a reset makes a new IMU: it keeps the pose the old one had
        let icm = Icm20948::new(self.motion.clone(), CPU_HZ);
        if let Some(old) = &self.imu { icm.input().set(old.get()); }
        self.imu = Some(icm.input());
        vec![(0, ICM20948_ADDRESS, Box::new(icm)), (0, SSD1306_ADDRESS, Box::new(Shared(oled))), (0, INA219_ADDRESS, Box::new(Shared(ina)))]
    }
    fn display(&self) -> Option<(u32, u32, Vec<u16>, u64)> {
        let s = self.oled.lock().unwrap();
        Some((128, u32::from(s.rows), s.rgb565(128), s.version))
    }
    fn display_version(&self) -> u64 { self.oled.lock().unwrap().version }
    fn report(&self) -> String {
        let d = self.drive.lock().unwrap();
        format!("[emu] general-driver: wheels L {:+.3} m/s {:+.3} m, R {:+.3} m/s {:+.3} m; duty L {:+.2} R {:+.2}",
                d.speed_mps[0], d.travel_m[0], d.speed_mps[1], d.travel_m[1], d.duty[0], d.duty[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the board as the bus does: outputs, then inputs every 100 us with a fixed PWM duty.
    fn run(b: &mut GeneralDriver, output: u64, duty: u32, until: u64) -> Vec<BoardEdge> {
        let mut edges = Vec::new();
        b.gpio_output_at(0, &[], output, output);
        let clock = move |pin: u8| if pin == 25 || pin == 26 { Some((100_000.0, duty)) } else { None };
        let mut c = b.now;
        while c < until { c += CPU_HZ / 10_000; b.advance_inputs_with_clock(c, &clock); edges.extend(b.take_edges()); }
        edges
    }

    #[test]
    fn forward_duty_turns_into_halfquad_counts_at_the_modelled_speed() {
        let mut b = GeneralDriver::new();
        // A forward (AIN1 21 high, AIN2 17 low), B backward (BIN1 22 low, BIN2 23 high), full duty, 1 s
        let edges = run(&mut b, 1 << 21 | 1 << 23, 65535, CPU_HZ);
        let d = *b.drive.lock().unwrap();
        assert!((d.speed_mps[0] - FULL_SPEED).abs() < 0.01 && (d.speed_mps[1] + FULL_SPEED).abs() < 0.01, "{d:?}");
        // halfQuad counts A's edges, signed by B: rising A with B high, falling A with B low count up
        let (mut a, mut bl, mut count) = (false, false, 0i64);
        for e in edges.iter().filter(|e| e.pin == 35 || e.pin == 34) {
            if e.pin == 34 { bl = e.level; continue; }
            count += if e.level == bl { 1 } else { -1 };
            a = e.level;
        }
        let _ = a;
        let metres = count as f64 / COUNTS_PER_TURN * std::f64::consts::PI * WHEEL_D;
        assert!((metres - d.travel_m[0]).abs() < 0.002, "{metres} vs {}", d.travel_m[0]);
        assert!((metres - FULL_SPEED * (1.0 - TAU_S)).abs() < 0.01, "1 s from rest with a 50 ms lag: {metres}");
        assert!(edges.windows(2).all(|w| w[0].cycle <= w[1].cycle), "edges come in time order");
        // stopping: both inputs low, the wheels settle and the model goes quiet
        run(&mut b, 0, 65535, 2 * CPU_HZ);
        assert_eq!(b.drive.lock().unwrap().speed_mps, [0.0, 0.0]);
        assert_eq!(b.next_deadline(), None);
    }

    #[test]
    fn duty_in_the_dead_band_does_not_move() {
        let mut b = GeneralDriver::new();
        let edges = run(&mut b, 1 << 21, 65535 * 20 / 255, CPU_HZ / 10);
        assert!(edges.is_empty());
        assert_eq!(b.drive.lock().unwrap().speed_mps, [0.0, 0.0]);
    }
}
