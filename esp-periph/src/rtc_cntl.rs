use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;
use emu_core::ClockDomain;

// ------------------------------------------------------------------ RTC controller
/// Reset causes (RTC_CNTL_RESET_CAUSE_PROCPU), as the ROM prints them.
pub const RST_POWERON: u32 = 1; pub const RST_SW_SYS: u32 = 3; pub const RST_RTCWDT_SYS: u32 = 9; pub const RST_SW_CPU: u32 = 12;
pub const RST_RTCWDT_CPU: u32 = 13; pub const RST_RTCWDT_RTC: u32 = 16;
pub fn reset_cause_name(c: u32) -> &'static str {
    match c { 1 => "POWERON", 3 => "RTC_SW_SYS_RESET", 5 => "DEEPSLEEP", 7 => "TG0WDT_SYS_RESET", 8 => "TG1WDT_SYS_RESET", 9 => "RTCWDT_SYS_RESET", 11 => "TG0WDT_CPU_RESET",
            12 => "RTC_SW_CPU_RESET", 13 => "RTCWDT_CPU_RESET", 15 => "RTCWDT_BROWN_OUT_RESET", 16 => "RTCWDT_RTC_RESET", 17 => "TG1WDT_CPU_RESET", 18 => "SUPER_WDT_RESET", _ => "?" }
}

/// RTC_CNTL: reset control, slow-clock time and the RTC watchdog.
/// WDTCONFIG0..WDTWPROTECT live at 0x98..0xb0 on S3 and 0x90..0xa8 on C3.
/// `esp_restart()` on ESP-IDF 5.x arms this watchdog and spins until it resets the chip.
pub struct RtcCntl { pub ram: RegRam, pub slow_ticks: u64, pub time_latch: u64, pub sw_reset: bool, pub reset_cause: u32,
                     wdt_base: u32, wdt_count: u64, wdt_stage: usize, wdt_unlocked: bool,
                     /// S3 only: the SENS block at +0x800 converts on SAR_MEAS1_START (see `sens_meas`); the chip refreshes
                     /// `now_cycles` before each access so a waveform source is sampled at the right emulated time.
                     pub sens_adc: bool, pub analog: crate::analog::AnalogInputs, pub now_cycles: u64 }
impl RtcCntl {
    pub fn preset_after_bootloader(&mut self) { self.ram.write(0xc0, 0xFFD7_0028); self.ram.write(0xc4, 0xFF0F_00F0); }
    fn request_reset(&mut self, cause: u32) { if !self.sw_reset { self.sw_reset = true; self.reset_cause = cause; } }
    /// Advance the watchdog by RTC slow-clock ticks.
    pub fn wdt_tick(&mut self, ticks: u64) {
        let conf0 = self.ram.read(self.wdt_base);
        if conf0 & (1 << 31) == 0 { return; }
        self.wdt_count += ticks;
        while self.wdt_stage < 4 {
            let timeout = self.ram.read(self.wdt_base + 4 + 4 * self.wdt_stage as u32) as u64;
            let action = (conf0 >> (28 - 3 * self.wdt_stage as u32)) & 7;
            if action == 0 { self.wdt_stage += 1; continue; }              // stage disabled: skip
            if self.wdt_count < timeout { break; }
            self.wdt_count = 0; self.wdt_stage += 1;
            match action {
                1 => { self.ram.write(0x44, self.ram.read(0x44) | (1 << 3)); }   // INT_RAW.WDT
                2 => self.request_reset(RST_RTCWDT_CPU),
                3 => self.request_reset(RST_RTCWDT_SYS),
                4 => self.request_reset(RST_RTCWDT_RTC),
                _ => {}
            }
            if self.sw_reset { break; }
        }
        if self.wdt_stage >= 4 { self.wdt_stage = 0; }
    }
    pub fn new() -> Self { let mut r = Self::with_wdt_base(0x98); r.sens_adc = true; r }
    pub fn new_c3() -> Self { Self::with_wdt_base(0x90) }
    fn with_wdt_base(wdt_base: u32) -> Self {
        let mut r = RtcCntl { ram: RegRam::new(), slow_ticks: 0, time_latch: 0, sw_reset: false, reset_cause: RST_POWERON, wdt_base, wdt_count: 0, wdt_stage: 0, wdt_unlocked: false,
                              sens_adc: false, analog: crate::analog::AnalogInputs::new(240_000_000), now_cycles: 0 };
        r.ram.write(0x38, 1 | (1 << 6));           // RESET_STATE: reset cause POWERON for both CPUs
        r.ram.write(0x74, 0);                        // CLK_CONF
        r
    }
    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            0x10 => self.time_latch as u32, 0x14 => (self.time_latch >> 32) as u32,
            0xc => self.ram.read(off) | (1 << 30),  // TIME_UPDATE: valid
            0x1fc => 0x2007270,
            0x850 => (self.ram.read(off) & !0x1ff) | (1 << 8) | 0x80,   // SENS_SAR_TSENS_CTRL (SENS block at +0x800): TSENS_READY, raw ~ room temperature
            _ => self.ram.read(off),
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        let wdt = self.wdt_base;
        match off {
            0x0 => { if v & (1 << 31) != 0 { self.request_reset(RST_SW_SYS); } else if v & (1 << 5) != 0 { self.request_reset(RST_SW_CPU); } self.ram.write(off, v & !((1 << 31) | (1 << 5))); }   // OPTIONS0.SW_SYS_RST / SW_PROCPU_RST
            0xc => { if v & (1 << 31) != 0 { self.time_latch = self.slow_ticks; } self.ram.write(off, v); }
            0x80c | 0x830 if self.sens_adc => self.sens_meas(off, v),
            _ if off == wdt + 0x18 => { self.wdt_unlocked = v == 0x50D8_3AA1; self.ram.write(off, v); }
            _ if (wdt..=wdt + 0x10).contains(&off) => { if self.wdt_unlocked { if off == wdt && (v ^ self.ram.read(wdt)) & (1 << 31) != 0 { self.wdt_count = 0; self.wdt_stage = 0; } self.ram.write(off, v); } }
            _ if off == wdt + 0x14 => { if self.wdt_unlocked && v & (1 << 31) != 0 { self.wdt_count = 0; self.wdt_stage = 0; } }   // WDTFEED
            _ => self.ram.write(off, v),
        }
    }
}

