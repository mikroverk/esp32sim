//! Classic ESP32 RTC-controlled SAR ADCs, DAC outputs and touch pads.
use esp_periph::{AnalogInputs, RegRam};

const ADC1_PINS: [u8; 8] = [36, 37, 38, 39, 32, 33, 34, 35];
const ADC2_PINS: [u8; 10] = [4, 0, 2, 15, 13, 12, 14, 27, 25, 26];
const TOUCH_PINS: [u8; 10] = [4, 0, 2, 15, 13, 12, 14, 27, 33, 32];
// GPIO, RTC_IO register, RTC mux bit (ESP-IDF rtc_io_desc).
const PADS: [(u8, u32, u32); 18] = [
    (36, 0x7c, 27),
    (37, 0x7c, 26),
    (38, 0x7c, 25),
    (39, 0x7c, 24),
    (34, 0x80, 29),
    (35, 0x80, 28),
    (25, 0x84, 17),
    (26, 0x88, 17),
    (33, 0x8c, 18),
    (32, 0x8c, 17),
    (4, 0x94, 19),
    (0, 0x98, 19),
    (2, 0x9c, 19),
    (15, 0xa0, 19),
    (13, 0xa4, 19),
    (12, 0xa8, 19),
    (14, 0xac, 19),
    (27, 0xb0, 19),
];
const START: u32 = 1 << 17;
const DONE: u32 = 1 << 16;
const START_FORCE: u32 = 1 << 18;
const PAD_FORCE: u32 = 1 << 31;

pub struct ClassicAdc {
    sens: RegRam,
    rtc_io: RegRam,
    pub analog: AnalogInputs,
    pub now_cycles: u64,
    touched: [bool; 10],
    touch_timer: bool,
}

impl Default for ClassicAdc {
    fn default() -> Self {
        Self::new()
    }
}

impl ClassicAdc {
    pub fn new() -> Self {
        let mut sens = RegRam::new();
        sens.write(0x2c, 0xf);
        sens.write(0x58, (1 << 25) | (4 << 16) | 0x1000);
        sens.write(0x84, (0x100 << 14) | (1 << 11));
        sens.write(0x8c, 0x3fff_ffff);
        let mut rtc_io = RegRam::new();
        for channel in 0..10 {
            rtc_io.write(0x94 + channel * 4, 4 << 23);
        }
        Self {
            sens,
            rtc_io,
            analog: AnalogInputs::new(crate::periph::CPU_HZ),
            now_cycles: 0,
            touched: [false; 10],
            touch_timer: false,
        }
    }

    pub fn set_touch_input(&mut self, pin: u8, touched: bool) -> bool {
        let Some(channel) = TOUCH_PINS.iter().position(|&p| p == pin) else {
            return false;
        };
        self.touched[channel] = touched;
        true
    }

    pub fn restore_inputs(&mut self, old: Self) {
        self.analog = old.analog;
        self.touched = old.touched;
    }

    pub fn set_touch_timer_enabled(&mut self, enabled: bool) {
        self.touch_timer = enabled;
    }

    pub fn rtc_pad_mask(&self) -> u64 {
        PADS.iter().fold(0, |mask, &(pin, off, mux)| {
            mask | if self.rtc_io.read(off) & (1 << mux) != 0 {
                1 << pin
            } else {
                0
            }
        })
    }

    pub fn dac_outputs(&self) -> Vec<(u8, u32)> {
        (0..2)
            .filter_map(|channel| {
                let pin = 25 + channel as u8;
                let value = self.rtc_io.read(0x84 + channel * 4);
                let powered = value & ((1 << 18) | (1 << 10)) == (1 << 18) | (1 << 10);
                let cw = self.sens.read(0x9c) & (1 << (24 + channel)) != 0;
                (powered && !cw).then_some((pin, (((value >> 19) & 255) * 3300 + 127) / 255))
            })
            .collect()
    }

    pub fn read_rtc_io(&self, off: u32) -> u32 {
        self.rtc_io.read(off)
    }

    pub fn write_rtc_io(&mut self, off: u32, value: u32) {
        self.rtc_io.write(off, value);
    }

    pub fn read_sens(&mut self, off: u32) -> u32 {
        if matches!(off, 0x70..=0x84) && self.touch_timer && self.sens.read(0x84) & (1 << 13) == 0 {
            self.measure_touch();
        }
        match off {
            // Preserve the existing boot temperature-ready stub.
            0x50 => (self.sens.read(off) & !0x1ff) | 0x180,
            _ => self.sens.read(off),
        }
    }

