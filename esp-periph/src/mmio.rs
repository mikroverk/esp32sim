//! The table a chip fills in once per peripheral, and everything derived from it: address
//! dispatch, the interrupt-source status word, clock-tick delivery, the next timer deadline,
//! unknown-register logging. `device_set!` turns the table into straight-line code with static
//! calls, so a peripheral costs what a hand-written match arm did; adding one is one line.
use crate::device::WriteEffect;
use crate::regram::RegRam;
use std::collections::{HashMap, HashSet};

/// A local source the chip does not route anywhere.
pub const NO_SOURCE: usize = usize::MAX;

/// What the chip writes by hand around its `device_set!` table.
pub trait DeviceSet: Sized + 'static {
    /// Start of the peripheral address space (block 0).
    const BASE: u32;
    fn block_name(block: u32) -> &'static str;
    fn misc(&self) -> &Misc;
    fn misc_mut(&mut self) -> &mut Misc;
    /// Called before a register access reaches its device: the place for the few registers whose
    /// value depends on another device (an interrupt status read, a timestamp).
    fn pre_access(&mut self, _block: u32, _off: u32, _write: bool) {}
}

/// What `device_set!` generates from the table.
pub trait Dispatch {
    fn dispatch_read(&mut self, block: u32, off: u32) -> Option<u32>;
    fn dispatch_write(&mut self, block: u32, off: u32, v: u32) -> Option<WriteEffect>;
    /// Which chip interrupt sources are asserted right now, one bit per source number.
    fn source_status(&self) -> [u32; 4];
    /// Advance device time by `cycles` CPU cycles: every clocked device receives the ticks its
    /// domain gained, domains in the clock table's order, devices in the device table's order.
    /// Advance clocked devices and report whether any device interrupt source changed.
    fn tick(&mut self, cycles: u64) -> bool;
    /// CPU cycles until the earliest timer deadline (conservative by one device tick), or
    /// `u32::MAX` when nothing is armed.
    fn cycles_until_deadline(&self) -> u32;
    /// (block, name) of every modelled device, for tools and docs.
    fn devices() -> &'static [(u32, &'static str)];
    /// Switch debug output on for every device whose name starts with `area` (case-insensitive).
    fn debug(&mut self, area: &str, on: bool);
}

/// Per-chip dispatch state that is not a device: the register RAM behind unmodelled blocks,
/// first-touch logging, pc attribution.
pub struct Misc {
    pub generic: HashMap<u32, RegRam>,
    seen: HashSet<(u32, bool)>,
    pub log_unknown: bool,
    pub log_all: bool,
    /// pc of the instruction making the access (`Bus::note_pc`)
    pub cur_pc: u32,
    /// (pc, addr, value, write) of every access, while an observer wants them
    pub mmio_log: Option<Vec<(u32, u32, u32, bool)>>,
}
impl Misc {
    pub fn new() -> Misc { Misc { generic: HashMap::new(), seen: HashSet::new(), log_unknown: false, log_all: false, cur_pc: 0, mmio_log: None } }
}
impl Default for Misc { fn default() -> Self { Self::new() } }

fn note<P: DeviceSet>(p: &mut P, addr: u32, block: u32, off: u32, write: bool, v: u32) {
    let m = p.misc_mut();
    if !m.log_unknown { return; }
    if m.seen.insert((addr & !3, write)) {
        eprintln!("[periph] {} {}+0x{:03x} ({:#010x}) {} pc={:#010x}", if write { "W" } else { "R" }, P::block_name(block), off, addr, if write { format!("= {:#x}", v) } else { String::new() }, m.cur_pc);
    }
}

#[inline]
pub fn read32<P: DeviceSet + Dispatch>(p: &mut P, addr: u32) -> u32 {
    let (block, off) = (addr.wrapping_sub(P::BASE) >> 12, addr & 0xfff);
    p.pre_access(block, off, false);
    let v = match p.dispatch_read(block, off) {
        Some(v) => v,
        None => { note(p, addr, block, off, false, 0); p.misc_mut().generic.entry(block).or_default().read(off) }
    };
    let m = p.misc_mut();
    if m.log_all { eprintln!("[rd] {}+0x{:03x} ({:#010x}) -> {:#010x} pc={:#010x}", P::block_name(block), off, addr, v, m.cur_pc); }
    if let Some(l) = &mut m.mmio_log { l.push((m.cur_pc, addr, v, false)); }
    v
}

#[inline]
pub fn write32<P: DeviceSet + Dispatch>(p: &mut P, addr: u32, v: u32) -> WriteEffect {
    let (block, off) = (addr.wrapping_sub(P::BASE) >> 12, addr & 0xfff);
    let m = p.misc_mut();
    if m.log_all { eprintln!("[wr] {}+0x{:03x} ({:#010x}) <- {:#010x} pc={:#010x}", P::block_name(block), off, addr, v, m.cur_pc); }
    if let Some(l) = &mut m.mmio_log { l.push((m.cur_pc, addr, v, true)); }
    p.pre_access(block, off, true);
    match p.dispatch_write(block, off, v) {
        Some(fx) => fx,
        None => { note(p, addr, block, off, true, v); p.misc_mut().generic.entry(block).or_default().write(off, v); WriteEffect::NONE }
    }
}

