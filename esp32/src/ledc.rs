//! Classic ESP32 LEDC: eight high-speed and eight low-speed PWM channels.
use emu_core::ClockDomain;
use esp_periph::{Device, RegRam, WriteEffect, APB_HZ};

const REF_TICK_HZ: u64 = 1_000_000;
const RC_FAST_HZ: u64 = 8_000_000;
const HS_SIGNAL0: usize = 71;
const CHANNELS: usize = 16;
const TIMERS: usize = 8;

pub struct ClassicLedc {
    regs: RegRam,
    active_conf: [u32; CHANNELS],
    active_duty: [u32; CHANNELS],
    duty_read: [u32; CHANNELS],
    pending: [bool; CHANNELS],
    unsupported_fade: [bool; CHANNELS],
    timer_conf: [u32; TIMERS],
    phase: [u128; TIMERS],
    raw: u32,
    ena: u32,
    dirty_signals: u16,
    pub clock_enabled: bool,
}

impl ClassicLedc {
    pub fn new() -> Self {
        Self {
            regs: RegRam::new(),
            active_conf: [0; CHANNELS],
            active_duty: [0; CHANNELS],
            duty_read: [0; CHANNELS],
            pending: [false; CHANNELS],
            unsupported_fade: [false; CHANNELS],
            timer_conf: [0; TIMERS],
            phase: [0; TIMERS],
            raw: 0,
            ena: 0,
            dirty_signals: u16::MAX,
            clock_enabled: false,
        }
    }

    fn channel(off: u32) -> Option<(usize, u32)> {
        let group = (off / 0xa0) as usize;
        if group > 1 {
            return None;
        }
        let within = off % 0xa0;
        (within < 8 * 0x14).then_some((group * 8 + (within / 0x14) as usize, within % 0x14))
    }

    fn timer(off: u32) -> Option<(usize, u32)> {
        if !(0x140..0x180).contains(&off) {
            return None;
        }
        let group = ((off - 0x140) / 0x20) as usize;
        let within = (off - 0x140) % 0x20;
        Some((group * 4 + (within / 8) as usize, within % 8))
    }

    fn channel_base(channel: usize) -> u32 {
        (channel / 8) as u32 * 0xa0 + (channel % 8) as u32 * 0x14
    }

    fn timer_settings(&self, timer: usize) -> Option<(u64, u64, u64)> {
        if !self.clock_enabled {
            return None;
        }
        let conf = self.timer_conf[timer];
        let resolution = conf & 0x1f;
        let divider = (conf >> 5) & 0x3ffff;
        if resolution == 0 || resolution > 20 || divider < 256 || conf & (1 << 23) != 0 {
            return None;
        }
        let source = if conf & (1 << 25) == 0 {
            REF_TICK_HZ
        } else if timer < 4 || self.regs.read(0x190) & 1 != 0 {
            APB_HZ
        } else {
            RC_FAST_HZ
        };
        Some((1u64 << resolution, divider as u64, source))
    }

    fn latch_channel(&mut self, channel: usize) {
        let base = Self::channel_base(channel);
        self.active_conf[channel] = self.regs.read(base) & !0x10;
        self.active_duty[channel] = self.regs.read(base + 8) & 0x1ffffff;
        let fade = self.regs.read(base + 12) & 0x3ff;
        self.unsupported_fade[channel] = fade != 0;
        self.pending[channel] = self.regs.read(base + 12) & (1 << 31) != 0 && fade == 0;
        self.dirty_signals |= 1 << channel;
    }

    fn latch_timer(&mut self, timer: usize, value: u32) {
        self.timer_conf[timer] = value & !((1 << 24) | (1 << 26));
        if value & (1 << 24) != 0 {
            self.phase[timer] = 0;
        }
    }

    pub fn take_signal_updates(&mut self) -> u16 {
        std::mem::take(&mut self.dirty_signals)
    }

    pub fn signal_level(&self, channel: usize) -> (bool, bool) {
        let conf = self.active_conf[channel];
        let enabled = conf & 4 != 0 && !self.unsupported_fade[channel];
        let level = if enabled {
            self.duty_read[channel] != 0
        } else {
            conf & 8 != 0
        };
        (level, enabled)
    }