    pub fn write_sens(&mut self, off: u32, value: u32) {
        match off {
            0x54 | 0x94 => {
                let old = self.sens.read(off);
                self.sens.write(off, (value & !0x1ffff) | (old & 0xffff));
                if value & START != 0 && old & START == 0 {
                    self.convert(usize::from(off == 0x94), off, value);
                } else if value & START != 0 {
                    self.sens.write(off, (value & !0x1ffff) | (old & 0x1ffff));
                }
            }
            0x70..=0x80 => {}
            0x84 => {
                let old = self.sens.read(off);
                self.sens
                    .write(off, (value & !((1 << 30) | 0x7ff)) | (old & (1 << 10)));
                if value & (1 << 13) != 0 && value & (1 << 12) != 0 && old & (1 << 12) == 0 {
                    self.measure_touch();
                }
            }
            _ => self.sens.write(off, value),
        }
    }

    fn convert(&mut self, unit: usize, off: u32, value: u32) {
        let control = self.sens.read(if unit == 0 { 0 } else { 0x90 });
        let digital = if unit == 0 {
            1 << 27
        } else {
            (1 << 27) | (1 << 28)
        };
        if value & (START_FORCE | PAD_FORCE) != START_FORCE | PAD_FORCE || control & digital != 0 {
            return;
        }
        let pads = (value >> 19) & 0xfff;
        let pins: &[u8] = if unit == 0 { &ADC1_PINS } else { &ADC2_PINS };
        let channel = pads.trailing_zeros() as usize;
        let powered = (self.sens.read(0x0c) >> 18) & 3 != 2;
        let mut raw = 0;
        // Analog channels connect directly to pads. IDF5.5 gpio_config_as_analog clears RTC mux.
        // ponytail: host voltage is absolute; model electrical contention only when a workload needs it.
        if powered && pads.count_ones() == 1 && channel < pins.len() {
            let attenuation =
                ((self.sens.read(0x34 + unit as u32 * 4) >> (channel * 2)) & 3) as usize;
            let width = ((self.sens.read(0x2c) >> (unit * 2)) & 3) + 9;
            // ESP-IDF esp32/adc_cali_line_fitting.c, nominal 1100 mV eFuse Vref.
            let scales = [
                [57431, 76236, 105481, 196602],
                [57236, 76175, 105678, 197170],
            ];
            let offsets = [[75, 78, 107, 142], [63, 66, 89, 128]];
            let slope = 1100 * scales[unit][attenuation] / 4096;
            let volts = self.analog.volts(pins[channel], self.now_cycles);
            let mv = ((volts.clamp(0.0, 3.3) * 1000.0).round() as u32)
                .saturating_sub(offsets[unit][attenuation]);
            // ponytail: linear transfer only; add the high-attenuation LUT for accuracy above raw 2880.
            raw = ((mv * 65536 + slope / 2) / slope).min(4095) >> (12 - width);
            if control & (1 << (28 + unit)) == 0 {
                raw ^= (1 << width) - 1;
            }
        }
        // ponytail: conversion completes immediately; schedule completion when sampling timing matters.
        self.sens.write(off, (value & !0x1ffff) | DONE | raw);
    }