/// The peripheral table of a chip. One line per device:
///
/// ```text
/// device_set! { Peripherals; clock: (clock) CPU_HZ, [(ClockDomain::Apb, 3), ...];
///     0x00 "UART0" (uart[0]) => [SRC_UART0];                    // block, name, field, chip source numbers
///     0x08 "EFUSE" (efuse) delta -0x800 @ 0x800..=0xfff => [];   // mounted at an offset, only that range
///     0x34 "WIFI_MAC" alias (wifi) delta 0x1000 => [];           // a further block of a device listed above
/// }
/// ```
///
/// `clock` names the `ClockTree` field and the divider table (CPU cycles per tick per domain).
/// Entries are tried in order, so a range-limited entry goes before the full-block one behind it.
/// An `alias` entry only dispatches: its device already ticks and reports sources once.
/// Optional `inline always;` keeps clock/source operations inline when optional service paths add callers.
#[macro_export]
macro_rules! device_set {
    ($P:ident; $(inline $hint:ident;)? clock: ($clk:ident) $cpu_hz:expr, [$(($dom:expr, $div:expr)),* $(,)?];
     $( $block:literal $name:literal $($alias:ident)? ($($f:tt)+) $(delta $d:literal)? $(@ $lo:literal ..= $hi:literal)? => [$($src:expr),* $(,)?]; )* ) => {
        impl $P {
            pub const CLOCKS: $crate::__Dividers<{ $crate::__count!($($dom)*) }> = [$(($dom, $div)),*];
            pub const fn new_clock() -> $crate::__ClockTree<{ $crate::__count!($($dom)*) }> { $crate::__ClockTree::new($cpu_hz) }
        }
        impl $crate::mmio::Dispatch for $P {
            #[inline]
            fn dispatch_read(&mut self, block: u32, off: u32) -> Option<u32> {
                match block {
                    $( $block $(if ($lo..=$hi).contains(&off))? => Some($crate::Device::read(&mut self.$($f)+, off $(.wrapping_add(($d as i32) as u32))?)), )*
                    _ => None,
                }
            }
            #[inline]
            fn dispatch_write(&mut self, block: u32, off: u32, v: u32) -> Option<$crate::WriteEffect> {
                match block {
                    $( $block $(if ($lo..=$hi).contains(&off))? => Some($crate::Device::write(&mut self.$($f)+, off $(.wrapping_add(($d as i32) as u32))?, v)), )*
                    _ => None,
                }
            }
            #[inline$(($hint))?]
            fn source_status(&self) -> [u32; 4] {
                let mut st = [0u32; 4];
                $( if !(false $(|| stringify!($alias) == "alias")?) {
                    let bits = $crate::Device::irq_sources(&self.$($f)+);
                    if bits != 0 { let mut b = 0u32; $( if $src != $crate::NO_SOURCE && bits & (1u64 << b) != 0 { st[$src / 32] |= 1 << ($src % 32); } b += 1; )* let _ = b; }
                } )*
                st
            }
            #[inline$(($hint))?]
            fn tick(&mut self, cycles: u64) -> bool {
                let mut irq_changed = false;
                let mut deltas = [($crate::__ClockDomain::Cpu, 0u64); 8]; let mut n = 0usize;
                self.$clk.advance(&Self::CLOCKS, cycles, |d, t| { if n < 8 { deltas[n] = (d, t); n += 1; } });
                for &(d, t) in &deltas[..n] {
                    $( if !(false $(|| stringify!($alias) == "alias")?) && $crate::Device::clock(&self.$($f)+) == Some(d) {
                        // Only clocked devices can change here. Do not scan unclocked
                        // GPIO/GDMA sources on every short MMIO-driven time flush.
                        let before = $crate::Device::irq_sources(&self.$($f)+);
                        $crate::Device::tick(&mut self.$($f)+, t);
                        irq_changed |= before != $crate::Device::irq_sources(&self.$($f)+);
                    } )*
                }
                irq_changed
            }
            #[inline$(($hint))?]
            fn cycles_until_deadline(&self) -> u32 {
                let mut best = u64::MAX;
                $( if !(false $(|| stringify!($alias) == "alias")?) && $crate::Device::has_deadline(&self.$($f)+) {
                    if let Some(t) = $crate::Device::next_deadline(&self.$($f)+) {
                        let div = $crate::__divider(&Self::CLOCKS, match $crate::Device::clock(&self.$($f)+) { Some(c) => c, None => $crate::__ClockDomain::Cpu });
                        best = best.min(t.saturating_sub(1) * div);
                    }
                } )*
                best.min(u32::MAX as u64) as u32
            }
            fn devices() -> &'static [(u32, &'static str)] { &[$(($block, $name)),*] }
            fn debug(&mut self, area: &str, on: bool) {
                let area = area.to_ascii_lowercase();
                $( if $name.to_ascii_lowercase().starts_with(&area) { $crate::Device::debug(&mut self.$($f)+, on); } )*
            }
        }
    };
}
#[doc(hidden)] #[macro_export]
macro_rules! __count { () => { 0usize }; ($x:tt $($rest:tt)*) => { 1usize + $crate::__count!($($rest)*) }; }
#[doc(hidden)] pub use emu_core::{ClockDomain as __ClockDomain, ClockTree as __ClockTree, Dividers as __Dividers};
#[doc(hidden)] pub use emu_core::clock::divider as __divider;
