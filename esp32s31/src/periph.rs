//! ESP32-S31 `device_set!` table (BASE 0), same shape as P4/C3.
//!
//! Shared esp-periph IP mounted here: UART0, Systimer (P4-style /2.5 wrap), USB-Serial/JTAG,
//! TimerGroup (with local MWDT stages for the WDT half), and S31 Efuse (ReadImage protocol —
//! shared `Efuse` is RegRam-only and cannot replace it).
//!
//! Still local outside this table (maps checked; not a shrug):
//! - Pads / GPIO / IO_MUX — shared `Gpio` S3 map mismatch (ENABLE@0x34, IN@0x64, PIN@0xf4,
//!   OUT_SEL@0xaf4, 62>57 pins) plus IO_MUX+PMU/HP hold electrical coupling; see `pads.rs`.
//! - Interrupt matrix — not `intmtx::Lines` (C3/C6 PLIC); S31 is CLIC + 169 sources / dual-core
//!   like S3/P4 local `IntMatrix`; see `interrupts.rs`. MWDT stages stay in `TimgS31` (shared
//!   `TimerGroup` has no stage WDT); PCR clock words stay on the bus (not Device blocks).

use emu_core::{ClockDomain, ClockTree};
use esp_periph::{
    device_set, mmio, Aes, Device, DeviceSet, Misc, WriteEffect, NO_SOURCE, Systimer,
    TimerGroup, Uart, UartLayout, UsbSerialJtag,
};
use emu_core::Fault;

use crate::efuse::Efuse;
use crate::mwdt::Mwdt;

/// The machine's clock: the 320 MHz CPLL clock an IDF app runs the S31 at (the Mosaico factory
/// firmware logs `CPU 320 MHz`). The 40 MHz domains divide it by 8.
pub const CPU_HZ: u64 = 320_000_000;

/// PCR clock words that gate IP in this set (HP_SYS_CLKRST).
pub const SYSTIMER_CLOCK: u32 = 0x2058_7120;
pub const USB_CLOCK: u32 = 0x2058_7144;
pub const TIMG0_CLOCK: u32 = 0x2058_7114;
pub const TIMG1_CLOCK: u32 = 0x2058_711c;

pub mod src {
    pub const USB_SERIAL_JTAG: usize = 2;
    pub const UART0: usize = 9;
    // IDF v6.1 soc/esp32s31/include/soc/interrupts.h: TIMERGRP0_T0, _T1, _WDT, then TIMERGRP1's.
    pub const TG0_T0: usize = 25;
    pub const TG0_T1: usize = 26;
    pub const TG0_WDT: usize = 27;
    pub const TG1_T0: usize = 28;
    pub const TG1_T1: usize = 29;
    pub const TG1_WDT: usize = 30;
    pub const SYSTIMER_TARGET0: usize = 33;
    pub const SYSTIMER_TARGET1: usize = 34;
    pub const SYSTIMER_TARGET2: usize = 35;
    // EFUSE_SOURCE = 152 is outside DeviceSet's 128-bit source_status; gathered via Irqs.
}

// ── Systimer: shared IP + S31 PCR gate + XTAL/2.5 (P4-style) ────────────────

#[derive(Default)]
pub struct SystimerS31 {
    pub st: Systimer,
    frac: u64,
    denominator: u64,
    pub clock: u32,
}

impl SystimerS31 {
    pub fn clock_on(&self) -> bool {
        self.clock & 3 == 1
    }

    fn counter_on(&self) -> bool {
        self.clock & 0x12 == 0x10
    }

    pub fn source_rc_fast(&self) -> bool {
        self.clock & 8 != 0
    }

    pub fn irqs(&self) -> [bool; 3] {
        std::array::from_fn(|t| self.st.irq(t))
    }

    pub fn clock_read(&self) -> u32 {
        self.clock
    }

    pub fn clock_write(&mut self, value: u32) -> Result<(), Fault> {
        if value & 4 != 0 {
            return Err(Fault::Prohibited);
        }
        if value & 2 != 0 {
            *self = Self { clock: value & 0x1b, ..Self::default() };
            return Ok(());
        }
        self.clock = value & 0x1b;
        Ok(())
    }

