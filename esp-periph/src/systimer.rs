//! System timer: two 52-bit units at 16 MHz, three comparators (esp_timer and the FreeRTOS tick).
use crate::device::{Device, WriteEffect};
use emu_core::ClockDomain;

// ------------------------------------------------------------------ System timer (16 MHz, 2 units, 3 comparators)
pub struct Systimer {
    /// one-shot alarm armed by COMPx_LOAD; fires as soon as the unit counter >= target (the S3
    /// compensates missed alarms: esp_timer arms targets already in the past to fire "now")
    pub armed: [bool; 3],
    pub conf: u32,
    pub unit: [u64; 2], pub unit_latch: [u64; 2], pub load: [u64; 2],
    pub target: [u64; 3], pub target_conf: [u32; 3], pub period_start: [u64; 3],
    pub int_ena: u32, pub int_raw: u32,
    ticks_acc: u64,
}
impl Systimer {
    /// CONF resets to 0x4600_0000: TIMER_UNIT0_WORK_EN and unit 1's two stall enables set (IDF
    /// v5.5.4 components/soc/esp32s3/register/soc/systimer_reg.h; the C3's, C6's and P4's are the same).
    /// esp-hal never writes it: `Instant::now` reads unit 0, counting from power-on.
    pub fn new() -> Self { Systimer { conf: 0x4600_0000, unit: [0; 2], unit_latch: [0; 2], load: [0; 2], target: [0; 3], target_conf: [0; 3], period_start: [0; 3], int_ena: 0, int_raw: 0, ticks_acc: 0, armed: [false; 3] } }
    pub fn tick(&mut self, ticks: u64) {
        for u in 0..2 {
            if self.conf & (1 << (30 - u as u32)) != 0 { self.unit[u] = (self.unit[u] + ticks) & ((1u64 << 52) - 1); }
        }
        for t in 0..3 {
            if self.conf & (1 << (24 - t as u32)) == 0 { continue; }   // TARGETt_WORK_EN: bits 24, 23, 22 (same header)
            let unit = ((self.target_conf[t] >> 31) & 1) as usize;
            let now = self.unit[unit];
            if self.target_conf[t] & (1 << 30) != 0 {
                let period = (self.target_conf[t] & 0x03ff_ffff) as u64;
                if period > 0 && now.wrapping_sub(self.period_start[t]) >= period {
                    // one alarm per elapsed period; re-align so a lagging start can't fire on every call
                    let elapsed = now.wrapping_sub(self.period_start[t]);
                    self.period_start[t] = now - (elapsed % period);
                    self.int_raw |= 1 << t;
                }
            } else if self.armed[t] && now >= self.target[t] {
                self.armed[t] = false;
                self.int_raw |= 1 << t;
            }
        }
        let _ = &mut self.ticks_acc;
    }
    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            0x0 => self.conf,
            0x4 | 0x8 => 1 << 29,
            0xc => (self.load[0] >> 32) as u32, 0x10 => self.load[0] as u32,
            0x14 => (self.load[1] >> 32) as u32, 0x18 => self.load[1] as u32,
            0x1c => (self.target[0] >> 32) as u32, 0x20 => self.target[0] as u32,
            0x24 => (self.target[1] >> 32) as u32, 0x28 => self.target[1] as u32,
            0x2c => (self.target[2] >> 32) as u32, 0x30 => self.target[2] as u32,
            0x34 => self.target_conf[0], 0x38 => self.target_conf[1], 0x3c => self.target_conf[2],
            0x40 => (self.unit_latch[0] >> 32) as u32, 0x44 => self.unit_latch[0] as u32,
            0x48 => (self.unit_latch[1] >> 32) as u32, 0x4c => self.unit_latch[1] as u32,
            0x64 => self.int_ena, 0x68 => self.int_raw, 0x70 => self.int_raw & self.int_ena,
            0x74 => self.target[0] as u32, 0x78 => (self.target[0] >> 32) as u32,
            0x7c => self.target[1] as u32, 0x80 => (self.target[1] >> 32) as u32,
            0x84 => self.target[2] as u32, 0x88 => (self.target[2] >> 32) as u32,
            0xfc => 0x2006171,
            _ => 0,
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x0 => self.conf = v,
            0x4 => if v & (1 << 30) != 0 { self.unit_latch[0] = self.unit[0]; },
            0x8 => if v & (1 << 30) != 0 { self.unit_latch[1] = self.unit[1]; },
            0xc => self.load[0] = (self.load[0] & 0xffff_ffff) | ((v as u64 & 0xfffff) << 32), 0x10 => self.load[0] = (self.load[0] & !0xffff_ffff) | v as u64,
            0x14 => self.load[1] = (self.load[1] & 0xffff_ffff) | ((v as u64 & 0xfffff) << 32), 0x18 => self.load[1] = (self.load[1] & !0xffff_ffff) | v as u64,
            0x1c => self.target[0] = (self.target[0] & 0xffff_ffff) | ((v as u64 & 0xfffff) << 32), 0x20 => self.target[0] = (self.target[0] & !0xffff_ffff) | v as u64,
            0x24 => self.target[1] = (self.target[1] & 0xffff_ffff) | ((v as u64 & 0xfffff) << 32), 0x28 => self.target[1] = (self.target[1] & !0xffff_ffff) | v as u64,
            0x2c => self.target[2] = (self.target[2] & 0xffff_ffff) | ((v as u64 & 0xfffff) << 32), 0x30 => self.target[2] = (self.target[2] & !0xffff_ffff) | v as u64,
            0x34 => self.target_conf[0] = v, 0x38 => self.target_conf[1] = v, 0x3c => self.target_conf[2] = v,
            0x50 | 0x54 | 0x58 => { let t = ((off - 0x50) / 4) as usize; let unit = ((self.target_conf[t] >> 31) & 1) as usize; self.period_start[t] = self.unit[unit]; self.armed[t] = true; }
            0x5c => self.unit[0] = self.load[0], 0x60 => self.unit[1] = self.load[1],
            0x64 => self.int_ena = v & 7, 0x6c => self.int_raw &= !v,
            _ => {}
        }
    }
    pub fn irq(&self, t: usize) -> bool { self.int_raw & self.int_ena & (1 << t) != 0 }
}

