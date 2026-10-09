//! S3/C3/C6 default-eFuse voltage transfer curves and C3/C6 APB SAR one-shot controller.
use crate::{AnalogInputs, Device, RegRam, WriteEffect};

/// SENS START rising edge and hardware-owned DONE/DATA. The caller supplies the
/// DATA value retained while START is low and the complete conversion result
/// (DONE and DATA, within bits 16:0).
#[inline(always)]
pub fn sens_oneshot(prev: u32, value: u32, convert: impl FnOnce() -> u32) -> u32 {
    let mut out = value & !0x1_ffff;                                // DATA and DONE are hardware-written
    if value & (1 << 17) == 0 { return out | (value & 0xffff); }    // START low: DONE drops
    if prev & (1 << 17) == 0 {                                      // START rising: convert now
        out |= convert();
    } else {
        out |= prev & 0x1_ffff;                                     // START held high: keep the last result
    }
    out
}

#[derive(Clone, Copy)]
pub enum Calibration {
    S3Adc1,
    S3Adc2,
    C3,
    C6,
}

// ESP-IDF v5.5 components/esp_adc/{esp32s3,esp32c3,esp32c6}/curve_fitting_coefficients.c.
// Coefficients copyright 2019-2024 Espressif Systems (Shanghai) CO LTD.
// Converted to Rust triples; Apache-2.0, see ../LICENSE-ADC.
// S3 ADC1 retains IDF v4.4.7 esp_adc_cal arithmetic from PR #165.
const S3_ADC1: [[(u64, u64, i32); 5]; 4] = [
    [(27856531419538344, 10000000000000000, -1), (50871540569528, 10000000000000000, -1), (9798249589, 1000000000000000, 1), (0, 0, 0), (0, 0, 0)],
    [(29831022915028695, 10000000000000000, -1), (49393185868806, 10000000000000000, -1), (101379430548, 10000000000000000, 1), (0, 0, 0), (0, 0, 0)],
    [(23285545746296417, 10000000000000000, -1), (147640181047414, 10000000000000000, -1), (208385525314, 10000000000000000, 1), (0, 0, 0), (0, 0, 0)],
    [(644403418269478, 1000000000000000, -1), (644334888647536, 10000000000000000, -1), (1297891447611, 10000000000000000, 1), (70769718, 1000000000000000, -1), (13515, 1000000000000000, 1)],
];
const S3_ADC2: [[(u64, u64, i32); 5]; 4] = [
    [(25668651654328927, 10000000000000000, -1), (1353548869615, 10000000000000000, 1), (36615265189, 10000000000000000, 1), (0, 0, 0), (0, 0, 0)],
    [(23690184690298404, 10000000000000000, -1), (66319894226185, 10000000000000000, -1), (118964995959, 10000000000000000, 1), (0, 0, 0), (0, 0, 0)],
    [(9452499397020617, 10000000000000000, -1), (200996773954387, 10000000000000000, -1), (259011467956, 10000000000000000, 1), (0, 0, 0), (0, 0, 0)],
    [(12247719764336924, 10000000000000000, 1), (755717904943462, 10000000000000000, -1), (1478791187119, 10000000000000000, 1), (79672528, 1000000000000000, -1), (15038, 1000000000000000, 1)],
];
const C3: [[(u64, u64, i32); 5]; 4] = [
    [(225966470500043, 1000000000000000, -1), (7265418501948, 10000000000000000, -1), (109410402681, 10000000000000000, 1), (0, 0, 0), (0, 0, 0)],
    [(4229623392600516, 10000000000000000, 1), (731527490903, 10000000000000000, -1), (88166562521, 10000000000000000, 1), (0, 0, 0), (0, 0, 0)],
    [(1017859239236435, 1000000000000000, -1), (97159265299153, 10000000000000000, -1), (149794028038, 10000000000000000, 1), (0, 0, 0), (0, 0, 0)],
    [(14912262772850453, 10000000000000000, -1), (228549975564099, 10000000000000000, -1), (356391935717, 10000000000000000, 1), (179964582, 10000000000000000, -1), (42046, 10000000000000000, 1)],
];
const C6: [[(u64, u64, i32); 5]; 4] = [
    [(0, 0, 0), (0, 0, 0), (0, 0, 0), (0, 0, 0), (0, 0, 0)],
    [(0, 0, 0), (0, 0, 0), (0, 0, 0), (0, 0, 0), (0, 0, 0)],
    [(12217864764388775, 10000000000000000, -1), (1954123107752, 10000000000000000, -1), (6409679727, 10000000000000000, 1), (0, 0, 0), (0, 0, 0)],
    [(3915910437042445, 10000000000000000, -1), (31536470857564, 10000000000000000, -1), (12493873014, 10000000000000000, 1), (0, 0, 0), (0, 0, 0)],
];

/// Calibrated millivolts with default zero eFuse calibration differences.
/// S3 ADC1 uses IDF 4.4's uint32 product and 1,000,000 scale; the others use IDF 5.5's
/// 65,536 scale. C6 block version 0.3 selects V2; C3 and S3 select V1.
pub fn millivolts(raw: u32, atten: u32, calibration: Calibration) -> u32 {
    let a = (atten & 3) as usize;
    let (digi, mv, coefficients) = match calibration {
        Calibration::S3Adc1 => ([3200, 2400, 1700, 900][a], 850, &S3_ADC1[a]),
        Calibration::S3Adc2 => ([3240, 2410, 1720, 915][a], 850, &S3_ADC2[a]),
        Calibration::C3 => (2000, [400, 550, 750, 1370][a], &C3[a]),
        Calibration::C6 => (if a == 2 { 2900 } else { 2850 }, [750, 1000, 1500, 2800][a], &C6[a]),
    };
    let voltage = match calibration {
        Calibration::S3Adc1 => u64::from(raw.wrapping_mul(1_000_000 * mv / digi)) / 1_000_000,
        _ => u64::from(raw) * u64::from(65536 * mv / digi) / 65536,
    };
    if voltage == 0 { return 0; }
    let mut power = 1u64; let mut error = 0i32;
    for &(coefficient, divisor, sign) in coefficients {
        if divisor == 0 { break; }
        error = error.wrapping_add(((power.wrapping_mul(coefficient) / divisor) as i32).wrapping_mul(sign));
        power = power.wrapping_mul(voltage);
    }
    (voltage as i32).wrapping_sub(error) as u32
}