    pub fn advance_ticks(&mut self, ticks: u64, stalled: [bool; 2]) {
        if ticks == 0 || !self.counter_on() || stalled == [true, true] {
            return;
        }
        self.st.tick(ticks);
    }

    pub fn advance_cpu(&mut self, cycles: u64, cpu_hz: u64, rc_fast_hz: u64, stalled: [bool; 2]) {
        if cycles == 0 || !self.counter_on() || cpu_hz == 0 || stalled == [true, true] {
            return;
        }
        let source = if self.source_rc_fast() { rc_fast_hz } else { 40_000_000 };
        if source == 0 {
            return;
        }
        let denominator = cpu_hz.saturating_mul(5);
        if self.denominator != 0 && self.denominator != denominator {
            self.frac = self.frac * denominator / self.denominator;
        }
        self.denominator = denominator;
        let total = self.frac + cycles * source * 2;
        self.frac = total % denominator;
        let ticks = total / denominator;
        if ticks != 0 {
            self.st.tick(ticks);
        }
    }

    pub fn ticks_until_irq(&self, stalled: [bool; 2]) -> Option<u64> {
        if !self.counter_on() || stalled == [true, true] {
            return None;
        }
        if self.irqs().iter().any(|&b| b) {
            return Some(0);
        }
        self.st.next_deadline()
    }
}

impl Device for SystimerS31 {
    fn read(&mut self, off: u32) -> u32 {
        if !self.clock_on() {
            return 0;
        }
        self.st.read(off)
    }

    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        if !self.clock_on() {
            return WriteEffect::NONE;
        }
        // IDF port_systick: PERIOD_MODE after one-shot COMP_LOAD without a second load.
        if matches!(off, 0x34 | 0x38 | 0x3c) {
            let t = ((off - 0x34) / 4) as usize;
            let old = self.st.target_conf[t];
            self.st.write(off, v);
            if v & (1 << 30) != 0 && old & (1 << 30) == 0 && v & 0x03ff_ffff != 0 {
                self.st.write(0x50 + t as u32 * 4, 1);
            }
            return WriteEffect::NONE;
        }
        self.st.write(off, v);
        WriteEffect::NONE
    }

    fn irq_sources(&self) -> u64 {
        Device::irq_sources(&self.st)
    }

    fn clock(&self) -> Option<ClockDomain> {
        Some(ClockDomain::Systimer)
    }

    fn tick(&mut self, ticks: u64) {
        if self.counter_on() {
            self.st.tick(ticks);
        }
    }

    fn has_deadline(&self) -> bool {
        true
    }

    fn next_deadline(&self) -> Option<u64> {
        if !self.counter_on() {
            return None;
        }
        self.st.next_deadline()
    }
}

// ── TIMG: shared TimerGroup + local MWDT stages ─────────────────────────────

/// HP_SYS_CLKRST TIMERGRPn_CTRL0 (IDF v6.1 hp_sys_clkrst_reg.h): Tn_SRC_SEL at bits 4:3 and
/// 7:6, Tn_CLK_EN at bits 5 and 8, so timer `t`'s fields sit `3 * t` above T0's.
const T0_SRC_SEL_SHIFT: u32 = 3;
const T0_CLK_EN: u32 = 1 << 5;

#[derive(Default)]
pub struct TimgS31 {
    pub timers: TimerGroup,
    pub wdt: Mwdt,
    /// T0's and T1's source ticks owed, in 1/`cpu_hz` of a tick (`advance_cpu`).
    frac: [u64; 2],
}

impl TimgS31 {
    pub fn irq_wdt(&self) -> bool {
        self.wdt.irq()
    }

    /// Whether T0 or T1 counts (TnCONFIG.EN); `SocBus` keeps it for its rounds (`timg_counting`).
    #[inline]
    pub fn counting(&self) -> bool {
        (self.timers.t[0].config | self.timers.t[1].config) >> 31 != 0
    }

    /// TIMG_T0_INT and TIMG_T1_INT, as INT_ST_TIMERS bits 0 and 1.
    pub fn irq_timers(&self) -> u8 {
        (self.timers.int_raw & self.timers.int_ena & 3) as u8
    }

