//! Low-speed LEDC channels shared by S3, C3, C6 and P4, and the classic ESP32's two speed groups.
//! Register layout follows the ESP-IDF 5.5 `soc/ledc_reg.h` and `hal/ledc_ll.h` for each target.
//! Duty updates and hardware fades are modelled at timer wraps: DUTY_START with PARA_UP puts DUTY in
//! effect at the next wrap of the channel's timer, then every DUTY_CYCLE wraps the duty moves by
//! DUTY_SCALE (integer duty units, above the 4 fraction bits), DUTY_NUM times per range, and the
//! last step raises DUTY_CHNG_END_CHn (INT_RAW bit 4 + n), which IDF's fade ISR waits for. S3, C3
//! and the classic groups have one range in CONF1 (DUTY_SCALE 9:0, DUTY_CYCLE 19:10, DUTY_NUM
//! 29:20, DUTY_INC 30); C6 and P4 run up to 16 gamma ranges (word: DUTY_INC 0, DUTY_CYCLE 10:1,
//! SCALE 20:11, DUTY_NUM 30:21; GAMMA_ENTRY_NUM 4:0 counts them). A plain update (one range,
//! SCALE 0) ends at the wrap that loads it.
use crate::{Device, Gpio, RegRam, WriteEffect, APB_HZ};
use emu_core::ClockDomain;

#[derive(Clone, Copy)]
/// `C6`: gamma RAM behind CHn_GAMMA_WR (0x100 + 0x10n), _WR_ADDR (+4), _RD_ADDR (+8) and
/// _RD_DATA (+0xc), kept in register storage at the P4's gamma RAM offsets; CHn_GAMMA_CONF at 0x180 + 4n (IDF v5.5.4 esp32c6 soc/ledc_reg.h, hal/ledc_ll.h
/// `ledc_ll_set_fade_param_range`). `P4`: the C6's register fields (and its external clock) with
/// the S3's eight channels; CHn_GAMMA_CONF at 0x100 + 4n and the gamma RAM mapped at
/// 0x400 + 0x40n (IDF v5.5.4 esp32p4 register/hw_ver3/soc/ledc_reg.h, ledc_struct.h
/// `ledc_gamma_ram_t`, ld/esp32p4.peripherals.ld LEDC_GAMMA_RAM 0x500D3400).
pub enum LedcLayout { S3, C3, C6, P4 }

/// `CLASSIC` marks a classic ESP32 speed group: S3's eight channels, C6-width timer fields and a
/// PARA_UP write that drops a pending duty update. S3/C3/C6/P4 use the default.
pub struct Ledc<const CLASSIC: bool = false> {
    layout: LedcLayout,
    regs: RegRam,
    duty: [u32; 8],
    pending: [bool; 8],
    fade: [Fade; 8],
    /// bit n: channel n runs a fade
    fading: u8,
    // Phase units cancel the divider's factor of 256, so even C6's maximum period fits u64.
    phase: [u64; 4],
    /// The rate of the ticks `tick` counts / 256 (phase units per tick at divider 1.0): the Apb
    /// clock domain, which the P4's device table runs at 40 MHz (CPU 360 MHz / 9) and the other
    /// chips at 80 MHz. Set with the layout, so the tick does not test the layout.
    tick_unit: u64,
    active: u8,
    timer_params: [u32; 4],
    raw: u32,
    ena: u32,
    /// C6 selects and gates its source clock in PCR, outside the LEDC block.
    pub external_clock_hz: u64,
    pub clock_enabled: bool,
}

/// A running duty change.
#[derive(Clone, Copy, Default)]
struct Fade {
    /// DUTY is in effect (the first wrap after DUTY_START loads it)
    loaded: bool,
    range: u8,
    ranges: u8,
    /// steps left in this range
    steps: u16,
    /// wraps since the last step
    count: u16,
}