/// ESP-IDF v4.4.7 `esp_adc_cal` for the ESP32-S3 (components/esp_adc_cal/esp32s3/esp_adc_cal.c and
/// esp_adc_cal_common.c, Apache-2.0): two-point reference from eFuse, then Espressif's curve-fitting
/// error polynomial per attenuation. Reproduced with the same integer arithmetic (including its
/// wrap-around) so the model's inverse lands on the exact millivolt the firmware computes.
pub mod idf44_s3_adc1 {
    const COEF: [[(u64, u64); 5]; 4] = [
        [(27856531419538344, 10_000_000_000_000_000), (50871540569528, 10_000_000_000_000_000), (9798249589, 1_000_000_000_000_000), (0, 0), (0, 0)],
        [(29831022915028695, 10_000_000_000_000_000), (49393185868806, 10_000_000_000_000_000), (101379430548, 10_000_000_000_000_000), (0, 0), (0, 0)],
        [(23285545746296417, 10_000_000_000_000_000), (147640181047414, 10_000_000_000_000_000), (208385525314, 10_000_000_000_000_000), (0, 0), (0, 0)],
        [(644403418269478, 1_000_000_000_000_000), (644334888647536, 10_000_000_000_000_000), (1297891447611, 10_000_000_000_000_000), (70769718, 1_000_000_000_000_000), (13515, 1_000_000_000_000_000)],
    ];
    const SIGN: [[i32; 5]; 4] = [[-1, -1, 1, 0, 0], [-1, -1, 1, 0, 0], [-1, -1, 1, 0, 0], [-1, -1, 1, -1, 1]];
    /// ADC1 reference code at 850 mV per attenuation with the calibration diffs the emulator's eFuse
    /// holds (all zero, block version 1): esp_efuse_rtc_calib_get_cal_voltage → 3200 / 2400 / 1700 / 900.
    pub const DIGI: [u32; 4] = [3200, 2400, 1700, 900];
    /// esp_adc_cal_raw_to_voltage(raw) for ADC1 at `atten`.
    pub fn millivolts(raw: u32, atten: usize) -> u32 {
        let coeff_a: u32 = 1_000_000 * 850 / DIGI[atten];
        let v = (raw.wrapping_mul(coeff_a) as u64) / 1_000_000;              // uint32 product, as in C
        if v == 0 { return 0; }
        let terms = if atten == 3 { 5 } else { 3 };
        let mut error = (COEF[atten][0].0 / COEF[atten][0].1) as i32 * SIGN[atten][0];
        let mut var: u64 = 1;
        for i in 1..terms {
            var = var.wrapping_mul(v);
            let term = var.wrapping_mul(COEF[atten][i].0) / COEF[atten][i].1;
            error = error.wrapping_add((term as i32).wrapping_mul(SIGN[atten][i]));
        }
        (v as i32).wrapping_sub(error) as u32
    }
}