    /// The watchdog's XTAL ticks; T0 and T1 count from their own source in `advance_cpu`.
    pub fn advance_xtal(&mut self, ticks: u64) -> Result<(), Fault> {
        self.wdt.advance_xtal(ticks)
    }

    /// TIMERGRPn_CTRL0. RST_EN resets the whole group, so T0 and T1 with the watchdog.
    pub fn clock_write(&mut self, value: u32) -> Result<(), Fault> {
        self.wdt.clock_write(value)?;
        if value & 2 != 0 { self.timers = TimerGroup::new(); self.frac = [0; 2]; }
        Ok(())
    }

    /// The rate timer `t` counts at: 0 XTAL, 1 RC_FAST, 2 PLL_F80M, the order of IDF v6.1
    /// `_timer_ll_set_clock_source` (esp_hal_timg/esp32s31/include/hal/timer_ll.h; esp-hal 1.2.3
    /// `src/soc/esp32s31/clocks.rs` agrees), or 0 while Tn_CLK_EN is clear. PLL_F80M is the fixed
    /// 80 MHz tap that LEDC also uses here (`ledc.rs`).
    fn source_hz(&self, t: usize, rc_fast_hz: u64) -> u64 {
        let clock = self.wdt.clock_read(); let shift = 3 * t as u32;
        if clock & T0_CLK_EN << shift == 0 { return 0; }
        match (clock >> (T0_SRC_SEL_SHIFT + shift)) & 3 { 0 => 40_000_000, 1 => rc_fast_hz, 2 => 80_000_000, _ => 0 }
    }

    /// Advances T0 and T1 by `cycles` of the `cpu_hz` machine clock, each at the rate of its own
    /// source. A timer whose TnCONFIG.EN is clear costs a test. True when an alarm fired.
    pub fn advance_cpu(&mut self, cycles: u64, cpu_hz: u64, rc_fast_hz: u64) -> bool {
        let mut fired = false;
        for t in 0..2 {
            if self.timers.t[t].config >> 31 == 0 { continue; }
            let hz = self.source_hz(t, rc_fast_hz);
            if hz == 0 { continue; }
            let owed = u128::from(cycles) * u128::from(hz) + u128::from(self.frac[t]);
            self.frac[t] = (owed % u128::from(cpu_hz)) as u64;
            let ticks = (owed / u128::from(cpu_hz)).min(u128::from(u64::MAX)) as u64;
            let raw = &mut self.timers.int_raw;
            self.timers.t[t].step(ticks, (1 << 54) - 1, || { *raw |= 1 << t; fired = true; });
        }
        fired
    }

    /// Machine cycles until the earlier armed alarm of T0 and T1, rounded up so that it is never
    /// late: `advance_cpu` hands a timer its `ticks` once `cycles * hz + frac >= ticks * cpu_hz`.
    pub fn cycles_until_alarm(&self, cpu_hz: u64, rc_fast_hz: u64) -> Option<u64> {
        (0..2).filter_map(|t| {
            let hz = self.source_hz(t, rc_fast_hz);
            if hz == 0 { return None; }
            let need = (u128::from(self.timers.t[t].deadline()?) * u128::from(cpu_hz)).saturating_sub(u128::from(self.frac[t]));
            Some(need.div_ceil(u128::from(hz)).min(u128::from(u64::MAX)) as u64)
        }).min()
    }

    /// The registers from 0x48, behind the group's APB gate: the watchdog's, and INT_*_TIMERS,
    /// where bit 2 is the watchdog's and bits 0 and 1 are T0's and T1's (IDF v6.1
    /// soc/esp32s31/register/soc/timer_group_reg.h). INT_RAW is write-1-to-clear (WTC) like INT_CLR.
    pub fn read_wdt_half(&self, off: u32) -> Result<u32, Fault> {
        let wdt = self.wdt.read(off)?;
        let (raw, ena) = (self.timers.int_raw & 3, self.timers.int_ena & 3);
        Ok(match off { 0x70 => wdt | ena, 0x74 => wdt | raw, 0x78 => wdt | (raw & ena), _ => wdt })
    }

