//! Timer group: two 54-bit timers on APB with prescaler and alarm, plus the RTC calibration register.
use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;
use crate::{RTC_SLOW_HZ, XTAL_HZ};
use emu_core::ClockDomain;

// ------------------------------------------------------------------ Timer group (T0/T1 + WDT + RTC calibration)
pub struct TimerGroup {
    ram: RegRam,
    pub t: [Timer; 2],
    pub int_raw: u32, pub int_ena: u32,
}
#[derive(Default, Clone, Copy)]
pub struct Timer { pub config: u32, pub count: u64, pub latch: u64, pub alarm: u64, pub load: u64, pub prescale_acc: u64 }
impl Timer {
    pub fn read(&self, o: u32) -> u32 {
        match o { 0x0 => self.config, 0x4 => self.latch as u32, 0x8 => (self.latch >> 32) as u32, 0x10 => self.alarm as u32, 0x14 => (self.alarm >> 32) as u32, 0x18 => self.load as u32, 0x1c => (self.load >> 32) as u32, _ => 0 }
    }
    pub fn write(&mut self, o: u32, v: u32, width_mask: u64) {
        match o {
            0x0 => self.config = v,
            0xc => self.latch = self.count,
            0x10 => self.alarm = (self.alarm & !0xffff_ffff) | v as u64, 0x14 => self.alarm = (self.alarm & 0xffff_ffff) | ((v as u64 & (width_mask >> 32)) << 32),
            0x18 => self.load = (self.load & !0xffff_ffff) | v as u64, 0x1c => self.load = (self.load & 0xffff_ffff) | ((v as u64 & (width_mask >> 32)) << 32),
            0x20 => self.count = self.load,
            _ => {}
        }
    }
    /// Advances by `apb_ticks`, calling `on_alarm` when the alarm fires. Inlined so each caller keeps
    /// its own constant `width_mask` (54 bits on S3/C3/C6, 64 on the classic ESP32).
    #[inline(always)]
    pub fn step(&mut self, apb_ticks: u64, width_mask: u64, mut on_alarm: impl FnMut()) {
        let t = self;
        if t.config & (1 << 31) == 0 { return; }   // TIMG_T0_EN
        let div = ((t.config >> 13) & 0xffff) as u64;
        let div = if div == 0 { 65536 } else { div };
        t.prescale_acc += apb_ticks;
        let mut steps = t.prescale_acc / div;
        t.prescale_acc %= div;
        if steps == 0 { return; }
        let inc = t.config & (1 << 30) != 0;   // TIMG_T0_INCREASE
        // Steps to the alarm from `c`, if counting reaches it (not already at or past it).
        let alarm = t.alarm;
        let gap = |c: u64| if inc { alarm.checked_sub(c) } else { c.checked_sub(alarm) }.filter(|&d| d > 0);
        if t.config & (1 << 10) != 0 {   // TIMG_T0_ALARM_EN
            if let Some(d) = gap(t.count).filter(|&d| d <= steps) {
                on_alarm();
                steps -= d;
                if t.config & (1 << 29) != 0 {   // autoreload
                    // q256: a tick can be longer than the reload period, so the steps after the
                    // alarm count from `load` and cross again every `gap(load)` steps.
                    t.count = t.load;
                    if let Some(p) = gap(t.load) { steps %= p; }
                } else { t.count = t.alarm; t.config &= !(1 << 10); }
            }
        }
        t.count = if inc { t.count.wrapping_add(steps) & width_mask } else { t.count.wrapping_sub(steps) & width_mask };
    }
    /// Counter steps to an armed alarm that counting will reach, and the prescaler divider.
    #[inline(always)]
    pub fn alarm_gap(&self) -> Option<(u64, u64)> {
        if self.config & (1 << 31) == 0 || self.config & (1 << 10) == 0 { return None; }   // TIMG_T0_EN, ALARM_EN
        let div = ((self.config >> 13) & 0xffff) as u64;
        let div = if div == 0 { 65536 } else { div };
        let steps = if self.config & (1 << 30) != 0 {
            if self.count >= self.alarm { return None } else { self.alarm - self.count }
        } else if self.count <= self.alarm { return None } else { self.count - self.alarm };
        Some((steps, div))
    }
    /// APB ticks to the alarm, saturating: 64-bit counters (classic ESP32) can be further away
    /// than `u64::MAX` APB ticks.
    pub fn deadline(&self) -> Option<u64> {
        self.alarm_gap().map(|(steps, div)| steps.saturating_mul(div).saturating_sub(self.prescale_acc))
    }
}
impl TimerGroup {
    pub fn new() -> Self { TimerGroup { ram: RegRam::new(), t: [Timer::default(); 2], int_raw: 0, int_ena: 0 } }
    pub fn tick(&mut self, apb_ticks: u64) {
        for i in 0..2 {
            self.t[i].step(apb_ticks, (1 << 54) - 1, || self.int_raw |= 1 << i);
        }
    }
    pub fn read(&mut self, off: u32) -> u32 {
        let (i, o) = if off < 0x24 { (0usize, off) } else if off < 0x48 { (1usize, off - 0x24) } else { (2, off) };
        if i < 2 {
            return self.t[i].read(o);
        }
        match off {
            0x68 => (self.ram.read(off) & !(1 << 15)) | (1 << 15),                       // RTCCALICFG: always RDY
            0x6c => { let n = (self.ram.read(0x68) >> 16) & 0x7fff; ((n as u64 * XTAL_HZ / RTC_SLOW_HZ) as u32) << 7 }   // RTCCALICFG1 value
            0x70 => self.int_ena, 0x74 => self.int_raw, 0x78 => self.int_raw & self.int_ena,
            0xf8 => 0x2006191,
            _ => self.ram.read(off),
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        let (i, o) = if off < 0x24 { (0usize, off) } else if off < 0x48 { (1usize, off - 0x24) } else { (2, off) };
        if i < 2 {
            self.t[i].write(o, v, (1 << 54) - 1);
            return;
        }
        match off {
            0x70 => self.int_ena = v, 0x7c => self.int_raw &= !v,
            _ => self.ram.write(off, v),
        }
    }
}
impl Default for TimerGroup { fn default() -> Self { Self::new() } }

impl Device for TimerGroup {
    fn read(&mut self, off: u32) -> u32 { TimerGroup::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { TimerGroup::write(self, off, v); WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { (self.int_raw & self.int_ena & 3) as u64 }
    fn clock(&self) -> Option<ClockDomain> { Some(ClockDomain::Apb) }
    fn tick(&mut self, apb_ticks: u64) { TimerGroup::tick(self, apb_ticks) }
    /// APB ticks until the earliest armed alarm.
    fn has_deadline(&self) -> bool { true }
    fn next_deadline(&self) -> Option<u64> {
        let mut best: Option<u64> = None;
        for t in &self.t {
            let Some((steps, div)) = t.alarm_gap() else { continue };
            let apb = (steps * div).saturating_sub(t.prescale_acc);
            best = Some(best.map_or(apb, |b| b.min(apb)));
        }
        best
    }
}