    pub fn pwm(&self, signal: usize) -> Option<(f64, u32)> {
        let channel = signal.checked_sub(HS_SIGNAL0)?;
        if channel >= CHANNELS
            || self.active_conf[channel] & 4 == 0
            || self.unsupported_fade[channel]
        {
            return None;
        }
        let timer = channel / 8 * 4 + (self.active_conf[channel] & 3) as usize;
        let (period, divider, source) = self.timer_settings(timer)?;
        let full = period * 16;
        let high = (self.duty_read[channel] as u64).min(full);
        let duty = ((high * 65535 + full / 2) / full) as u32;
        Some((source as f64 * 256.0 / divider as f64 / period as f64, duty))
    }

    fn ticks_until_wrap(&self, timer: usize) -> Option<u64> {
        let (period, divider, source) = self.timer_settings(timer)?;
        let modulus = APB_HZ as u128 * divider as u128 * period as u128;
        let remaining = modulus - self.phase[timer];
        Some(
            remaining
                .div_ceil(source as u128 * 256)
                .min(u64::MAX as u128) as u64,
        )
    }
}

impl Default for ClassicLedc {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for ClassicLedc {
    fn read(&mut self, off: u32) -> u32 {
        if let Some((channel, reg)) = Self::channel(off) {
            return if reg == 0x10 {
                self.duty_read[channel]
            } else {
                self.regs.read(off)
            };
        }
        if let Some((timer, reg)) = Self::timer(off) {
            if reg == 4 {
                return self.timer_settings(timer).map_or(0, |(_, divider, _)| {
                    (self.phase[timer] / (APB_HZ as u128 * divider as u128)) as u32
                });
            }
        }
        match off {
            0x180 => self.raw,
            0x184 => self.raw & self.ena,
            0x188 => self.ena,
            0x18c => 0,
            _ => self.regs.read(off),
        }
    }

    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        if let Some((channel, reg)) = Self::channel(off) {
            match reg {
                0 => self.regs.write(off, value & 0x8000_001f),
                4 => self.regs.write(off, value & 0xfffff),
                8 => self.regs.write(off, value & 0x1ffffff),
                12 => self.regs.write(off, value),
                16 => return WriteEffect::NONE,
                _ => unreachable!(),
            }
            if channel < 8 || reg == 0 && value & (1 << 4) != 0 {
                self.latch_channel(channel);
                if channel >= 8 {
                    self.regs.write(off, self.regs.read(off) & !(1 << 4));
                }
            }
            return WriteEffect::NONE;
        }
        if let Some((timer, reg)) = Self::timer(off) {
            if reg == 0 {
                self.regs.write(off, value & 0x07ff_ffff & !(1 << 26));
                if timer < 4 || value & (1 << 26) != 0 {
                    self.latch_timer(timer, value);
                }
            }
            return WriteEffect::NONE;
        }
        match off {
            0x180 | 0x184 => {}
            0x188 => self.ena = value & 0x00ff_ffff,
            0x18c => self.raw &= !value,
            _ => self.regs.write(off, value),
        }
        WriteEffect::NONE
    }

    fn irq_sources(&self) -> u64 {
        u64::from(self.raw & self.ena != 0)
    }

    fn clock(&self) -> Option<ClockDomain> {
        Some(ClockDomain::Apb)
    }

    fn tick(&mut self, ticks: u64) {
        for timer in 0..TIMERS {
            let Some((period, divider, source)) = self.timer_settings(timer) else {
                continue;
            };
            let modulus = APB_HZ as u128 * divider as u128 * period as u128;
            let phase = self.phase[timer] + ticks as u128 * source as u128 * 256;
            self.phase[timer] = phase % modulus;
            if phase < modulus {
                continue;
            }
            self.raw |= 1 << timer;
            for channel in 0..CHANNELS {
                if self.pending[channel]
                    && channel / 8 * 4 + (self.active_conf[channel] & 3) as usize == timer
                {
                    self.duty_read[channel] = self.active_duty[channel];
                    self.pending[channel] = false;
                    let conf1 = Self::channel_base(channel) + 12;
                    self.regs.write(conf1, self.regs.read(conf1) & !(1 << 31));
                    self.raw |= 1 << (8 + channel);
                    self.dirty_signals |= 1 << channel;
                }
            }
        }
    }