impl Device for Systimer {
    fn read(&mut self, off: u32) -> u32 { Systimer::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Systimer::write(self, off, v); WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { (0..3).fold(0, |m, t| m | ((self.irq(t) as u64) << t)) }
    fn clock(&self) -> Option<ClockDomain> { Some(ClockDomain::Systimer) }
    fn tick(&mut self, ticks: u64) { Systimer::tick(self, ticks) }
    /// Systimer ticks until the earliest armed comparator fires.
    fn has_deadline(&self) -> bool { true }
    #[allow(clippy::implicit_saturating_sub)] // The explicit overdue branch documents that missed alarms fire now.
    fn next_deadline(&self) -> Option<u64> {
        let mut best: Option<u64> = None;
        for t in 0..3 {
            if self.conf & (1 << (24 - t as u32)) == 0 { continue; }
            let unit = ((self.target_conf[t] >> 31) & 1) as usize;
            if self.conf & (1 << (30 - unit as u32)) == 0 { continue; }              // unit stopped: never fires
            let now = self.unit[unit];
            let ticks = if self.target_conf[t] & (1 << 30) != 0 {
                let period = (self.target_conf[t] & 0x03ff_ffff) as u64;
                if period == 0 { continue; }
                let elapsed = now.wrapping_sub(self.period_start[t]);
                if elapsed >= period { 0 } else { period - elapsed }
            } else if self.armed[t] { self.target[t].saturating_sub(now) } else { continue };
            best = Some(best.map_or(ticks, |b| b.min(ticks)));
        }
        best
    }
}

impl Default for Systimer { fn default() -> Self { Self::new() } }
