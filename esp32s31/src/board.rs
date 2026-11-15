//! Boards around the ESP32-S31. One so far: Espressif's ESP-Mosaico (board revision v1.2), wiring
//! from esp-mosaico-bsp (`esp_mosaico.h`) and the Wokwi board-esp-mosaico docs: a CO5300 480 x 480
//! AMOLED on GP-SPI2 in QSPI mode (CLK 42, CS 50, D0-D3 36/51/35/9, TE 43), its RESET on GPIO 44
//! (shared with the touch controller's), and the display supply enabled by GPIO 60 driven low.
//! Modelled: the panel and its reset and power gates, the CST9220 touch on I2C0 (0x5A, SDA 56,
//! SCL 3, INT 6), the AI button on GPIO 7 and the Boot key on GPIO 61, the vibration motor on GPIO 8, the PWR_SW shutdown line on GPIO 57, the BQ27220 fuel gauge (0x55), the BMI270 IMU (0x69) and two BMM150 magnetometers (0x11, 0x12) on I2C0, and the panel's TE output on GPIO 43: a 60 Hz frame with a 1 ms high blanking pulse.
//! Audio: the ES8311 codec (0x19) on I2C0 as a register file with its chip ID, serving the factory
//! firmware's `audio_dac` and `audio_adc` both (no ES7210 is addressed), full duplex on I2S0 (the factory firmware logs BCLK 37, WS 49, DOUT 52, DIN 40) with the
//! speaker amplifier enabled by GPIO 45; what I2S0 transmits is the run's audio (`--wav`).
use esp_soc::devices::co5300::Co5300;
use esp_soc::{Board, BoardModel, NoBoard};

pub fn make_board(name: &str) -> Option<Board> {
    match name {
        "none" | "bare" => Some(Box::new(NoBoard)),
        "mosaico" | "esp-mosaico" => Some(Box::new(Mosaico::new())),
        _ => None,
    }
}

pub const PIN_LCD_RESET: u8 = 44;
pub const PIN_LCD_POWER_N: u8 = 60;
pub const PIN_LCD_TE: u8 = 43;
/// The speaker amplifier's enable (the factory firmware's `gpio_pa_control`), active high.
pub const PIN_PA: u8 = 45;
/// The ES8311 DAC on I2C0: the factory firmware addresses it as 0x32 (8-bit), CE high.
/// Chip ID registers 0xFD/0xFE read 0x83/0x11 as on the other boards' ES8311.
pub const ES8311_ADDRESS: u8 = 0x19;
/// CST9220 INT, active low: low while a finger is down, and for 1 ms after it lifts so the
/// driver reads the release report.
pub const PIN_TOUCH_INT: u8 = 6;
/// The AI button on top (esp-mosaico-bsp `esp_mosaico.h` BSP_BUTTON_AI_GPIO; the factory
/// firmware's board manager calls it `button_power`: single click sleep/back, long press chat).
/// Active low (BSP_BUTTON_ACTIVE_LEVEL 0) with the board's pull-up.
pub const PIN_AI_BUTTON: u8 = 7;
/// The Boot key: BSP_BUTTON_BOOT_GPIO (the factory firmware's `button_boot`), active low. These
/// two are the board's only GPIO keys: the BSP, esp-brookesia's esp_mosaico_v1_2 board files and
/// esp-mosaico-claw read no other button, and there is no power key on a GPIO (power is
/// switched outside the chip; the chip can only request shutdown on `PIN_POWER_SWITCH`).
pub const PIN_BOOT_BUTTON: u8 = 61;
/// The vibration motor: BSP_MOTOR_GPIO, on high (BSP_MOTOR_ON_LEVEL 1). The BSP drives it from
/// LEDC PWM by default (CONFIG_BSP_MOTOR_ENABLE_PWM: low-speed timer 0, 10 bits, 5 kHz, channel
/// 0) or as a plain output. Modelled as on or off; an undriven pin reads off (`pull_downs`: a
/// motor driver's gate pull-down is assumed, the schematic is not public).
pub const PIN_MOTOR: u8 = 8;
/// PWR_SW: BSP_POWER_SWITCH_GPIO, open drain; `bsp_power_set_shutdown(true)` drives it low
/// (BSP_POWER_SWITCH_ASSERT_LEVEL 0) to ask the power circuit to switch the board off, and
/// releases it to high impedance otherwise. Reported, not acted on.
pub const PIN_POWER_SWITCH: u8 = 57;
/// What the web page's indicator shows while the motor runs.
const MOTOR_COLOUR: [u8; 3] = [255, 160, 0];
/// The BMI270 IMU (`imu_sensor`) on I2C0: the factory firmware's board-manager entry
/// {"imu_sensor", "custom", "bmi270", addr 0x69, "i2c_master"} (SDO high).
pub const BMI270_ADDRESS: u8 = esp_soc::devices::BMI270_ADDRESS;
/// Two BMM150 magnetometers on I2C0, directly on the bus (not behind the BMI270's AUX
/// interface): `magnetometer_1` and `magnetometer_2` list 8-bit addresses 0x22 and 0x24, which
/// the firmware logs as yaml_addr and shifts to the 7-bit i2c_addr.
pub const BMM150_ADDRESSES: [u8; 2] = [0x11, 0x12];
const TOUCH_RELEASE_NS: u64 = 1_000_000;
/// CO5300 TE: one frame every 1/60 s, high for the first 1 ms (the blanking interval).
const TE_PERIOD_NS: u64 = 16_666_667;
const TE_HIGH_NS: u64 = 1_000_000;