    fn has_deadline(&self) -> bool {
        true
    }

    fn next_deadline(&self) -> Option<u64> {
        (0..TIMERS)
            .filter(|&timer| {
                self.ena & (1 << timer) != 0
                    || (0..CHANNELS).any(|channel| {
                        self.pending[channel]
                            && self.ena & (1 << (8 + channel)) != 0
                            && channel / 8 * 4 + (self.active_conf[channel] & 3) as usize == timer
                    })
            })
            .filter_map(|timer| self.ticks_until_wrap(timer))
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configure(ledc: &mut ClassicLedc, channel: usize, timer: usize, low_speed: bool) {
        ledc.clock_enabled = true;
        if low_speed {
            ledc.write(0x190, 1);
        }
        let timer_off = if low_speed { 0x160 } else { 0x140 } + timer as u32 * 8;
        let channel_off = if low_speed { 0xa0 } else { 0 } + channel as u32 * 0x14;
        ledc.write(
            timer_off,
            8 | (16000 << 5) | (1 << 25) | u32::from(low_speed) << 26,
        );
        ledc.write(channel_off + 8, 64 << 4);
        ledc.write(
            channel_off + 12,
            (1 << 31) | (1 << 30) | (1 << 20) | (1 << 10),
        );
        ledc.write(channel_off, timer as u32 | 4 | u32::from(low_speed) << 4);
    }

    #[test]
    fn high_and_low_speed_channels_latch_duty_and_report_pwm() {
        let mut ledc = ClassicLedc::new();
        configure(&mut ledc, 0, 0, false);
        configure(&mut ledc, 0, 0, true);
        assert_eq!(ledc.read(0x10), 0);
        assert_eq!(ledc.read(0xb0), 0);
        ledc.tick(15999);
        assert_eq!(ledc.read(0x10), 0);
        ledc.tick(1);
        assert_eq!(ledc.read(0x10), 64 << 4);
        assert_eq!(ledc.read(0xb0), 64 << 4);
        for signal in [71, 79] {
            let (hz, duty) = ledc.pwm(signal).unwrap();
            assert!((hz - 5000.0).abs() < 0.001);
            assert_eq!(duty, 16384);
        }
    }

    #[test]
    fn low_speed_shadow_registers_require_para_up() {
        let mut ledc = ClassicLedc::new();
        ledc.clock_enabled = true;
        ledc.write(0x190, 1);
        ledc.write(0x160, 8 | (16000 << 5) | (1 << 25));
        ledc.write(0xa8, 64 << 4);
        ledc.write(0xac, (1 << 31) | (1 << 30) | (1 << 20) | (1 << 10));
        ledc.write(0xa0, 4);
        ledc.tick(100_000);
        assert!(ledc.pwm(79).is_none());
        assert_eq!(ledc.read(0xb0), 0);

        ledc.write(0x160, 8 | (16000 << 5) | (1 << 25) | (1 << 26));
        ledc.write(0xa0, 4 | (1 << 4));
        assert_eq!(ledc.read(0xa0) & (1 << 4), 0);
        assert_eq!(ledc.read(0x160) & (1 << 26), 0);
        ledc.tick(16000);
        assert_eq!(ledc.read(0xb0), 64 << 4);
    }

    #[test]
    fn interrupt_status_and_clear_cover_timer_and_duty_completion() {
        let mut ledc = ClassicLedc::new();
        configure(&mut ledc, 0, 0, false);
        ledc.write(0x188, (1 << 8) | 1);
        assert_eq!(ledc.irq_sources(), 0);
        ledc.tick(16000);
        assert_eq!(ledc.read(0x180) & 0x101, 0x101);
        assert_eq!(ledc.read(0x184) & 0x101, 0x101);
        assert_eq!(ledc.irq_sources(), 1);
        ledc.write(0x18c, 0x101);
        assert_eq!(ledc.irq_sources(), 0);
    }
}