impl Ledc {
    pub fn new(layout: LedcLayout) -> Self { Self::with_layout(layout) }
    /// The physical high-time fraction, including matrix inversion, normalized to 65535.
    /// No pulse edges are synthesized; callers observe channel configuration separately.
    pub fn output(&self, gpio: &Gpio, pin: u8) -> Option<(f64, u32)> {
        let matrix = *gpio.func_out_sel.get(pin as usize)?;
        if gpio.enable & (1u64 << pin) == 0 { return None; }
        let base = match self.layout { LedcLayout::S3 => 73, LedcLayout::C3 => 45, LedcLayout::C6 => 0, LedcLayout::P4 => 126 };
        let bits = if matches!(self.layout, LedcLayout::S3 | LedcLayout::P4) { 9 } else { 8 };
        let channel = (matrix & ((1 << bits) - 1)).checked_sub(base)? as usize;
        let timer = self.channel_timer_settings(channel)?;
        let hz = self.source_hz();
        if hz == 0 { return None; }
        let (hz, mut duty) = self.channel_wave(channel, timer, hz);
        if matrix & (1 << bits) != 0 { duty = 65535 - duty; }
        Some((hz, duty))
    }
}

/// Classic speed groups: their source clock and matrix signals are chosen by the classic adapter.
impl Ledc<true> {
    pub fn new_classic() -> Self { Self::with_layout(LedcLayout::S3) }
    /// Frequency and normalized duty of one channel clocked at `hz`.
    pub fn channel_output(&self, channel: usize, hz: u64) -> Option<(f64, u32)> {
        let timer = self.channel_timer_settings(channel)?;
        if hz == 0 { return None; }
        Some(self.channel_wave(channel, timer, hz))
    }
    pub fn channel_timer(&self, channel: usize) -> usize { (self.regs.read(channel as u32 * 0x14) & 3) as usize }
    pub fn timer_conf(&self, timer: usize) -> u32 { self.timer_params[timer] }
    pub fn signal_level(&self, channel: usize) -> (bool, bool) {
        let conf = self.regs.read(channel as u32 * 0x14);
        let enabled = conf & 4 != 0;
        (if enabled { self.duty[channel] != 0 } else { conf & 8 != 0 }, enabled)
    }
    pub fn next_wrap(&self, source: impl Fn(usize, u32) -> u64) -> Option<u64> { self.wrap_deadline(true, source) }
    pub fn any_timer(&self) -> bool { self.active != 0 }
    pub fn fading(&self) -> bool { self.fading != 0 }
}

