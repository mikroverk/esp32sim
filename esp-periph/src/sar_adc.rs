//! C3/C6 APB SAR one-shot controller and default-eFuse voltage transfer curves.
use crate::{AnalogInputs, Device, RegRam, WriteEffect};

#[derive(Clone, Copy)]
pub enum Calibration {
    S3Adc2,
    C3,
    C6,
}

// ESP-IDF v5.5 components/esp_adc/{esp32s3,esp32c3,esp32c6}/curve_fitting_coefficients.c.
// Coefficients copyright 2019-2024 Espressif Systems (Shanghai) CO LTD.
// Converted to Rust triples; Apache-2.0, see ../LICENSE-ADC.
const S3_ADC2: [[(u64, u64, i32); 5]; 4] = [
    [
        (25668651654328927, 10000000000000000, -1),
        (1353548869615, 10000000000000000, 1),
        (36615265189, 10000000000000000, 1),
        (0, 0, 0),
        (0, 0, 0),
    ],
    [
        (23690184690298404, 10000000000000000, -1),
        (66319894226185, 10000000000000000, -1),
        (118964995959, 10000000000000000, 1),
        (0, 0, 0),
        (0, 0, 0),
    ],
    [
        (9452499397020617, 10000000000000000, -1),
        (200996773954387, 10000000000000000, -1),
        (259011467956, 10000000000000000, 1),
        (0, 0, 0),
        (0, 0, 0),
    ],
    [
        (12247719764336924, 10000000000000000, 1),
        (755717904943462, 10000000000000000, -1),
        (1478791187119, 10000000000000000, 1),
        (79672528, 1000000000000000, -1),
        (15038, 1000000000000000, 1),
    ],
];
const C3: [[(u64, u64, i32); 5]; 4] = [
    [
        (225966470500043, 1000000000000000, -1),
        (7265418501948, 10000000000000000, -1),
        (109410402681, 10000000000000000, 1),
        (0, 0, 0),
        (0, 0, 0),
    ],
    [
        (4229623392600516, 10000000000000000, 1),
        (731527490903, 10000000000000000, -1),
        (88166562521, 10000000000000000, 1),
        (0, 0, 0),
        (0, 0, 0),
    ],
    [
        (1017859239236435, 1000000000000000, -1),
        (97159265299153, 10000000000000000, -1),
        (149794028038, 10000000000000000, 1),
        (0, 0, 0),
        (0, 0, 0),
    ],
    [
        (14912262772850453, 10000000000000000, -1),
        (228549975564099, 10000000000000000, -1),
        (356391935717, 10000000000000000, 1),
        (179964582, 10000000000000000, -1),
        (42046, 10000000000000000, 1),
    ],
];
const C6: [[(u64, u64, i32); 5]; 4] = [
    [(0, 0, 0), (0, 0, 0), (0, 0, 0), (0, 0, 0), (0, 0, 0)],
    [(0, 0, 0), (0, 0, 0), (0, 0, 0), (0, 0, 0), (0, 0, 0)],
    [
        (12217864764388775, 10000000000000000, -1),
        (1954123107752, 10000000000000000, -1),
        (6409679727, 10000000000000000, 1),
        (0, 0, 0),
        (0, 0, 0),
    ],
    [
        (3915910437042445, 10000000000000000, -1),
        (31536470857564, 10000000000000000, -1),
        (12493873014, 10000000000000000, 1),
        (0, 0, 0),
        (0, 0, 0),
    ],
];

/// IDF 5.5 calibrated millivolts with the emulator's default zero calibration differences.
/// C6 block version 0.3 selects calibration V2; C3 and S3 select V1.
pub fn millivolts(raw: u32, atten: u32, calibration: Calibration) -> i32 {
    let a = (atten & 3) as usize;
    let (digi, mv, coefficients) = match calibration {
        Calibration::S3Adc2 => ([3240, 2410, 1720, 915][a], 850, &S3_ADC2[a]),
        Calibration::C3 => (2000, [400, 550, 750, 1370][a], &C3[a]),
        Calibration::C6 => (
            if a == 2 { 2900 } else { 2850 },
            [750, 1000, 1500, 2800][a],
            &C6[a],
        ),
    };
    let voltage = u64::from(raw) * (65536 * mv / digi) / 65536;
    if voltage == 0 {
        return 0;
    }
    let mut power = 1u64;
    let mut error = 0i32;
    for &(coefficient, divisor, sign) in coefficients {
        if divisor == 0 {
            break;
        }
        error = error.wrapping_add((power.wrapping_mul(coefficient) / divisor) as i32 * sign);
        power = power.wrapping_mul(voltage);
    }
    (voltage as i32).wrapping_sub(error)
}

pub fn voltage_code(volts: f32, atten: u32, calibration: Calibration) -> u32 {
    let want = (volts.max(0.0) * 1000.0) as i64;
    // ponytail: exhaustive 12-bit inverse; cache a transfer table if sampling throughput matters.
    (0..4096)
        .min_by_key(|&raw| (i64::from(millivolts(raw, atten, calibration)) - want).abs())
        .unwrap()
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
        Self {
            analog: AnalogInputs::new(cpu_hz),
            now_cycles: 0,
            c6,
            ram: RegRam::new(),
            raw: 0,
        }
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
                        let calibration = if self.c6 {
                            Calibration::C6
                        } else {
                            Calibration::C3
                        };
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