/// The ESP-Mosaico v1.2.
pub struct Mosaico {
    /// the CO5300; its frame is what the glass shows (480 x 480, no column offset in the BSP)
    pub panel: Co5300,
    /// the supply enable (GPIO 60, active low) and the reset line (GPIO 44, active low)
    pub powered: bool,
    pub in_reset: bool,
    /// the speaker amplifier's enable (GPIO 45)
    pub amp: bool,
    pub gpio_events: u64,
    /// transactions the panel could not take: unpowered or held in reset
    pub dropped: u64,
    /// the finger the CST9220 reports
    pub touch: std::sync::Arc<std::sync::Mutex<crate::cst9220::Touch>>,
    /// the storage NAND on GP-SPI3 (the BSP's MOSAICO_NAND)
    pub nand: crate::spi_nand::SpiNand,
    /// TE: the level now and the edges not yet delivered (`advance_to` takes emulated nanoseconds)
    te_level: bool,
    te_now: u64,
    /// INT level and, after a release, when it goes back high
    touch_int_low: bool,
    touch_release_at: Option<u64>,
    te_edges: Vec<esp_soc::board::BoardEdge>,
    /// Scripted motion for the BMI270 and the BMM150s (clock: emulated nanoseconds, stored by `advance_to`)
    pub imu: std::sync::Arc<esp_soc::devices::imu_motion::ImuMotion>,
    /// The vibration motor: running, how often it started, how long it ran (emulated ns, up to
    /// the last start when running), and its one-LED indicator for `leds()`.
    pub motor: bool,
    pub motor_starts: u64,
    motor_ns: u64,
    motor_since: u64,
    motor_led: [[u8; 3]; 1],
    motor_updates: u64,
    /// PWR_SW asserted (driven low): the firmware asked for power off.
    pub shutdown_requested: bool,
}
impl Default for Mosaico { fn default() -> Self { Self::new() } }
impl Mosaico {
    pub const WIDTH: usize = 480;
    pub const HEIGHT: usize = 480;
    pub fn new() -> Self {
        Mosaico { panel: Co5300::new(Self::WIDTH, Self::HEIGHT, 0), powered: false, in_reset: false, amp: false, gpio_events: 0, dropped: 0, touch: Default::default(), nand: crate::spi_nand::SpiNand::new(), te_level: false, te_now: 0, touch_int_low: false, touch_release_at: None, te_edges: Vec::new(), imu: Default::default(),
                  motor: false, motor_starts: 0, motor_ns: 0, motor_since: 0, motor_led: [[0; 3]], motor_updates: 0, shutdown_requested: false }
    }
    /// Emulated nanoseconds the motor has run, including the current run.
    pub fn motor_ns(&self) -> u64 { self.motor_ns + if self.motor { self.te_now.saturating_sub(self.motor_since) } else { 0 } }
    fn set_motor(&mut self, on: bool) {
        if on == self.motor { return; }
        if on { self.motor_starts += 1; self.motor_since = self.te_now; } else { self.motor_ns += self.te_now.saturating_sub(self.motor_since); }
        self.motor = on;
        self.motor_led = [if on { MOTOR_COLOUR } else { [0; 3] }];
        self.motor_updates += 1;
    }
}
impl BoardModel for Mosaico {
    fn name(&self) -> &'static str { "mosaico" }
    fn uses_gpio_edges(&self) -> bool { false }
    fn gpio_changes(&mut self, changes: &[(u8, bool)]) {
        for &(pin, level) in changes {
            self.gpio_events += 1;
            match pin {
                PIN_LCD_POWER_N => { if level && self.powered { self.panel.reset(); } self.powered = !level; }
                PIN_LCD_RESET => { if !level { self.panel.reset(); } self.in_reset = !level; }
                PIN_PA => self.amp = level,
                PIN_MOTOR => self.set_motor(level),
                PIN_POWER_SWITCH => self.shutdown_requested = !level,
                _ => {}
            }
        }
    }
    fn gpio_events(&self) -> u64 { self.gpio_events }
    fn pull_downs(&self) -> u64 { 1 << PIN_MOTOR }
    /// The motor's indicator: one "LED", lit while it runs.
    fn leds(&self) -> Option<(&[[u8; 3]], u64)> { Some((&self.motor_led, self.motor_updates)) }
    fn input_levels(&self) -> Vec<(u8, bool)> { vec![(PIN_LCD_TE, self.te_level), (PIN_TOUCH_INT, !self.touch_int_low), (PIN_AI_BUTTON, true), (PIN_BOOT_BUTTON, true)] }
    /// The S31 machine passes emulated time in nanoseconds. TE runs only while the panel is powered
    /// and out of reset; a jump over several frames yields the level at `now` (one edge pair at most).
    fn advance_to(&mut self, now: esp_soc::board::VirtualCycle) {
        self.te_now = now;
        self.imu.cycle.store(now, std::sync::atomic::Ordering::Relaxed);
        if self.touch_release_at.is_some_and(|t| now >= t) {
            self.touch_release_at = None; self.touch_int_low = false;
            self.te_edges.push(esp_soc::board::BoardEdge { cycle: now, pin: PIN_TOUCH_INT, level: true });
        }
        let level = self.powered && !self.in_reset && now % TE_PERIOD_NS < TE_HIGH_NS;
        if level != self.te_level {
            self.te_level = level;
            self.te_edges.push(esp_soc::board::BoardEdge { cycle: now, pin: PIN_LCD_TE, level });
        }
    }
    fn named_pin(&self, name: &str) -> Option<u8> {
        match name { "ai" | "power" | "btn1" | "button" => Some(PIN_AI_BUTTON), "boot" | "btn2" => Some(PIN_BOOT_BUTTON), _ => None }
    }
    fn take_edges(&mut self) -> Vec<esp_soc::board::BoardEdge> { std::mem::take(&mut self.te_edges) }
    /// The next TE edge, in the same emulated nanoseconds `advance_to` takes, so an idle machine
    /// wakes for it instead of jumping over the 1 ms blanking pulse.
    fn next_deadline(&self) -> Option<esp_soc::board::VirtualCycle> {
        if !self.powered || self.in_reset { return None; }
        let now = self.te_now;
        let phase = now % TE_PERIOD_NS;
        let te = now - phase + if phase < TE_HIGH_NS { TE_HIGH_NS } else { TE_PERIOD_NS };
        Some(self.touch_release_at.map_or(te, |t| t.min(te)))
    }
    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn esp_periph::i2c::I2cDevice>)> {
        vec![(0, crate::cst9220::ADDRESS, Box::new(crate::cst9220::Cst9220::new(self.touch.clone(), Self::WIDTH as u16, Self::HEIGHT as u16))),
             (0, crate::bq27220::ADDRESS, Box::new(crate::bq27220::Bq27220::new())),
             (0, ES8311_ADDRESS, Box::new(esp_periph::i2c::Reg8Device::new("es8311", &[(0xfd, 0x83), (0xfe, 0x11)]))),
             (0, BMI270_ADDRESS, Box::new(esp_soc::devices::Bmi270::new(self.imu.clone(), 1_000_000_000))),
             (0, BMM150_ADDRESSES[0], Box::new(esp_soc::devices::Bmm150::with_motion(Default::default(), self.imu.clone(), 1_000_000_000))),
             (0, BMM150_ADDRESSES[1], Box::new(esp_soc::devices::Bmm150::with_motion(Default::default(), self.imu.clone(), 1_000_000_000)))]
    }
    fn set_imu_motion(&mut self, mode: u32) -> bool { self.imu.mode.store(mode, std::sync::atomic::Ordering::Relaxed); true }
    fn imu_motion(&self) -> u32 { self.imu.mode.load(std::sync::atomic::Ordering::Relaxed) }
    fn touch(&mut self, x: u16, y: u16, down: bool) {
        *self.touch.lock().unwrap() = crate::cst9220::Touch { down, x, y };
        // A new report: INT falls (again, for a move while down) and, on release, rises 1 ms later.
        if self.touch_int_low { self.te_edges.push(esp_soc::board::BoardEdge { cycle: self.te_now, pin: PIN_TOUCH_INT, level: true }); }
        self.touch_int_low = true;
        self.te_edges.push(esp_soc::board::BoardEdge { cycle: self.te_now, pin: PIN_TOUCH_INT, level: false });
        self.touch_release_at = (!down).then_some(self.te_now + TOUCH_RELEASE_NS);
    }
    fn spi_transfer(&mut self, host: u8, tx: &[u8], rx_len: usize) -> Vec<u8> {
        if host == 3 { return self.nand.transaction(tx, rx_len); }
        if host == 2 {
            if self.powered && !self.in_reset { self.panel.transaction(tx); } else { self.dropped += 1; }
        }
        vec![0xff; rx_len]
    }
    fn display(&self) -> Option<(u32, u32, Vec<u16>, u64)> {
        Some((Self::WIDTH as u32, Self::HEIGHT as u32, self.panel.frame.clone(), self.panel.pixels_written))
    }
    fn display_version(&self) -> u64 { self.panel.pixels_written }
    fn display_frames(&self) -> u64 { self.panel.frames }
    fn report(&self) -> String {
        let n = self.nand.stats;
        format!("[emu] mosaico: CO5300 {} RAMWR, {} pixels; power {}, reset {}; {} transactions dropped; gpio events {}; speaker amp {}\n[emu] mosaico: NAND {} page reads, {} programs, {} erases, {} refused\n[emu] mosaico: motor {}, {} starts, {} ms on; PWR_SW {}",
                self.panel.frames, self.panel.pixels_written, if self.powered { "on" } else { "off" },
                if self.in_reset { "held" } else { "released" }, self.dropped, self.gpio_events, if self.amp { "on" } else { "off" }, n.reads, n.programs, n.erases, n.refused,
                if self.motor { "on" } else { "off" }, self.motor_starts, self.motor_ns() / 1_000_000, if self.shutdown_requested { "asserted (shutdown requested)" } else { "released" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_takes_commands_only_when_powered_and_out_of_reset() {
        let mut b = Mosaico::new();
        let ramwr = [0x32, 0, 0x2c, 0, 0x12, 0x34];
        b.spi_transfer(2, &ramwr, 0);
        assert_eq!(b.panel.pixels_written, 0);
        assert_eq!(b.dropped, 1);
        b.gpio_changes(&[(PIN_LCD_POWER_N, false), (PIN_LCD_RESET, false)]);
        b.spi_transfer(2, &ramwr, 0);
        assert_eq!(b.panel.pixels_written, 0, "held in reset");
        b.gpio_changes(&[(PIN_LCD_RESET, true)]);
        b.spi_transfer(2, &ramwr, 0);
        assert_eq!(b.panel.frame[0], 0x1234);
    }

    #[test]
    fn motor_runs_while_gpio8_is_high_and_lights_its_indicator() {
        let mut b = Mosaico::new();
        assert_eq!(b.pull_downs(), 1 << PIN_MOTOR, "an undriven motor pin reads off");
        assert_eq!(b.leds(), Some((&[[0, 0, 0]][..], 0)));
        b.advance_to(1_000_000);
        b.gpio_changes(&[(PIN_MOTOR, true)]);
        assert!(b.motor);
        assert_eq!(b.leds(), Some((&[MOTOR_COLOUR][..], 1)));
        b.gpio_changes(&[(PIN_MOTOR, true)]);
        assert_eq!(b.leds().unwrap().1, 1, "no change, no update");
        b.advance_to(201_000_000);
        assert_eq!(b.motor_ns(), 200_000_000, "running: counted up to now");
        b.gpio_changes(&[(PIN_MOTOR, false)]);
        b.advance_to(500_000_000);
        assert_eq!((b.motor, b.motor_starts, b.motor_ns()), (false, 1, 200_000_000));
        assert!(b.report().contains("motor off, 1 starts, 200 ms on; PWR_SW released"), "{}", b.report());
        b.gpio_changes(&[(PIN_POWER_SWITCH, false)]);
        assert!(b.report().contains("PWR_SW asserted"), "{}", b.report());
    }

    #[test]
    fn keys_by_name() {
        let b = Mosaico::new();
        assert_eq!(b.named_pin("ai"), Some(7));
        assert_eq!(b.named_pin("boot"), Some(61));
        assert!(b.input_levels().contains(&(PIN_AI_BUTTON, true)) && b.input_levels().contains(&(PIN_BOOT_BUTTON, true)), "released: high");
    }
}