// Each variant gets its own non-generic code, so S3/C3/C6 compile exactly one copy of each
// method as before; a generic impl would be instantiated per calling crate.
macro_rules! ledc_impl { ($classic:literal) => {
impl Ledc<$classic> {
    fn with_layout(layout: LedcLayout) -> Self {
        Self { layout, regs: RegRam::new(), duty: [0; 8], pending: [false; 8],
            fade: [Fade::default(); 8], fading: 0, phase: [0; 4],
            tick_unit: (if matches!(layout, LedcLayout::P4) { 40_000_000 } else { APB_HZ }) / 256, active: 0, timer_params: [0; 4], raw: 0, ena: 0, external_clock_hz: 0, clock_enabled: true }
    }
    fn channels(&self) -> usize { if matches!(self.layout, LedcLayout::S3 | LedcLayout::P4) { 8 } else { 6 } }
    fn shift(&self) -> u32 { u32::from($classic || matches!(self.layout, LedcLayout::C6 | LedcLayout::P4)) }
    fn source_hz(&self) -> u64 {
        if !self.clock_enabled { return 0; }
        if matches!(self.layout, LedcLayout::C6 | LedcLayout::P4) { return self.external_clock_hz; }
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
    /// Timer period and divider of an enabled channel with a modelled duty.
    #[inline(always)]
    fn channel_timer_settings(&self, channel: usize) -> Option<(u64, u64)> {
        if channel >= self.channels() { return None; }
        let conf = self.regs.read(channel as u32 * 0x14);
        if conf & 4 == 0 { return None; }
        self.timer((conf & 3) as usize)
    }
    /// Frequency and duty normalized to 65535 for a channel clocked at `hz`.
    #[inline(always)]
    fn channel_wave(&self, channel: usize, (period, divider): (u64, u64), hz: u64) -> (f64, u32) {
        let full = period * 16;
        let high = (self.duty[channel] as u64).min(full);
        let duty = ((high * 65535 + full / 2) / full) as u32;
        (hz as f64 * 256.0 / divider as f64 / period as f64, duty)
    }
    /// Advance the timers, each clocked at `source(timer, timer_conf)` Hz.
    #[inline(always)]
    pub fn tick_with_source(&mut self, ticks: u64, source: impl Fn(usize, u32) -> u64) {
        for n in 0..4 {
            // `active` bit n is `timer(n).is_some()`, recomputed on every timer configuration write.
            if self.active & (1 << n) == 0 { continue; }
            let Some((period, divider)) = self.timer(n) else { continue; };
            let hz = source(n, self.timer_params[n]);
            let modulus = self.tick_unit * divider * period;
            match ticks.checked_mul(hz).and_then(|delta| self.phase[n].checked_add(delta)) {
                Some(total) if total < modulus => self.phase[n] = total,
                _ => self.wrap_timer(n, ticks, hz, modulus),
            }
        }
    }
    /// The rest of `tick_with_source` for a timer whose phase reaches `modulus` (or overflows) now.
    #[cold]
    #[inline(never)]
    fn wrap_timer(&mut self, n: usize, ticks: u64, hz: u64, modulus: u64) {
        let (phase, wraps) = match ticks.checked_mul(hz).and_then(|delta| self.phase[n].checked_add(delta)) {
            Some(total) => (total % modulus, total / modulus),
            None => {
                let total = self.phase[n] as u128 + ticks as u128 * hz as u128;
                ((total % modulus as u128) as u64, (total / modulus as u128).min(u64::MAX as u128) as u64)
            }
        };
        self.phase[n] = phase;
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
        if self.fading != 0 { self.step_fades(n, wraps); }
    }
    /// The fade range count of channel `ch` (GAMMA_ENTRY_NUM; IDF writes at least 1).
    fn ranges(&self, ch: usize) -> u8 {
        match self.layout {
            LedcLayout::C6 => (self.regs.read(0x180 + ch as u32 * 4) & 0x1f).max(1) as u8,
            LedcLayout::P4 => (self.regs.read(0x100 + ch as u32 * 4) & 0x1f).max(1) as u8,
            _ => 1,
        }
    }
    /// (increase, cycle, scale, steps) of fade range `r` of channel `ch`.
    fn range(&self, ch: usize, r: u8) -> (bool, u32, u32, u32) {
        // The C6 keeps its gamma RAM where the P4 maps its own (see `LedcLayout`).
        let w = match self.layout {
            LedcLayout::C6 | LedcLayout::P4 => self.regs.read(0x400 + ch as u32 * 0x40 + (r as u32 & 15) * 4),
            _ => {
                let c = self.regs.read(ch as u32 * 0x14 + 12);
                return (c & (1 << 30) != 0, (c >> 10) & 0x3ff, c & 0x3ff, (c >> 20) & 0x3ff);
            }
        };
        (w & 1 != 0, (w >> 1) & 0x3ff, (w >> 11) & 0x3ff, (w >> 21) & 0x3ff)
    }
    /// Runs the fades of the channels on timer `n` through `wraps` wraps.
    fn step_fades(&mut self, n: usize, wraps: u64) {
        let max = if self.shift() == 1 { 0x1ffffff } else { 0x7ffff };
        for ch in 0..self.channels() {
            let off = ch as u32 * 0x14;
            if self.fading & (1 << ch) == 0 || self.regs.read(off) & 3 != n as u32 { continue; }
            let (mut f, mut w) = (self.fade[ch], wraps);
            if !f.loaded {
                f.loaded = true;
                self.duty[ch] = self.regs.read(off + 8);
                f.steps = self.range(ch, 0).3 as u16;
                w = w.saturating_sub(1);
            }
            loop {
                if f.steps == 0 {
                    if f.range + 1 < f.ranges { f.range += 1; f.steps = self.range(ch, f.range).3 as u16; f.count = 0; continue; }
                    self.fading &= !(1 << ch);
                    self.raw |= 1 << (4 + ch);
                    let conf = self.regs.read(off + 12);
                    self.regs.write(off + 12, conf & !(1 << 31));
                    break;
                }
                if w == 0 { break; }
                let (inc, cycle, scale, _) = self.range(ch, f.range);
                let cycle = cycle.max(1) as u64;
                let due = ((f.count as u64).saturating_add(w) / cycle).min(f.steps as u64);
                if due == 0 { f.count += w as u16; break; }
                let delta = ((scale as u64) << 4) * due;
                let duty = self.duty[ch] as u64;
                self.duty[ch] = if inc { (duty + delta).min(max) } else { duty.saturating_sub(delta) } as u32;
                w -= due * cycle - f.count as u64;
                f.count = 0;
                f.steps -= due as u16;
            }
            self.fade[ch] = f;
        }
    }
    /// Ticks until the next wrap of a timer that matters: one with a fading channel whose
    /// DUTY_CHNG_END interrupt is enabled, and with `all`, one with its overflow interrupt
    /// enabled or a pending plain update.
    fn wrap_deadline(&self, all: bool, source: impl Fn(usize, u32) -> u64) -> Option<u64> {
        (0..4).filter_map(|n| {
            let waiting = (0..self.channels()).any(|ch| (self.fading & (1 << ch) != 0 || all && self.pending[ch])
                && self.ena & (1 << (4 + ch)) != 0 && self.regs.read(ch as u32 * 0x14) & 3 == n as u32);
            if !(waiting || all && self.ena & (1 << n) != 0) { return None; }
            let (period, div) = self.timer(n)?;
            let hz = source(n, self.timer_params[n]);
            // A timer shrunk under its phase wraps at the next tick.
            (hz != 0).then(|| (self.tick_unit * div * period).saturating_sub(self.phase[n]).max(1).div_ceil(hz))
        }).min()
    }
}

impl Device for Ledc<$classic> {
    fn read(&mut self, off: u32) -> u32 {
        if off < self.channels() as u32 * 0x14 && off % 0x14 == 0x10 { return self.duty[(off / 0x14) as usize]; }
        if (0xa4..=0xbc).contains(&off) && off % 8 == 4 {
            let n = ((off - 0xa4) / 8) as usize;
            return self.timer_settings(n).map_or(0, |(_, div)| (self.phase[n] / (self.tick_unit * div)) as u32);
        }
        match off {
            0xc0 => self.raw, 0xc4 => self.raw & self.ena, 0xc8 => self.ena, 0xcc => 0,
            _ if matches!(self.layout, LedcLayout::C6) && (0x100..0x160).contains(&off) && off % 16 == 12 => {
                let ch = (off - 0x100) / 16;
                self.regs.read(0x400 + ch * 0x40 + (self.regs.read(off - 4) & 15) * 4)
            }
            _ => self.regs.read(off),
        }
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
                        if $classic && value & (1 << 4) != 0 { self.pending[n] = false; }
                        if value & (1 << 4) != 0 && self.regs.read(off + 12) & (1 << 31) != 0 {
                            let ranges = self.ranges(n);
                            let plain = ranges == 1 && self.range(n, 0).2 == 0;
                            self.pending[n] = plain;
                            self.fading = (self.fading & !(1 << n)) | (u8::from(!plain) << n);
                            self.fade[n] = Fade { ranges, ..Fade::default() };
                        }
                    }
                    _ => self.regs.write(off, value),
                }
            }
            // C6 CHn_GAMMA_WR_ADDR: stores CHn_GAMMA_WR in the range it names.
            _ if matches!(self.layout, LedcLayout::C6) && (0x100..0x160).contains(&off) && off % 16 == 4 => {
                self.regs.write(off, value & 15);
                self.regs.write(0x400 + (off - 0x100) / 16 * 0x40 + (value & 15) * 4, self.regs.read(off - 4));
            }
            _ if (0xa0..=0xb8).contains(&off) && off.is_multiple_of(8) => {
                self.regs.write(off, value & !(1 << (25 + self.shift())));
                if value & (1 << (25 + self.shift())) != 0 { self.timer_params[((off - 0xa0) / 8) as usize] = value; }
                if value & (1 << (23 + self.shift())) != 0 { self.phase[((off - 0xa0) / 8) as usize] = 0; }
                let n = ((off - 0xa0) / 8) as usize;
                self.active = (self.active & !(1 << n)) | (u8::from(self.timer(n).is_some()) << n);
            }
            _ => self.regs.write(off, value),
        }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 { u64::from(self.raw & self.ena != 0) }
    fn clock(&self) -> Option<ClockDomain> { if self.active != 0 && self.source_hz() != 0 { Some(ClockDomain::Apb) } else { None } }
    fn tick(&mut self, ticks: u64) {
        let hz = self.source_hz();
        if hz == 0 { return; }
        self.tick_with_source(ticks, |_, _| hz);
    }
    /// Only a running fade with its end interrupt enabled sets a deadline (classic: see ClassicLedc).
    /// The scan pays one byte test without a fade; the rest stays out of its frame.
    #[inline(always)]
    fn has_deadline(&self) -> bool { self.fading != 0 }
    #[inline(always)]
    fn next_deadline(&self) -> Option<u64> { if self.fading == 0 { None } else { self.fade_deadline() } }
}
impl Ledc<$classic> {
    #[cold]
    #[inline(never)]
    fn fade_deadline(&self) -> Option<u64> {
        let hz = self.source_hz();
        self.wrap_deadline(false, |_, _| hz)
    }
}
} }
ledc_impl!(false);
ledc_impl!(true);

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
            let bits = if matches!(layout, LedcLayout::S3) { 9 } else { 8 };
            gpio.func_out_sel[4] |= 1 << (bits + 1);
            gpio.func_out_sel[5] = signal | (1 << bits);
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
    /// The P4's eighth channel at 0x8C, its matrix signal 126 + 7 (IDF v5.5.4 esp32p4
    /// gpio_sig_map.h:235), and a plain duty, whose gamma range 0 has DUTY_NUM 0 at 0x400 + 7 * 0x40.
    #[test]
    fn p4_channel_seven_and_its_gamma_ram() {
        let mut ledc = Ledc::new(LedcLayout::P4);
        ledc.external_clock_hz = 80_000_000;
        ledc.write(0xa0, 10 | (256 << 5) | (1 << 26));           // timer 0: 10-bit, divider 1, PARA_UP
        ledc.write(0x8c + 8, 512 << 4);                          // channel 7 duty: half of 1024 (4 fraction bits)
        ledc.write(0x8c + 12, 1 << 31);                          // DUTY_START
        ledc.write(0x8c, 4 | (1 << 4));                          // SIG_OUT_EN, PARA_UP, timer 0
        ledc.tick(1 << 20);
        let mut gpio = Gpio::with_pins(57);
        gpio.enable = 1 << 28;
        gpio.func_out_sel[28] = 126 + 7;
        assert_eq!(ledc.read(0x8c + 16), 512 << 4);
        assert_eq!(ledc.output(&gpio, 28).unwrap().1, 32768);
        // Two gamma ranges: up 3 x 16 every 2 periods, then down 1 x 8 every period.
        ledc.write(0xcc, u32::MAX);                              // the plain update's DUTY_CHNG_END
        ledc.write(0xc8, 1 << (4 + 7));
        ledc.write(0x100 + 7 * 4, 2);                            // CH7_GAMMA_CONF ENTRY_NUM
        ledc.write(0x400 + 7 * 0x40, 1 | (2 << 1) | (16 << 11) | (3 << 21));
        ledc.write(0x400 + 7 * 0x40 + 4, (1 << 1) | (8 << 11) | (1 << 21));
        ledc.write(0x8c + 8, 100 << 4);
        ledc.write(0x8c + 12, 1 << 31);
        ledc.write(0x8c, 4 | (1 << 4));
        let period = 512u64;                                     // 40 MHz ticks per 10-bit period at 80 MHz, divider 1
        let phase = ledc.next_deadline().unwrap();
        assert!(phase > 0 && phase <= period);
        ledc.tick(phase);
        assert_eq!(ledc.read(0x8c + 16), 100 << 4, "DUTY loads at the wrap");
        ledc.tick(2 * period);
        assert_eq!(ledc.read(0x8c + 16), 116 << 4);
        ledc.tick(4 * period);
        assert_eq!(ledc.read(0x8c + 16), 148 << 4);
        assert_eq!((ledc.irq_sources(), ledc.read(0x8c + 12) >> 31), (0, 1));
        ledc.tick(period);
        assert_eq!(ledc.read(0x8c + 16), 140 << 4);
        assert_eq!((ledc.irq_sources(), ledc.read(0x8c + 12) >> 31), (1, 0));
        assert_eq!(ledc.next_deadline(), None);
        assert_eq!(ledc.output(&gpio, 28).unwrap().1, (140 * 16 * 65535 + 8192) / 16384);
    }

    /// The C6's gamma RAM sits behind CHn_GAMMA_WR / _WR_ADDR / _RD_ADDR / _RD_DATA, written the way
    /// `ledc_ll_set_fade_param_range` does: WR_ADDR, WR, WR_ADDR again.
    #[test]
    fn c6_gamma_ram_is_indirect_and_runs_each_range() {
        let mut ledc = Ledc::new(LedcLayout::C6);
        ledc.external_clock_hz = APB_HZ;
        ledc.write(0xa0, 8 | (256 << 5) | (1 << 26));            // 8-bit, divider 1: a wrap every 256 ticks
        let ch = 2u32;
        let range = |l: &mut Ledc, r: u32, w: u32| { l.write(0x104 + ch * 16, r); l.write(0x100 + ch * 16, w); l.write(0x104 + ch * 16, r); };
        range(&mut ledc, 0, (1 << 1) | (10 << 11) | (2 << 21));  // down 2 x 10, every period
        range(&mut ledc, 1, 1 | (3 << 1) | (5 << 11) | (2 << 21)); // up 2 x 5, every 3 periods
        ledc.write(0x108 + ch * 16, 1);
        assert_eq!(ledc.read(0x10c + ch * 16), 1 | (3 << 1) | (5 << 11) | (2 << 21));
        ledc.write(0x180 + ch * 4, 2);
        ledc.write(ch * 0x14 + 8, 50 << 4);
        ledc.write(ch * 0x14 + 12, 1 << 31);
        ledc.write(ch * 0x14, 4 | (1 << 4));
        ledc.tick(256 * 2);
        assert_eq!(ledc.read(ch * 0x14 + 16), 40 << 4);
        ledc.tick(256);
        assert_eq!(ledc.read(ch * 0x14 + 16), 30 << 4);
        ledc.tick(256 * 5);
        assert_eq!((ledc.read(ch * 0x14 + 16), ledc.read(0xc0) & (1 << (4 + ch))), (35 << 4, 0));
        ledc.tick(256);
        assert_eq!((ledc.read(ch * 0x14 + 16), ledc.read(0xc0) & (1 << (4 + ch))), (40 << 4, 1 << (4 + ch)));
        // The same fade in one batch, and a plain update (one range, SCALE 0) ends at its wrap.
        ledc.write(0xcc, u32::MAX);
        ledc.write(ch * 0x14 + 12, 1 << 31);
        ledc.write(ch * 0x14, 4 | (1 << 4));
        ledc.tick(256 * 100);
        assert_eq!(ledc.read(ch * 0x14 + 16), 40 << 4);
        range(&mut ledc, 0, (1 << 1) | (1 << 21));
        ledc.write(0x180 + ch * 4, 1);
        ledc.write(ch * 0x14 + 8, 7 << 4);
        ledc.write(ch * 0x14 + 12, 1 << 31);
        ledc.write(ch * 0x14, 4 | (1 << 4));
        assert_eq!(ledc.next_deadline(), None);
        ledc.tick(256);
        assert_eq!(ledc.read(ch * 0x14 + 16), 7 << 4);
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
    fn idle_clock_and_large_batches_match_exact_phase() {
        let mut ledc = Ledc::new(LedcLayout::C6);
        assert_eq!(ledc.clock(), None);
        ledc.external_clock_hz = 17_500_000;
        ledc.write(0xa0, 20 | (0x3ffff << 5) | (1 << 26));
        assert_eq!(ledc.clock(), Some(ClockDomain::Apb));
        let modulus = (APB_HZ as u128 / 256) * 0x3ffff * (1 << 20);
        let mut phase = 0;
        for ticks in [1, 1000, u32::MAX as u64, u64::MAX, 1] {
            phase = (phase + ticks as u128 * 17_500_000) % modulus;
            ledc.tick(ticks);
            assert_eq!(ledc.phase[0] as u128, phase);
        }
        ledc.write(0xa0, 1 << 23);
        assert_eq!(ledc.clock(), None);
    }

    /// A guest that shrinks a timer under a running fade leaves its phase past the new period: the
    /// next wrap is one tick away, not an underflow.
    #[test]
    fn shrinking_a_timer_under_a_fade_wraps_at_the_next_tick() {
        let mut ledc = Ledc::new(LedcLayout::C3);
        ledc.write(0xd0, 1);
        ledc.write(0xa0, 14 | (256 << 4) | (1 << 25));           // 14-bit, divider 1
        ledc.write(0xc8, 1 << 4);
        ledc.write(12, (1 << 31) | (100 << 20) | 1);
        ledc.write(0, 4 | (1 << 4));
        ledc.tick(10_000);
        ledc.write(0xa0, 1 | (256 << 4) | (1 << 25));            // 1-bit: a period of 2 ticks
        assert_eq!(ledc.next_deadline(), Some(1));
        ledc.tick(1);
        assert!(ledc.read(0xa4) < 2, "the counter is back inside the period");
    }

    /// CONF1's single range (S3/C3): DUTY_NUM 4, DUTY_CYCLE 0 (one period), DUTY_SCALE 1, down.
    #[test]
    fn conf1_fade_steps_each_wrap_and_ends_with_the_interrupt() {
        for batch in [false, true] {
            let mut ledc = Ledc::new(LedcLayout::C3);
            ledc.write(0xd0, 1);
            ledc.write(0xa0, 8 | (256 << 4) | (1 << 25));
            ledc.write(8, 128 << 4);
            ledc.write(12, (1 << 31) | (4 << 20) | 1);
            ledc.write(0, 4 | (1 << 4));
            let mut gpio = Gpio::new();
            gpio.enable = 1;
            gpio.func_out_sel[0] = 45;
            if batch { ledc.tick(80000); } else {
                ledc.tick(256);
                assert_eq!(ledc.read(16), 128 << 4);
                for step in 1..4 { ledc.tick(256); assert_eq!(ledc.read(16), (128 - step) << 4); }
                assert_eq!((ledc.read(0xc0) & (1 << 4), ledc.read(12) >> 31), (0, 1));
                ledc.tick(256);
            }
            assert_eq!(ledc.read(16), 124 << 4);
            assert_eq!(ledc.output(&gpio, 0).unwrap().1, (124 * 65535 + 128) / 256);
            assert_eq!(ledc.read(0xc0) & (1 << 4), 1 << 4);
            assert_eq!(ledc.read(12) & (1 << 31), 0);
        }
    }

    /// One configured timer `n` (8-bit, divider 1.0, so it wraps every 256 ticks) with channel
    /// `n + 1` waiting for a duty update on it; every other timer is unconfigured.
    fn check_each_timer_alone<const C: bool>(make: impl Fn() -> Ledc<C>, shift: u32)
    where Ledc<C>: Device {
        for n in 0..4u32 {
            let mut ledc = make();
            let conf = 8 | (256 << (4 + shift));
            ledc.write(0xa0 + n * 8, conf | (1 << (25 + shift)));
            let ch = n + 1;
            ledc.write(ch * 0x14 + 8, 77 << 4);
            ledc.write(ch * 0x14 + 12, 1 << 31);
            ledc.write(ch * 0x14, n | 4 | (1 << 4));
            let counter = |l: &mut Ledc<C>, t: u32| l.read(0xa4 + t * 8);
            ledc.tick(255);
            assert_eq!(counter(&mut ledc, n), 255, "timer {n}");
            assert_eq!(ledc.read(0xc0), 0, "timer {n}");
            assert_eq!(ledc.read(ch * 0x14 + 0x10), 0, "timer {n}");
            ledc.tick(1);
            assert_eq!(counter(&mut ledc, n), 0, "timer {n}");
            assert_eq!(ledc.read(0xc0), (1 << n) | (1 << (4 + ch)), "timer {n}");
            assert_eq!(ledc.read(ch * 0x14 + 0x10), 77 << 4, "timer {n}");
            assert_eq!(ledc.read(ch * 0x14 + 12) >> 31, 0, "timer {n}");
            ledc.tick(256 * 3 + 40);
            assert_eq!(counter(&mut ledc, n), 40, "timer {n}");
            for t in (0..4).filter(|&t| t != n) { assert_eq!(ledc.phase[t as usize], 0, "timer {t} beside {n}"); }
            // Paused, the timer stops; resumed, it counts on from where it stopped.
            ledc.write(0xa0 + n * 8, conf | (1 << (22 + shift)));
            assert_eq!(ledc.clock(), None, "timer {n}");
            ledc.tick(100);
            assert_eq!(counter(&mut ledc, n), 40, "timer {n}");
            ledc.write(0xa0 + n * 8, conf);
            ledc.tick(10);
            assert_eq!(counter(&mut ledc, n), 50, "timer {n}");
            // A second timer with twice the period counts independently.
            let other = 3 - n;
            ledc.write(0xa0 + other * 8, 9 | (256 << (4 + shift)) | (1 << (25 + shift)));
            ledc.write(0xcc, u32::MAX);
            ledc.tick(300);
            assert_eq!(counter(&mut ledc, n), (50 + 300) % 256, "timer {n}");
            assert_eq!(counter(&mut ledc, other), 300, "timer {other} beside {n}");
            assert_eq!(ledc.read(0xc0), 1 << n, "timer {n}");
            ledc.tick(212);
            assert_eq!(counter(&mut ledc, other), 0, "timer {other} beside {n}");
            assert_eq!(ledc.read(0xc0), (1 << n) | (1 << other), "timer {n}");
        }
    }

    #[test]
    fn each_timer_alone_counts_wraps_and_latches_its_channel() {
        for (layout, shift) in [(LedcLayout::S3, 0), (LedcLayout::C3, 0), (LedcLayout::C6, 1)] {
            check_each_timer_alone(|| {
                let mut ledc = Ledc::new(layout);
                if matches!(layout, LedcLayout::C6) { ledc.external_clock_hz = APB_HZ; } else { ledc.write(0xd0, 1); }
                ledc
            }, shift);
        }
        check_each_timer_alone(|| { let mut ledc = Ledc::new_classic(); ledc.write(0xd0, 1); ledc }, 1);
    }

    #[test]
    fn a_tick_that_ends_exactly_on_the_period_wraps() {
        let mut ledc = Ledc::new(LedcLayout::C3);
        ledc.write(0xd0, 1);
        ledc.write(0xa8, 8 | (256 << 4) | (1 << 25));
        ledc.tick(256);
        assert_eq!((ledc.phase[1], ledc.read(0xc0)), (0, 2));
        ledc.tick(255);
        assert_eq!((ledc.phase[1], ledc.read(0xc0)), (255 * APB_HZ, 2));
        ledc.write(0xcc, 2);
        ledc.tick(1);
        assert_eq!((ledc.phase[1], ledc.read(0xc0)), (0, 2));
    }

}