    pub fn write_wdt_half(&mut self, off: u32, value: u32) -> Result<(), Fault> {
        match off {
            0x70 => { self.wdt.write(off, value & 4)?; self.timers.int_ena = value & 3; }
            0x74 | 0x7c => { self.wdt.write(off, value & 4)?; self.timers.int_raw &= !(value & 3); }
            _ => self.wdt.write(off, value)?,
        }
        Ok(())
    }
}

impl Device for TimgS31 {
    fn read(&mut self, off: u32) -> u32 {
        if off < 0x48 {
            self.timers.read(off)
        } else {
            self.read_wdt_half(off).unwrap_or(0)
        }
    }

    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        if off < 0x48 {
            Device::write(&mut self.timers, off, v)
        } else {
            let _ = self.write_wdt_half(off, v);
            WriteEffect::NONE
        }
    }

    /// In the order of the `device_set!` table: T0, T1, WDT.
    fn irq_sources(&self) -> u64 {
        u64::from(self.irq_timers()) | u64::from(self.wdt.irq()) << 2
    }
}

// ── USB Serial/JTAG: shared IP + S31 PCR gate ───────────────────────────────

pub struct UsbS31 {
    pub usj: UsbSerialJtag,
    pub clock: u32,
    attached: bool,
}

impl Default for UsbS31 {
    fn default() -> Self {
        let mut usj = UsbSerialJtag::new(CPU_HZ);
        usj.connected = false;
        Self { usj, clock: 1, attached: false }
    }
}

impl UsbS31 {
    pub fn clock_on(&self) -> bool {
        self.clock & 1 != 0
    }

    pub fn clock_read(&self) -> u32 {
        self.clock
    }

    pub fn clock_write(&mut self, value: u32) {
        self.clock = value & 1;
    }

    pub fn irq(&self) -> bool {
        self.clock_on() && self.usj.irq()
    }

    pub fn attach_host(&mut self, on: bool) {
        self.attached = on;
        self.usj.connected = on;
    }

    pub fn host_send(&mut self, data: &[u8]) -> Result<(), Fault> {
        if !self.attached || !self.clock_on() {
            return Err(Fault::Prohibited);
        }
        if data.is_empty() || data.len() > 64 || !self.usj.rx.is_empty() {
            return Err(Fault::Prohibited);
        }
        self.usj.host_input(data);
        Ok(())
    }

    pub fn host_receive(&mut self) -> Option<Vec<u8>> {
        if !self.attached || !self.clock_on() {
            return None;
        }
        if self.usj.tx_out.is_empty() && self.usj.tx_fifo.is_empty() {
            return None;
        }
        // Prefer flushed bytes; if guest only filled fifo without WR_DONE yet, leave it.
        if self.usj.tx_out.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.usj.tx_out))
    }
}

impl Device for UsbS31 {
    fn read(&mut self, off: u32) -> u32 {
        if !self.clock_on() {
            return 0;
        }
        self.usj.read(off)
    }

    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        if !self.clock_on() {
            return WriteEffect::NONE;
        }
        self.usj.write(off, v);
        WriteEffect::NONE
    }

    fn irq_sources(&self) -> u64 {
        u64::from(self.irq())
    }

    fn clock(&self) -> Option<ClockDomain> {
        Some(ClockDomain::Cpu)
    }

    fn tick(&mut self, cycles: u64) {
        if self.clock_on() {
            self.usj.tick(cycles);
        }
    }
}

// ── Efuse: S31 ReadImage protocol as Device (shared Efuse is RegRam-only) ───

impl Device for Efuse {
    fn read(&mut self, off: u32) -> u32 {
        Efuse::read(self, off).unwrap_or(0)
    }

    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        let _ = Efuse::write(self, off, v);
        WriteEffect::NONE
    }

    fn irq_sources(&self) -> u64 {
        u64::from(Efuse::irq(self))
    }
}