    fn measure_touch(&mut self) {
        if self.sens.read(0x84) & (1 << 11) == 0 {
            return;
        }
        let enabled = self.sens.read(0x8c) & 0x3ff;
        for register_channel in 0..10 {
            // Classic SENS result/enable bits exchange T8 and T9; RTC pad routing does not.
            let channel = match register_channel {
                8 => 9,
                9 => 8,
                n => n,
            };
            let pad = self.rtc_io.read(0x94 + channel as u32 * 4);
            let active = enabled & (1 << register_channel) != 0 && pad & (7 << 23) != 0;
            // ponytail: fixed capacitance baseline; add a capacitance input when analog touch timing matters.
            let count = if !active {
                0
            } else if self.touched[channel] {
                300
            } else {
                1000
            };
            let off = 0x70 + (register_channel as u32 / 2) * 4;
            let shift = if register_channel & 1 == 0 { 16 } else { 0 };
            self.sens.write(
                off,
                (self.sens.read(off) & !(0xffff << shift)) | (count << shift),
            );
        }
        self.sens.write(0x84, self.sens.read(0x84) | (1 << 10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(adc: &mut ClassicAdc, pin: u8) {
        let &(_, off, mux) = PADS.iter().find(|&&(p, _, _)| p == pin).unwrap();
        adc.write_rtc_io(off, adc.read_rtc_io(off) | (1 << mux));
    }

    fn sample(adc: &mut ClassicAdc, unit: usize, channel: usize) -> u32 {
        let off = if unit == 0 { 0x54 } else { 0x94 };
        let value = PAD_FORCE | START_FORCE | (1 << (19 + channel));
        adc.write_sens(off, value);
        assert_eq!(adc.read_sens(off) & DONE, 0);
        adc.write_sens(off, value | START);
        assert_ne!(adc.read_sens(off) & DONE, 0);
        adc.read_sens(off) & 0xffff
    }

    #[test]
    fn adc_channels_mux_width_attenuation_and_input_changes() {
        let mut adc = ClassicAdc::new();
        adc.write_sens(0, 1 << 28);
        adc.write_sens(0x90, 1 << 29);
        adc.write_sens(0x34, 0xffff);
        adc.write_sens(0x38, 0xfffff);
        for (unit, pins) in [&ADC1_PINS[..], &ADC2_PINS[..]].iter().enumerate() {
            for (channel, &pin) in pins.iter().enumerate() {
                adc.analog.set(pin, esp_periph::AnalogSource::Const(1.650));
                assert_eq!(
                    sample(&mut adc, unit, channel),
                    if unit == 0 { 1872 } else { 1884 }
                );
                route(&mut adc, pin);
                assert_eq!(
                    sample(&mut adc, unit, channel),
                    if unit == 0 { 1872 } else { 1884 }
                );
            }
        }
        adc.analog.set(34, esp_periph::AnalogSource::Const(0.800));
        assert_eq!(sample(&mut adc, 0, 6), 817);
        for width in 0..4 {
            adc.write_sens(0x2c, 0xc | width);
            assert_eq!(sample(&mut adc, 0, 6), 817 >> (3 - width));
        }
        adc.write_sens(0x2c, 0xf);
        let expected = [3081, 2311, 1603, 817];
        for attenuation in 0..4 {
            adc.write_sens(0x34, attenuation << 12);
            assert_eq!(sample(&mut adc, 0, 6), expected[attenuation as usize]);
        }
        adc.write_sens(0, 0);
        assert_eq!(sample(&mut adc, 0, 6), 4095 - 817);
        adc.write_rtc_io(0x80, 0);
        assert_eq!(sample(&mut adc, 0, 6), 4095 - 817);
    }

    #[test]
    fn adc_start_edge_read_only_result_and_controller_selection() {
        let mut adc = ClassicAdc::new();
        route(&mut adc, 34);
        adc.write_sens(0, 1 << 28);
        adc.analog.set(34, esp_periph::AnalogSource::Const(3.300));
        assert_eq!(sample(&mut adc, 0, 6), 4095);
        adc.analog.set(34, esp_periph::AnalogSource::Const(0.000));
        let start = PAD_FORCE | START_FORCE | (1 << 25) | START;
        adc.write_sens(0x54, start);
        assert_eq!(adc.read_sens(0x54) & 0x1ffff, DONE | 4095);
        assert_eq!(sample(&mut adc, 0, 6), 0);
        adc.write_sens(0, (1 << 28) | (1 << 27));
        adc.write_sens(0x54, start & !START);
        adc.write_sens(0x54, start | DONE | 42);
        assert_eq!(adc.read_sens(0x54) & 0x1ffff, 0);
        adc.write_sens(0, 1 << 28);
        adc.analog.set(34, esp_periph::AnalogSource::Const(1.650));
        adc.write_sens(0x0c, 2 << 18);
        assert_eq!(sample(&mut adc, 0, 6), 0);
    }

    #[test]
    fn dac_requires_power_and_dc_mode_with_either_gpio_mux() {
        let mut adc = ClassicAdc::new();
        for channel in 0..2 {
            let off = 0x84 + channel * 4;
            let value = (128 << 19) | (1 << 18) | (1 << 10);
            adc.write_rtc_io(off, value);
            assert!(adc.dac_outputs().contains(&(25 + channel as u8, 1656)));
            adc.write_rtc_io(off, value | (1 << 17));
        }
        assert_eq!(adc.dac_outputs(), vec![(25, 1656), (26, 1656)]);
        adc.write_sens(0x9c, 1 << 24);
        assert_eq!(adc.dac_outputs(), vec![(26, 1656)]);
        adc.write_rtc_io(0x88, 1 << 17);
        assert!(adc.dac_outputs().is_empty());
    }

    #[test]
    fn touch_oneshot_timer_and_t8_t9_routing() {
        let mut adc = ClassicAdc::new();
        adc.write_sens(0x84, (1 << 11) | (1 << 12) | (1 << 13));
        assert_ne!(adc.read_sens(0x84) & (1 << 10), 0);
        for off in (0x70..=0x80).step_by(4) {
            assert_eq!(adc.read_sens(off), 1000 << 16 | 1000);
        }
        for pin in TOUCH_PINS {
            route(&mut adc, pin);
        }
        adc.set_touch_input(4, true);
        assert_eq!(adc.read_sens(0x70) >> 16, 1000);
        adc.write_sens(0x84, 1 << 11);
        adc.set_touch_timer_enabled(true);
        assert_eq!(adc.read_sens(0x70) >> 16, 300);
        adc.set_touch_input(33, true);
        assert_eq!(adc.read_sens(0x80), 1000 << 16 | 300);
        adc.set_touch_input(32, true);
        assert_eq!(adc.read_sens(0x80), 300 << 16 | 300);
        adc.write_sens(0x8c, 1 << 8);
        assert_eq!(adc.read_sens(0x80), 300 << 16);
        assert!(!adc.set_touch_input(34, true));
    }
}
