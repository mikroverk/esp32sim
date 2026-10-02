//! Low-speed LEDC channels shared by S3, C3 and C6. Register layout follows the
//! ESP-IDF 5.5 `soc/ledc_reg.h` and `hal/ledc_ll.h` for each target.
//! Static duty updates are modeled; hardware fade sequences are not yet modeled.
use crate::{Device, Gpio, RegRam, WriteEffect, APB_HZ};
use emu_core::ClockDomain;

#[derive(Clone, Copy)]
pub enum LedcLayout { S3, C3, C6 }

pub struct Ledc {
    layout: LedcLayout,
    regs: RegRam,
    duty: [u32; 8],
    pending: [bool; 8],
    unsupported_fade: [bool; 8],
    phase: [u128; 4],
    timer_params: [u32; 4],
    raw: u32,
    ena: u32,
    /// C6 selects and gates its source clock in PCR, outside the LEDC block.
    pub external_clock_hz: u64,
    pub clock_enabled: bool,
}

impl Ledc {
    pub fn new(layout: LedcLayout) -> Self {
        Self { layout, regs: RegRam::new(), duty: [0; 8], pending: [false; 8],
            unsupported_fade: [false; 8], phase: [0; 4], timer_params: [0; 4], raw: 0, ena: 0, external_clock_hz: 0, clock_enabled: true }
    }
    fn channels(&self) -> usize { if matches!(self.layout, LedcLayout::S3) { 8 } else { 6 } }
    fn shift(&self) -> u32 { u32::from(matches!(self.layout, LedcLayout::C6)) }
    fn source_hz(&self) -> u64 {
        if !self.clock_enabled { return 0; }
        if matches!(self.layout, LedcLayout::C6) { return self.external_clock_hz; }
        match self.regs.read(0xd0) & 3 { 1 => APB_HZ, 2 => 17_500_000, 3 => 40_000_000, _ => 0 }
    }
    fn timer_settings(&self, n: usize) -> Option<(u64, u64)> {
        let conf = self.timer_params[n];
        let shift = self.shift();
        let resolution = conf & (0xf | (shift << 4));
        let divider = (conf >> (4 + shift)) & 0x3ffff;
        if resolution == 0 || resolution > 14 + shift * 6 || divider < 256 { return None; }
        Some((1u64 << resolution, divider as u64))
    }
    fn timer(&self, n: usize) -> Option<(u64, u64)> {
        if self.regs.read(0xa0 + n as u32 * 8) & (3 << (22 + self.shift())) != 0 { return None; }
        self.timer_settings(n)
    }
    /// The physical high-time fraction, including matrix inversion, normalized to 65535.
    /// No pulse edges are synthesized; callers observe channel configuration separately.
    pub fn output(&self, gpio: &Gpio, pin: u32) -> Option<(f64, u32)> {
        let matrix = *gpio.func_out_sel.get(pin as usize)?;
        if gpio.enable & (1u64 << pin) == 0 { return None; }
        let base = match self.layout { LedcLayout::S3 => 73, LedcLayout::C3 => 45, LedcLayout::C6 => 0 };
        let channel = (matrix & 0x1ff).checked_sub(base)? as usize;
        if channel >= self.channels() || self.unsupported_fade[channel] { return None; }
        let conf = self.regs.read(channel as u32 * 0x14);
        if conf & 4 == 0 { return None; }
        let (period, divider) = self.timer((conf & 3) as usize)?;
        let hz = self.source_hz();
        if hz == 0 { return None; }
        let full = period * 16;
        let high = (self.duty[channel] as u64).min(full);
        let mut duty = ((high * 65535 + full / 2) / full) as u32;
        if matrix & (1 << 9) != 0 { duty = 65535 - duty; }
        Some((hz as f64 * 256.0 / divider as f64 / period as f64, duty))
    }
}