fn new_uart() -> Uart {
    let mut uart = Uart::new(UartLayout {
        thrhd_mask: 0xff,
        rxfifo_rst: 1 << 22,
        rxfifo_cnt_mask: 0xff,
        // ROM UART download mode is not modelled on the S31 yet.
        autobaud_en: 0,
        autobaud_regs: 0,
        autobaud_clock_hz: 0,
    });
    // S31 v6.1 uart_reg.h defaults.
    for (off, value) in [
        (0x14, 694),
        (0x20, 0x1c),
        (0x24, 0x6060),
        (0x88, 3 << 24),
        (0x8c, 36_774_432),
        (0x9c, 1280),
    ] {
        uart.write(off, value);
    }
    uart
}

pub struct Peripherals {
    pub uart0: Uart,
    pub usb: UsbS31,
    pub systimer: SystimerS31,
    pub timg: [TimgS31; 2],
    pub efuse: Efuse,
    /// AES accelerator (DR_REG_AES_BASE 0x2050_8000): same register map as the C6, routed by
    /// `S31Bus` behind the crypto clock. The WPA supplicant unwraps the group key of four-way
    /// handshake message 3 with it.
    pub aes: Aes,
    pub misc: Misc,
    pub clock: ClockTree<{ Peripherals::CLOCKS.len() }>,
}

impl Default for Peripherals {
    fn default() -> Self {
        Self::new()
    }
}

impl Peripherals {
    pub fn new() -> Self {
        Peripherals {
            uart0: new_uart(),
            usb: UsbS31::default(),
            systimer: SystimerS31::default(),
            timg: [TimgS31::default(), TimgS31::default()],
            efuse: Efuse::default(),
            aes: Aes::new(),
            misc: Misc::new(),
            clock: Self::new_clock(),
        }
    }

    pub fn block_name(block: u32) -> &'static str {
        match block {
            0x2038a => "UART0",
            0x20391 => "USB_SERIAL_JTAG",
            0x20399 => "SYSTIMER",
            0x20580 => "TIMG0",
            0x20581 => "TIMG1",
            0x20715 => "EFUSE",
            _ => "?",
        }
    }

    /// True when `addr` belongs to a block in this DeviceSet (not PCR clock words).
    pub fn owns(addr: u32) -> bool {
        matches!(
            addr >> 12,
            0x2038a | 0x20391 | 0x20399 | 0x20580 | 0x20581 | 0x20715
        )
    }

    pub fn read32(&mut self, addr: u32) -> u32 {
        mmio::read32(self, addr)
    }

    pub fn write32(&mut self, addr: u32, v: u32) -> WriteEffect {
        mmio::write32(self, addr, v)
    }
}

device_set! { Peripherals; clock: (clock) CPU_HZ, [(ClockDomain::Xtal, 8), (ClockDomain::Apb, 8), (ClockDomain::Systimer, 8), (ClockDomain::Cpu, 1)];
    0x2038a "UART0" (uart0) => [src::UART0];
    0x20391 "USB_SERIAL_JTAG" (usb) => [src::USB_SERIAL_JTAG];
    0x20399 "SYSTIMER" (systimer) => [src::SYSTIMER_TARGET0, src::SYSTIMER_TARGET1, src::SYSTIMER_TARGET2];
    0x20580 "TIMG0" (timg[0]) => [src::TG0_T0, src::TG0_T1, src::TG0_WDT];
    0x20581 "TIMG1" (timg[1]) => [src::TG1_T0, src::TG1_T1, src::TG1_WDT];
    0x20715 "EFUSE" (efuse) => [NO_SOURCE];
}