pub fn voltage_code(volts: f32, atten: u32, calibration: Calibration) -> u32 {
    let want = (volts.max(0.0) * 1000.0) as i64;
    (0..4096).min_by_key(|&raw| (i64::from(millivolts(raw, atten, calibration) as i32) - want).abs()).unwrap()
}

pub struct SarAdc {
    pub analog: AnalogInputs,
    pub now_cycles: u64,
    c6: bool,
    ram: RegRam,
    raw: u32,
}
impl SarAdc {
    pub fn new(c6: bool, cpu_hz: u64) -> Self {
        Self { analog: AnalogInputs::new(cpu_hz), now_cycles: 0, c6, ram: RegRam::new(), raw: 0 }
    }
}
impl Device for SarAdc {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x44 => self.raw,
            0x48 => self.raw & self.ram.read(0x40),
            0x4c => 0,
            _ => self.ram.read(off),
        }
    }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        match off {
            0x20 => {
                let old = self.ram.read(off);
                self.ram.write(off, value);
                if value & !old & (1 << 29) != 0 {
                    let channel = (value >> 25) & 15;
                    let unit = channel >> 3;
                    let pin = match (self.c6, channel) {
                        (true, 0..=6) | (false, 0..=4) => Some(channel as u8),
                        (false, 8) => Some(5),
                        _ => None,
                    };
                    if let Some(pin) = pin.filter(|_| value & (1 << (31 - unit)) != 0) {
                        let calibration = if self.c6 { Calibration::C6 } else { Calibration::C3 };
                        let raw = self.analog.convert(pin, self.now_cycles, |v| {
                            voltage_code(v, (value >> 23) & 3, calibration)
                        });
                        self.ram.write(0x2c + 4 * unit, raw);
                        self.raw |= 1 << (31 - unit);
                    }
                }
            }
            0x4c => self.raw &= !value,
            0x2c | 0x30 | 0x44 | 0x48 => {}
            _ => self.ram.write(off, value),
        }
        WriteEffect::NONE
    }
}

#[cfg(test)]
mod calibration_tests {
    use super::*;
    #[test]
    fn sens_edges_preserve_only_the_selected_hardware_result() {
        let start = 1 << 17;
        let done = 1 << 16;
        assert_eq!(sens_oneshot(start | done | 42, 7, || panic!("low START")), 7);
        assert_eq!(sens_oneshot(start | done | 42, 0, || panic!("S3 low START")), 0);
        assert_eq!(sens_oneshot(42, start | 9, || done | 123), start | done | 123);
        assert_eq!(sens_oneshot(start | done | 42, start | 9, || panic!("held START")), start | done | 42);
    }

    #[test]
    fn idf44_forward_matches_hand_computed_points() {
        // atten3, digi 900: coeff_a = 944444; raw 1000 → v = 944 mV, error = -0 - 60 + 115 - 59 + 10 = 6 → 938
        assert_eq!(millivolts(1000, 3, Calibration::S3Adc1), 938);
        assert_eq!(millivolts(0, 3, Calibration::S3Adc1), 0);
        // All four attenuations against IDF v4.4.7's own C (esp_adc_cal/esp32s3/esp_adc_cal.c +
        // esp_adc_cal_common.c, compiled unchanged): (atten, raw) -> mV at raw 1000 and 3000.
        // The full 4 x 4096 table matched exactly when this was written.
        for (atten, raw, mv) in [(0, 1000, 268), (0, 3000, 796), (1, 1000, 356), (1, 3000, 1058),
                                 (2, 1000, 504), (2, 3000, 1478), (3, 1000, 938), (3, 3000, 2713)] {
            assert_eq!(millivolts(raw, atten, Calibration::S3Adc1), mv, "atten {atten} raw {raw}");
        }
    }
    #[test]
    fn injected_volts_round_trip_through_the_firmware_formula() {
        for mv in [200u32, 500, 1000, 1650, 2500, 3000] {
            let code = voltage_code(mv as f32 / 1000.0, 3, Calibration::S3Adc1);
            let back = millivolts(code, 3, Calibration::S3Adc1) as i64;
            assert!((back - mv as i64).abs() <= 1, "{mv} mV → code {code} → {back} mV");
        }
    }
    #[test]
    fn s3_adc1_preserves_all_12_bit_idf44_values() {
        // FNV-1a of little-endian u32 results from unchanged PR #165, raw 0..4096.
        for (atten, expected) in [0x8d1925107b2b6226, 0x4a688a8ccb028764, 0xdbd6af723f3116ad, 0x169fa60ebe565fba].into_iter().enumerate() {
            let mut hash = 0xcbf29ce484222325u64;
            for raw in 0..4096 {
                for byte in millivolts(raw, atten as u32, Calibration::S3Adc1).to_le_bytes() {
                    hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                }
            }
            assert_eq!(hash, expected, "atten {atten}");
        }
    }
}