impl RtcCntl {
    /// The 12-bit ADC1 code a typical S3 gives for `volts` at `atten`: the inverse of the IDF 4.4
    /// calibration above (the code whose calibrated reading is nearest), saturating at 4095, so
    /// analogReadMilliVolts() on the firmware reports the injected voltage to within a code.
    /// Noise, INL beyond Espressif's fitted curve and chip-to-chip spread are not modelled.
    pub fn s3_adc_code(volts: f32, atten: u32) -> u32 {
        let want = (volts.max(0.0) * 1000.0) as i64;
        let a = (atten & 3) as usize;
        let mut best = (0u32, i64::MAX);
        for raw in 0..4096u32 {
            let d = (idf44_s3_adc1::millivolts(raw, a) as i32 as i64 - want).abs();
            if d < best.1 { best = (raw, d); }
        }
        best.0
    }
    /// SENS_SAR_MEAS1_CTRL2 (IDF adc_ll_rtc_start_convert / _convert_is_done / _get_convert_value):
    /// EN_PAD [30:19] picks the ADC1 channel, START [17] rising runs one conversion, DONE [16]
    /// and DATA [15:0] carry the result. ADC1 channel n is GPIO n+1 on the S3; the channel's
    /// attenuation is SENS_SAR_ATTEN1 (+0x814) bits [2n+1:2n]. ADC2 uses CTRL2 +0x830,
    /// ATTEN2 +0x838 and GPIO n+11. Both convert instantaneously.
    fn sens_meas(&mut self, off: u32, v: u32) {
        let prev = self.ram.read(off);
        let mut out = v & !0x1_ffff;                         // DATA and DONE are hardware-written
        if v & (1 << 17) == 0 { self.ram.write(off, out); return; }   // START low: DONE drops
        if prev & (1 << 17) == 0 {                           // START rising: convert now
            let pads = (v >> 19) & 0xfff;
            let code = if pads == 0 { 0 } else {
                let ch = pads.trailing_zeros();
                let adc2 = off == 0x830;
                let atten = (self.ram.read(if adc2 { 0x838 } else { 0x814 }) >> (2 * ch)) & 3;
                if ch >= 10 { 0 } else {
                    self.analog.convert(ch as u8 + if adc2 { 11 } else { 1 }, self.now_cycles, |v| {
                        if adc2 { crate::sar_adc::voltage_code(v, atten, crate::sar_adc::Calibration::S3Adc2) }
                        else { Self::s3_adc_code(v, atten) }
                    })
                }
            };
            out |= (1 << 16) | code;
        } else {
            out |= prev & 0x1_ffff;                          // START held high: keep the last result
        }
        self.ram.write(off, out);
    }
}

impl Default for RtcCntl { fn default() -> Self { Self::new() } }

impl Device for RtcCntl {
    fn read(&mut self, off: u32) -> u32 { RtcCntl::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { RtcCntl::write(self, off, v); WriteEffect::NONE }
    fn clock(&self) -> Option<ClockDomain> { Some(ClockDomain::RtcSlow) }
    fn tick(&mut self, ticks: u64) { self.slow_ticks += ticks; self.wdt_tick(ticks); }
}

#[cfg(test)]
mod sens_adc_tests {
    use super::*;
    #[test]
    fn idf44_forward_matches_hand_computed_points() {
        // atten3, digi 900: coeff_a = 944444; raw 1000 → v = 944 mV, error = -0 - 60 + 115 - 59 + 10 = 6 → 938
        assert_eq!(idf44_s3_adc1::millivolts(1000, 3), 938);
        assert_eq!(idf44_s3_adc1::millivolts(0, 3), 0);
        // All four attenuations against IDF v4.4.7's own C (esp_adc_cal/esp32s3/esp_adc_cal.c +
        // esp_adc_cal_common.c, compiled unchanged): (atten, raw) -> mV at raw 1000 and 3000.
        // The full 4 x 4096 table matched exactly when this was written.
        for (atten, raw, mv) in [(0, 1000, 268), (0, 3000, 796), (1, 1000, 356), (1, 3000, 1058),
                                 (2, 1000, 504), (2, 3000, 1478), (3, 1000, 938), (3, 3000, 2713)] {
            assert_eq!(idf44_s3_adc1::millivolts(raw, atten), mv, "atten {atten} raw {raw}");
        }
    }
    #[test]
    fn injected_volts_round_trip_through_the_firmware_formula() {
        for mv in [200u32, 500, 1000, 1650, 2500, 3000] {
            let code = RtcCntl::s3_adc_code(mv as f32 / 1000.0, 3);
            let back = idf44_s3_adc1::millivolts(code, 3) as i64;
            assert!((back - mv as i64).abs() <= 1, "{mv} mV → code {code} → {back} mV");
        }
    }
}