impl DeviceSet for Peripherals {
    const BASE: u32 = 0;
    fn block_name(block: u32) -> &'static str {
        Peripherals::block_name(block)
    }
    fn misc(&self) -> &Misc {
        &self.misc
    }
    fn misc_mut(&mut self) -> &mut Misc {
        &mut self.misc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use emu_core::Fault;

    fn systimer_on() -> SystimerS31 {
        let mut t = SystimerS31::default();
        t.clock_write(0x11).unwrap();
        t
    }

    #[test]
    fn systimer_pcr_gate_and_reset() {
        let mut t = systimer_on();
        // Default CONF already enables unit0; advance while counter_on.
        t.advance_ticks(5, [false; 2]);
        assert_eq!(t.st.unit[0], 5);
        // APB gate clear (0x10): MMIO off, but counter_on still true — counters keep running.
        t.clock_write(0x10).unwrap();
        assert!(!t.clock_on());
        t.advance_ticks(3, [false; 2]);
        assert_eq!(t.st.unit[0], 8);
        // Soft reset (bit1) clears IP state while latching clock word.
        t.clock_write(0x13).unwrap();
        assert_eq!(t.st.unit[0], 0);
        assert_eq!(t.clock_write(0x15), Err(Fault::Prohibited));
        // Counter clock bit clear stops ticks.
        t.clock_write(0x11).unwrap();
        t.advance_ticks(2, [false; 2]);
        assert_eq!(t.st.unit[0], 2);
        t.clock_write(0x01).unwrap();
        t.advance_ticks(10, [false; 2]);
        assert_eq!(t.st.unit[0], 2);
    }

    #[test]
    fn systimer_period_mode_enter_reloads_like_comp_load() {
        // IDF port_systick: one-shot COMP_LOAD then PERIOD_MODE without a second load.
        let mut t = systimer_on();
        // unit0 + target0 WORK_EN
        Device::write(&mut t, 0, 0x4100_0000);
        Device::write(&mut t, 0x20, 4); // target0 value low
        Device::write(&mut t, 0x50, 1); // COMP_LOAD
        t.advance_ticks(4, [false; 2]);
        assert_eq!(t.st.int_raw & 1, 1);
        Device::write(&mut t, 0x6c, 1); // clear
        // Enter period mode — wrap auto COMP_LOAD from current count
        Device::write(&mut t, 0x34, (1 << 30) | 4);
        t.advance_ticks(12, [false; 2]);
        assert_ne!(t.st.int_raw & 1, 0);
    }

    #[test]
    fn usb_shared_packet_and_clock_gate() {
        let mut u = UsbS31::default();
        u.attach_host(true);
        // fill 64-byte packet then WR_DONE via write path that flushes at 64
        for b in 0..64u32 {
            Device::write(&mut u, 0, b);
        }
        assert_eq!(u.host_receive(), Some((0..64).map(|x| x as u8).collect()));
        Device::write(&mut u, 0, 65);
        Device::write(&mut u, 4, 1); // WR_DONE
        assert_eq!(u.host_receive(), Some(vec![65]));
        u.clock_write(0);
        assert_eq!(u.host_send(b"x"), Err(Fault::Prohibited));
        assert_eq!(Device::read(&mut u, 4), 0);
    }

    #[test]
    fn device_set_owns_expected_blocks() {
        assert!(Peripherals::owns(0x2038_a000));
        assert!(Peripherals::owns(0x2039_1000));
        assert!(Peripherals::owns(0x2039_9000));
        assert!(Peripherals::owns(0x2058_0000));
        assert!(Peripherals::owns(0x2058_1000));
        assert!(Peripherals::owns(0x2071_5000));
        assert!(!Peripherals::owns(SYSTIMER_CLOCK));
        assert!(!Peripherals::owns(0x2058_3000)); // GPIO stays local
    }

    /// esp-hal 1.2.3 `OneShotTimer::schedule` on timer `t`: LOAD 0, autoreload off, the alarm,
    /// then EN and ALARM_EN; the divider stays at its reset value 1, which divides by 2.
    fn one_shot(g: &mut TimgS31, t: u32, alarm: u32) {
        let base = 0x24 * t;
        Device::write(g, base + 0x18, 0); Device::write(g, base + 0x1c, 0); Device::write(g, base + 0x20, 1);
        Device::write(g, base + 0x10, alarm); Device::write(g, base + 0x14, 0);
        Device::write(g, base, 1 << 31 | 1 << 30 | 1 << 13 | 1 << 10);
    }

    #[test]
    fn timg_timers_count_from_their_selected_source() {
        const CPU: u64 = 320_000_000;
        // T0 and T1 from XTAL, RC_FAST or PLL_F80M; one 2-tick alarm (divider 2: 4 source ticks).
        for (sel, hz) in [(0, 40_000_000), (1, 17_500_000), (2, 80_000_000)] {
            for t in 0..2 {
                let mut g = TimgS31::default();
                g.clock_write(0x001 | 0x120 | sel << 3 | sel << 6).unwrap();
                one_shot(&mut g, t, 2);
                // Four cycles in, the deadline is the first cycle by which four source ticks have
                // passed since the start: exact, never late, whatever the warm-up left owed.
                for warm in [0, 1, 3] { assert!(!g.advance_cpu(warm, CPU, 17_500_000)); }
                let left = g.cycles_until_alarm(CPU, 17_500_000).unwrap();
                assert_eq!(4 + left, (4 * CPU).div_ceil(hz), "sel={sel} t={t}");
                assert!(!g.advance_cpu(left - 1, CPU, 17_500_000), "sel={sel} t={t}: early");
                assert!(g.advance_cpu(1, CPU, 17_500_000), "sel={sel} t={t}: late");
                assert_eq!(g.timers.int_raw, 1 << t);
                assert_eq!(g.irq_timers(), 0, "INT_ENA clear");
            }
        }
        // Tn_CLK_EN clear: the timer stands still and has no deadline.
        let mut g = TimgS31::default();
        g.clock_write(0x001 | 2 << 3).unwrap(); one_shot(&mut g, 0, 2);
        assert_eq!(g.cycles_until_alarm(CPU, 17_500_000), None);
        assert!(!g.advance_cpu(1 << 20, CPU, 17_500_000));
        // RC_FAST powered down: no source clock either.
        g.clock_write(0x021 | 1 << 3).unwrap();
        assert_eq!(g.cycles_until_alarm(CPU, 0), None); assert!(!g.advance_cpu(1 << 20, CPU, 0));
        // There is no fourth source, and the WDT keeps XTAL and its reset behaviour.
        for bad in [0x921 | 3 << 3, 0x921 | 3 << 6, 0x921 | 1 << 9, 0x921 | 1 << 2] { assert_eq!(g.clock_write(bad), Err(Fault::Prohibited), "{bad:#x}"); }
        assert_eq!(g.wdt.clock_read(), 0x021 | 1 << 3);
    }

    #[test]
    fn timg_int_registers_are_shared_by_the_timers_and_the_watchdog() {
        let mut g = TimgS31::default();
        g.clock_write(0x1b1).unwrap(); // esp-hal 1.2.3: PLL_F80M for T0 and T1, WDT clock off
        g.write_wdt_half(0x70, 7).unwrap();
        assert_eq!((g.read_wdt_half(0x70), g.wdt.read(0x70), g.timers.int_ena), (Ok(7), Ok(4), 3));
        one_shot(&mut g, 1, 1);
        assert!(g.advance_cpu(8, 320_000_000, 17_500_000));
        assert_eq!(g.irq_timers(), 2); assert!(!g.irq_wdt());
        assert_eq!((g.read_wdt_half(0x74), g.read_wdt_half(0x78)), (Ok(2), Ok(2)));
        assert_eq!(Device::irq_sources(&g), 2, "T0, T1, WDT, as the device_set! table lists them");
        g.write_wdt_half(0x7c, 1).unwrap(); assert_eq!(g.irq_timers(), 2, "T0's clear bit");
        g.write_wdt_half(0x74, 2).unwrap(); assert_eq!(g.irq_timers(), 0, "INT_RAW is WTC");
        g.write_wdt_half(0x70, 1).unwrap(); assert_eq!(g.read_wdt_half(0x70), Ok(1));
        // The group's APB gate guards these registers, as it guards the watchdog's.
        g.clock_write(0x1b0).unwrap();
        assert_eq!(g.read_wdt_half(0x70), Err(Fault::Prohibited)); assert_eq!(g.write_wdt_half(0x70, 3), Err(Fault::Prohibited));
        assert_eq!(g.timers.int_ena, 1);
        // RST_EN resets T0 and T1 with the watchdog.
        g.clock_write(0x1b3).unwrap();
        assert_eq!((g.timers.t[1].config, g.timers.int_ena), (0x6000_2000, 0));
        assert_eq!(g.cycles_until_alarm(320_000_000, 17_500_000), None);
    }
}