impl Device for Ledc {
    fn read(&mut self, off: u32) -> u32 {
        if off < self.channels() as u32 * 0x14 && off % 0x14 == 0x10 { return self.duty[(off / 0x14) as usize]; }
        if (0xa4..=0xbc).contains(&off) && off % 8 == 4 {
            let n = ((off - 0xa4) / 8) as usize;
            return self.timer_settings(n).map_or(0, |(_, div)| (self.phase[n] / (APB_HZ as u128 * div as u128)) as u32);
        }
        match off { 0xc0 => self.raw, 0xc4 => self.raw & self.ena, 0xc8 => self.ena, 0xcc => 0, _ => self.regs.read(off) }
    }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        match off {
            0xc0 | 0xc4 => {}, 0xc8 => self.ena = value, 0xcc => self.raw &= !value,
            _ if off < self.channels() as u32 * 0x14 => {
                let n = (off / 0x14) as usize;
                match off % 0x14 {
                    0x10 => {},
                    0x08 => self.regs.write(off, value & if self.shift() == 1 { 0x1ffffff } else { 0x7ffff }),
                    0x04 => self.regs.write(off, value & if self.shift() == 1 { 0xfffff } else { 0x3fff }),
                    0x00 => {
                        self.regs.write(off, value & !(1 << 4));
                        if value & (1 << 4) != 0 && self.regs.read(off + 12) & (1 << 31) != 0 {
                            let fade = if matches!(self.layout, LedcLayout::C6) {
                                (self.regs.read(0x100 + n as u32 * 16) >> 11) & 0x3ff
                            } else { self.regs.read(off + 12) & 0x3ff };
                            self.unsupported_fade[n] = fade != 0;
                            self.pending[n] = fade == 0;
                        }
                    }
                    _ => self.regs.write(off, value),
                }
            }
            _ if (0xa0..=0xb8).contains(&off) && off.is_multiple_of(8) => {
                self.regs.write(off, value & !(1 << (25 + self.shift())));
                if value & (1 << (25 + self.shift())) != 0 { self.timer_params[((off - 0xa0) / 8) as usize] = value; }
                if value & (1 << (23 + self.shift())) != 0 { self.phase[((off - 0xa0) / 8) as usize] = 0; }
            }
            _ => self.regs.write(off, value),
        }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 { u64::from(self.raw & self.ena != 0) }
    fn clock(&self) -> Option<ClockDomain> { Some(ClockDomain::Apb) }
    fn tick(&mut self, ticks: u64) {
        let hz = self.source_hz();
        if hz == 0 { return; }
        for n in 0..4 {
            let Some((period, divider)) = self.timer(n) else { continue; };
            let modulus = APB_HZ as u128 * divider as u128 * period as u128;
            let phase = self.phase[n] + ticks as u128 * hz as u128 * 256;
            self.phase[n] = phase % modulus;
            if phase < modulus { continue; }
            self.raw |= 1 << n;
            for ch in 0..self.channels() {
                let off = ch as u32 * 0x14;
                if self.pending[ch] && self.regs.read(off) & 3 == n as u32 {
                    self.duty[ch] = self.regs.read(off + 8);
                    self.pending[ch] = false;
                    let conf = self.regs.read(off + 12);
                    self.regs.write(off + 12, conf & !(1 << 31));
                    self.raw |= 1 << (4 + ch);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_duty_latches_at_wrap_and_preserves_fractional_divider() {
        for (layout, signal) in [(LedcLayout::S3, 73), (LedcLayout::C3, 45), (LedcLayout::C6, 0)] {
            let mut ledc = Ledc::new(layout);
            ledc.external_clock_hz = 40_000_000;
            ledc.write(0xd0, 3);
            let mut gpio = Gpio::new();
            gpio.enable = (1 << 4) | (1 << 5);
            gpio.func_out_sel[4] = signal;
            gpio.func_out_sel[5] = signal | (1 << 9);
            // XTAL / 1.5 / 256, 25% duty, both pins attached to the same channel.
            let shift = ledc.shift();
            ledc.write(0xa0, 8 | (384 << (4 + shift)));
            assert!(ledc.timer_settings(0).is_none());
            ledc.write(0xa0, 8 | (384 << (4 + shift)) | (1 << (25 + shift)));
            ledc.write(8, 64 << 4);
            ledc.write(12, 1 << 31);
            ledc.write(0, 4 | (1 << 4));
            assert_eq!(ledc.read(16), 0);
            ledc.tick(767);
            assert_eq!(ledc.read(16), 0);
            ledc.tick(1);
            assert_eq!(ledc.read(16), 64 << 4);
            let (hz, duty) = ledc.output(&gpio, 4).unwrap();
            assert!((hz - 40_000_000.0 / 384.0).abs() < 0.001);
            assert_eq!(duty, 16384);
            assert_eq!(ledc.output(&gpio, 5).unwrap().1, 49151);
            assert_eq!(ledc.read(12) >> 31, 0);
            ledc.write(0xc8, 1 << 4);
            assert_eq!(ledc.irq_sources(), 1);
            ledc.write(0xcc, 1 << 4);
            assert_eq!(ledc.irq_sources(), 0);
            ledc.tick(200);
            let counter = ledc.read(0xa4);
            assert!(counter > 0);
            ledc.write(0xa0, 8 | (384 << (4 + shift)) | (1 << (22 + shift)));
            ledc.tick(1000);
            assert_eq!(ledc.read(0xa4), counter);
            assert!(ledc.output(&gpio, 4).is_none());
        }
    }
    #[test]
    fn c6_twenty_bit_timer_and_duty_do_not_truncate_or_overflow() {
        let mut ledc = Ledc::new(LedcLayout::C6);
        ledc.external_clock_hz = 80_000_000;
        ledc.write(0xa0, 20 | (0x3ffff << 5) | (1 << 26));
        ledc.write(8, 1 << 24);
        ledc.write(12, 1 << 31);
        ledc.write(0, 4 | (1 << 4));
        ledc.tick(u32::MAX as u64);
        let mut gpio = Gpio::new();
        gpio.enable = 1;
        gpio.func_out_sel[0] = 0;
        assert_eq!(ledc.read(16), 1 << 24);
        assert_eq!(ledc.output(&gpio, 0).unwrap().1, 65535);
        assert!(ledc.read(0xa4) < 1 << 20);
        ledc.clock_enabled = false;
        let count = ledc.read(0xa4);
        ledc.tick(u64::MAX);
        assert_eq!(ledc.read(0xa4), count);
        assert!(ledc.output(&gpio, 0).is_none());
    }

    #[test]
    fn unsupported_fade_does_not_claim_successful_duty_or_completion() {
        let mut ledc = Ledc::new(LedcLayout::C3);
        ledc.write(0xd0, 1);
        ledc.write(0xa0, 8 | (256 << 4) | (1 << 25));
        ledc.write(8, 128 << 4);
        ledc.write(12, (1 << 31) | (4 << 20) | 1);
        ledc.write(0, 4 | (1 << 4));
        ledc.tick(80000);
        let mut gpio = Gpio::new();
        gpio.enable = 1;
        gpio.func_out_sel[0] = 45;
        assert!(ledc.output(&gpio, 0).is_none());
        assert_eq!(ledc.read(0xc0) & (1 << 4), 0);
        assert_ne!(ledc.read(12) & (1 << 31), 0);
    }

}
