//! GPIO matrix: output/enable/input words, per-pin config and interrupt types, the function
//! selection tables, and the edge queues the SoC hands to its board and pulse counters.
use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;

// ------------------------------------------------------------------ GPIO
pub struct Gpio {
    pub out: u64, pub enable: u64, pub input: u64, pub status: u64, pub pin: [u32; 49],
    pub func_in_sel: [u32; 256], pub func_out_sel: [u32; 49], ram: RegRam,
    pub input_changes: Vec<(u8, bool)>,
    /// (pin, level) changes of enabled outputs since last drain
    pub changes: Vec<(u8, bool)>,
    pub strap: u32,
    pull_up: u64, pull_down: u64,
    external_mask: u64, external_levels: u64,
}
impl Gpio {
    pub fn new() -> Self { Gpio { out: 0, enable: 0, input: (1u64 << 49) - 1, status: 0, pin: [0; 49], func_in_sel: [0x3c; 256], func_out_sel: [0x100; 49], ram: RegRam::new(), changes: Vec::new(), strap: 0x0f, input_changes: Vec::new(), pull_up: 0, pull_down: 0, external_mask: 0, external_levels: 0 } }
    /// Report every pin whose driven level changed: `out & enable` before against after. Both
    /// words matter — a driver that toggles the output *enable* to produce a level (IDF 5.5's
    /// esp_lcd releases the D/C line after each colour transfer and re-enables it before the
    /// next) changes what a board sees just as much as one that toggles `out`.
    fn note_out(&mut self, old_out: u64, old_enable: u64) {
        let vis = self.out & self.enable; let oldvis = old_out & old_enable;
        let mut diff = (vis ^ oldvis) & ((1u64 << 49) - 1);
        while diff != 0 {
            let p = diff.trailing_zeros() as u8;
            self.changes.push((p, vis & (1u64 << p) != 0));
            diff &= diff - 1;
        }
    }
    pub fn set_input(&mut self, pin: u8, level: bool) -> bool {
        if pin >= 49 { return false; }
        let mask = 1u64 << pin;
        self.external_mask |= mask;
        if level { self.external_levels |= mask; } else { self.external_levels &= !mask; }
        self.resolve(mask)
    }
    /// Stop driving a pin externally; resolve it from output enable and IO_MUX pulls.
    pub fn release_input(&mut self, pin: u8) -> bool {
        if pin >= 49 { return false; }
        let mask = 1u64 << pin;
        self.external_mask &= !mask;
        self.resolve(mask)
    }
    pub fn set_pulls(&mut self, pin: u8, up: bool, down: bool) {
        if pin >= 49 { return; }
        let mask = 1u64 << pin;
        self.pull_up = (self.pull_up & !mask) | if up { mask } else { 0 };
        self.pull_down = (self.pull_down & !mask) | if down { mask } else { 0 };
        self.resolve(mask);
    }
    /// Preserve host drives across a chip reset, without retaining pad configuration or edges.
    pub fn restore_external(&mut self, old: &Self) {
        self.external_mask = old.external_mask;
        self.external_levels = old.external_levels;
        self.resolve(self.external_mask);
        self.input_changes.clear();
    }
    // INT_TYPE/INT_ENA: IDF v5.5.4 components/soc/esp32s3/register/soc/gpio_reg.h:278-301.
    // C3/C6 use the same fields; see docs/evidence/gpio-input-feedback/README.md.
    fn resolve(&mut self, mask: u64) -> bool {
        let mask = mask & ((1u64 << 49) - 1);
        // Host-driven levels are ideal digital sources; opposing output drivers do not
        // model electrical contention. Floating and simultaneous pulls retain HIGH.
        let passive = self.pull_up | !self.pull_down;
        let driven = (self.out & self.enable) | (passive & !self.enable);
        let levels = (self.external_levels & self.external_mask) | (driven & !self.external_mask);
        let mut changed = (self.input ^ levels) & mask;
        self.input = (self.input & !mask) | (levels & mask);
        let mut irq = false;
        while changed != 0 {
            let pin = changed.trailing_zeros() as u8;
            let bit = 1u64 << pin;
            let level = self.input & bit != 0;
            self.input_changes.push((pin, level));
            let typ = (self.pin[pin as usize] >> 7) & 7;
            if (level && typ == 1) || (!level && typ == 2) || typ == 3 {
                self.status |= bit;
                irq = true;
            }
            changed &= changed - 1;
        }
        irq
    }
    /// The pin the matrix routes peripheral output signal `sig` to, if any.
    pub fn pin_for_signal(&self, sig: u32) -> Option<u8> {
        self.func_out_sel.iter().position(|&s| s & 0x1ff == sig).map(|p| p as u8)
    }
    pub fn level(&self, pin: u8) -> bool {
        if pin as usize >= self.pin.len() { return false; }
        self.input & (1u64 << pin) != 0
    }
    pub fn irq(&self) -> bool {
        // level-type interrupts on current input, plus latched edge status, gated by INT_ENA (bits 13..17, bit 13 = core0)
        (0..49u8).any(|p| { let cfg = self.pin[p as usize]; let ena = (cfg >> 13) & 1 != 0; let typ = (cfg >> 7) & 7;
            ena && ((self.status & (1u64 << p) != 0) || (typ == 4 && !self.level(p)) || (typ == 5 && self.level(p))) })
    }
    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            0x4 => self.out as u32, 0x10 => (self.out >> 32) as u32,
            0x20 => self.enable as u32, 0x2c => (self.enable >> 32) as u32,
            0x38 => self.strap,
            0x3c => self.input as u32, 0x40 => (self.input >> 32) as u32,
            0x44 => self.status as u32, 0x50 => (self.status >> 32) as u32,
            0x5c => self.status as u32, 0x68 => (self.status >> 32) as u32,     // PCPU_INT: interrupt status seen by core 0
            0x74..=0x134 => self.pin[((off - 0x74) / 4) as usize],
            0x154..=0x550 => self.func_in_sel[((off - 0x154) / 4) as usize],
            0x554..=0x614 => self.func_out_sel[((off - 0x554) / 4) as usize],
            0x6fc => 0x2006130,
            _ => self.ram.read(off),
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        let (old, old_enable) = (self.out, self.enable);
        match off {
            0x4 => self.out = (self.out & !0xffff_ffff) | v as u64,
            0x8 => self.out |= v as u64,
            0xc => self.out &= !(v as u64),
            0x10 => self.out = (self.out & 0xffff_ffff) | ((v as u64 & 0x1ffff) << 32),
            0x14 => self.out |= (v as u64) << 32,
            0x18 => self.out &= !((v as u64) << 32),
            0x20 => { self.enable = (self.enable & !0xffff_ffff) | v as u64; }
            0x24 => self.enable |= v as u64,
            0x28 => self.enable &= !(v as u64),
            0x2c => self.enable = (self.enable & 0xffff_ffff) | ((v as u64 & 0x1ffff) << 32),
            0x30 => self.enable |= (v as u64) << 32,
            0x34 => self.enable &= !((v as u64) << 32),
            0x44 => self.status = (self.status & !0xffff_ffff) | v as u64,
            0x48 => self.status |= v as u64,
            0x4c => self.status &= !(v as u64),
            0x50 => self.status = (self.status & 0xffff_ffff) | ((v as u64) << 32),
            0x54 => self.status |= (v as u64) << 32,
            0x58 => self.status &= !((v as u64) << 32),
            0x74..=0x134 => self.pin[((off - 0x74) / 4) as usize] = v,
            0x154..=0x550 => self.func_in_sel[((off - 0x154) / 4) as usize] = v,
            0x554..=0x614 => self.func_out_sel[((off - 0x554) / 4) as usize] = v,
            _ => self.ram.write(off, v),
        }
        // enable changes also change what's visible on pins
        if matches!(off, 0x4 | 0x8 | 0xc | 0x10 | 0x14 | 0x18 | 0x20 | 0x24 | 0x28 | 0x2c | 0x30 | 0x34) { self.note_out(old, old_enable); self.resolve((old ^ self.out) | (old_enable ^ self.enable)); }
    }
}

impl Default for Gpio { fn default() -> Self { Self::new() } }

impl Device for Gpio {
    fn read(&mut self, off: u32) -> u32 { Gpio::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Gpio::write(self, off, v); WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { self.irq() as u64 }
}
