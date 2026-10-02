//! ESP32-C3 peripherals.
//!
//! The C3 and the S3 share most of their peripheral IP — UART, USB-Serial/JTAG, systimer, timer
//! groups, GPIO, the SPI flash controller, GDMA, SHA/AES/RSA are the same blocks with the same
//! register layouts — so the models come from `esp-periph` and only the address map, the cache
//! controller and the interrupt controller are written here.

use esp_periph::{i2c::I2c, rmt_compact::RmtCompact, GpSpi};
use emu_core::{ClockDomain, ClockTree};
use esp_periph::{device_set, mmio, Device, DeviceSet, Dispatch, Misc, WriteEffect, NO_SOURCE};
use esp_periph::{Aes, Efuse, Gdma, Gpio, RegRam, Rsa, RtcCntl, Sha, SpiMem, SystemRegs, Systimer, TimerGroup, Uart, UartLayout, UsbSerialJtag};

pub const CPU_HZ: u64 = 160_000_000;
pub const PERIPH_BASE: u32 = 0x6000_0000;
pub const PERIPH_END: u32 = 0x6010_0000;

/// Interrupt sources, numbered as the hardware numbers them — that is, by the order of the
/// `INTERRUPT_CORE0_*_MAP_REG` registers. Do **not** take these from `soc/interrupts.h`: that
/// enum omits the NMI entries, so its indices are shifted and every source lands on the wrong
/// line. Only the sources we can assert are listed.
pub mod src {
    pub const APB_CTRL: usize = 14; pub const GPIO: usize = 16; pub const SPI2: usize = 19;
    pub const UART0: usize = 21; pub const UART1: usize = 22; pub const LEDC: usize = 23;
    pub const EFUSE: usize = 24; pub const USB_SERIAL_JTAG: usize = 26; pub const RTC_CORE: usize = 27;
    pub const RMT: usize = 28; pub const I2C_EXT0: usize = 29;
    pub const TG0_T0: usize = 32; pub const TG0_WDT: usize = 33;
    pub const TG1_T0: usize = 34; pub const TG1_WDT: usize = 35;
    pub const SYSTIMER_T0: usize = 37; pub const SYSTIMER_T1: usize = 38; pub const SYSTIMER_T2: usize = 39;
    pub const DMA_CH0: usize = 44; pub const DMA_CH1: usize = 45; pub const DMA_CH2: usize = 46;
    pub const RSA: usize = 47; pub const AES: usize = 48; pub const SHA: usize = 49;
    /// software interrupts, raised by writing `SYSTEM_CPU_INTR_FROM_CPU_n` — this is how the
    /// FreeRTOS port yields, so without them `xPortStartScheduler` just returns
    pub const FROM_CPU0: usize = 50;
    pub const COUNT: usize = 62;
}

/// The C3's interrupt matrix (`INTERRUPT_CORE0`, 0x600C2000).
///
/// 62 peripheral sources are each mapped to one of 31 CPU interrupt lines. A line is taken when
/// it is enabled, its priority is **at or above** the threshold, and `mstatus.MIE` is set; the
/// CPU then vectors to `mtvec + 4*line`. Level lines follow the source; edge lines latch and are
/// cleared by writing CPU_INT_CLEAR.
pub struct Intc {
    pub map: [u32; src::COUNT],
    pub lines: esp_periph::intmtx::Lines,
    ram: RegRam,
}

impl Default for Intc { fn default() -> Self { Self::new() } }

impl Intc {
    pub fn new() -> Self {
        Intc { map: [0; src::COUNT], lines: Default::default(), ram: RegRam::new() }
    }

    pub fn read(&self, off: u32) -> u32 {
        match off {
            0x000..=0x0f8 => self.map.get((off / 4) as usize).copied().unwrap_or(0),
            0x104 => self.lines.enable,
            0x108 => self.lines.int_type,
            0x110 => self.lines.level | self.lines.edge_pending,     // EIP_STATUS: raw source state
            0x114..=0x190 => self.lines.pri[((off - 0x114) / 4) as usize],
            0x194 => self.lines.thresh,
            _ => self.ram.read(off),
        }
    }

    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x000..=0x0f8 => { if let Some(m) = self.map.get_mut((off / 4) as usize) { *m = v & 0x1f; } }
            0x104 => self.lines.enable = v,
            0x108 => self.lines.int_type = v,
            0x10c => self.lines.edge_pending &= !v,            // CPU_INT_CLEAR
            0x114..=0x190 => self.lines.pri[((off - 0x114) / 4) as usize] = v & 0xf,
            0x194 => self.lines.thresh = v & 0xf,
            _ => self.ram.write(off, v),
        }
    }
}


/// The C3's cache controller. Only the "operation finished" bits matter to us: the ROM and the
/// bootloader kick a sync/preload/lock and then poll for done, so a model that never completes
/// hangs the boot. Register offsets differ from the S3's, which is why this is not shared.
pub struct Extmem { ram: RegRam }

impl Default for Extmem { fn default() -> Self { Self::new() } }

impl Extmem {
    pub fn new() -> Self { Extmem { ram: RegRam::new() } }
    pub fn read(&self, off: u32) -> u32 {
        let v = self.ram.read(off);
        match off {
            0x01c => v | (1 << 2),                 // ICACHE_LOCK_CTRL: LOCK_DONE
            0x028 => v | (1 << 1),                 // ICACHE_SYNC_CTRL: SYNC_DONE
            0x034 => v | (1 << 1),                 // ICACHE_PRELOAD_CTRL: PRELOAD_DONE
            0x040 => v | (1 << 3),                 // ICACHE_AUTOLOAD_CTRL: AUTOLOAD_DONE
            0x0b0 => 0x001,                        // CACHE_STATE: icache idle
            0x0cc => if v & 1 != 0 { v | (1 << 2) } else { v & !(1 << 2) },   // ICACHE_FREEZE: DONE follows ENA
            0x3fc => 0x2007_0000,                  // DATE
            _ => v,
        }
    }
    pub fn write(&mut self, off: u32, v: u32) { self.ram.write(off, v); }
}


/// Seed the efuse block the way real C3 silicon reads back. The S3's `Efuse::new` lays the
/// wafer-version fields out for its own chip; on the C3 they live in BLK1 bits 114 (minor low),
/// 183 (minor high) and 184 (major), and the bootloader refuses to start an app whose
/// `min_chip_rev` is above what it finds — so getting these wrong stops the boot with
/// "chip revision check failed".
/// Values verified against real silicon (a C3 module, MAC 3c:84:27:b6:a7:1c, 2026-08-29):
/// wafer v0.4, package 0, block revision v1.3. The bootloader prints both and refuses to start
/// an app whose `min_chip_rev` is above the wafer version.
pub fn efuse_c3(mac: [u8; 6], rev_major: u32, rev_minor: u32, blk_minor: u32) -> Efuse {
    let mut e = Efuse::new(mac);
    e.write(0x48, (mac[0] as u32) << 8 | mac[1] as u32);       // BLK1 word 1: MAC high, nothing else
    // BLK1 word 3 holds WAFER_VERSION_MINOR_LO (bit 114), PKG_VERSION (117) and BLK_VERSION_MINOR (120)
    e.write(0x50, (rev_minor & 7) << 18 | (blk_minor & 7) << 24);
    e.write(0x58, ((rev_minor >> 3) & 1) << 23 | (rev_major & 3) << 24);
    e                                                          // BLK_VERSION_MAJOR = 1 comes from Efuse::new (0x6c)
}
impl Device for Intc {
    fn read(&mut self, off: u32) -> u32 { Intc::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Intc::write(self, off, v); WriteEffect::NONE }
}
impl Device for Extmem {
    fn read(&mut self, off: u32) -> u32 { Extmem::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Extmem::write(self, off, v); WriteEffect::NONE }
}

/// APB_CTRL + 0xB0 is the hardware RNG (`WDEV_RND_REG`); everything else in the block is plain
/// configuration. The model is the shared xorshift32 in `esp_periph::rng`; re-exported here so
/// `esp32c3::periph::Rng` keeps working.
pub use esp_periph::Rng;

pub struct Peripherals {
    pub i2c: I2c,
    pub spi2: GpSpi,
    pub rmt: RmtCompact,
    pub io_mux: RegRam,
    pub uart: [Uart; 2],
    pub usb: UsbSerialJtag,
    pub systimer: Systimer,
    pub timg: [TimerGroup; 2],
    pub gpio: Gpio,
    pub rtc: RtcCntl,
    pub efuse: Efuse,
    pub system: SystemRegs,
    pub extmem: Extmem,
    pub intc: Intc,
    pub spi0: SpiMem,
    pub spi1: SpiMem,
    pub gdma: Gdma,
    pub sha: Sha,
    pub aes: Aes,
    pub rsa: Rsa,
    pub rng: Rng,
    /// register RAM behind unmodelled blocks, first-touch logging, pc attribution
    pub misc: Misc,
    pub spi_exec: bool,
    clock: ClockTree<4>,
    last_status: [u32; 4],
}

// Every peripheral, where it sits, and its interrupt source numbers (`src`).
device_set! { Peripherals; clock: (clock) CPU_HZ, [(ClockDomain::Systimer, 10), (ClockDomain::Apb, 2), (ClockDomain::RtcSlow, 1067), (ClockDomain::Cpu, 1)];
    0x00 "UART0" (uart[0]) => [src::UART0];
    0x10 "UART1" (uart[1]) => [src::UART1];
    0x02 "SPI1" (spi1) => [];
    0x03 "SPI0" (spi0) => [];
    0x04 "GPIO" (gpio) => [src::GPIO];
    0x09 "IO_MUX" (io_mux) => [];
    0x13 "I2C0" (i2c) => [src::I2C_EXT0];
    0x16 "RMT" (rmt) => [src::RMT];
    0x24 "SPI2" (spi2) => [src::SPI2];
    // the efuse controller shares the RTC block on the C3, at +0x800
    0x08 "EFUSE" (efuse) delta -0x800 @ 0x800..=0xfff => [];
    0x08 "RTCCNTL" (rtc) => [];
    0x1f "TIMG0" (timg[0]) => [src::TG0_T0];
    0x20 "TIMG1" (timg[1]) => [src::TG1_T0];
    0x23 "SYSTIMER" (systimer) => [src::SYSTIMER_T0, src::SYSTIMER_T1, src::SYSTIMER_T2];
    0x26 "APB_CTRL" (rng) @ 0xb0..=0xb3 => [];
    0x3a "AES" (aes) => [src::AES];
    0x3b "SHA" (sha) => [];
    0x3c "RSA" (rsa) => [src::RSA];
    // three channels; out and in interrupts of a channel share one source
    0x3f "GDMA" (gdma) => [src::DMA_CH0, src::DMA_CH1, src::DMA_CH2, NO_SOURCE, NO_SOURCE, src::DMA_CH0, src::DMA_CH1, src::DMA_CH2, NO_SOURCE, NO_SOURCE];
    0x43 "USB_SERIAL_JTAG" (usb) => [src::USB_SERIAL_JTAG];
    0xc0 "SYSTEM" (system) => [src::FROM_CPU0, src::FROM_CPU0 + 1, src::FROM_CPU0 + 2, src::FROM_CPU0 + 3];
    0xc2 "INTERRUPT" (intc) => [];
    0xc4 "EXTMEM" (extmem) => [];
}

impl DeviceSet for Peripherals {
    const BASE: u32 = PERIPH_BASE;
    fn block_name(block: u32) -> &'static str { Peripherals::block_name(block) }
    fn misc(&self) -> &Misc { &self.misc }
    fn misc_mut(&mut self) -> &mut Misc { &mut self.misc }
    fn pre_access(&mut self, block: u32, _off: u32, _write: bool) {
        if block == 0x26 { self.rng.now = self.clock.cycles() as u32; }
    }
}

impl Peripherals {
    pub fn new(mac: [u8; 6]) -> Self {
        Peripherals {
            i2c: I2c::new(), spi2: GpSpi::new(), rmt: RmtCompact::new(CPU_HZ), io_mux: RegRam::new(),
            uart: [Uart::new(UartLayout::C3), Uart::new(UartLayout::C3)], usb: UsbSerialJtag::new(CPU_HZ), systimer: Systimer::new(),
            timg: [TimerGroup::new(), TimerGroup::new()], gpio: { let mut g = Gpio::new(); g.func_out_sel.fill(128); g }, rtc: RtcCntl::new_c3(),
            efuse: efuse_c3(mac, 0, 4, 3), system: SystemRegs::new(0x28), extmem: Extmem::new(), intc: Intc::new(),
            spi0: { let mut s = SpiMem::new(false); s.has_psram = false; s },
            spi1: { let mut s = SpiMem::new(true); s.has_psram = false; s },   // the C3 has no PSRAM
            gdma: Gdma::new(),
            sha: Sha::new(), aes: Aes::new(), rsa: Rsa::new(), rng: Rng::new(),
            misc: Misc::new(), spi_exec: false, clock: Self::new_clock(),
            last_status: [0; 4],
        }
    }

    pub fn block_name(block: u32) -> &'static str {
        match block {
            0x00 => "UART0", 0x02 => "SPI1", 0x03 => "SPI0", 0x04 => "GPIO", 0x05 => "FE2", 0x06 => "FE",
            0x08 => "RTCCNTL/EFUSE", 0x09 => "IO_MUX", 0x0e => "RTC_I2C", 0x10 => "UART1",
            0x13 => "I2C0", 0x14 => "UHCI0", 0x16 => "RMT", 0x19 => "LEDC", 0x1c => "NRX", 0x1d => "BB",
            0x1f => "TIMG0", 0x20 => "TIMG1", 0x23 => "SYSTIMER", 0x24 => "SPI2", 0x26 => "APB_CTRL",
            0x2b => "TWAI", 0x2d => "I2S", 0x3a => "AES", 0x3b => "SHA", 0x3c => "RSA", 0x3d => "DS",
            0x3e => "HMAC", 0x3f => "GDMA", 0x40 => "APB_SARADC", 0x43 => "USB_SERIAL_JTAG",
            0xc0 => "SYSTEM", 0xc1 => "SENSITIVE", 0xc2 => "INTERRUPT", 0xc4 => "EXTMEM",
            0xc5 => "MMU", 0xcc => "XTS_AES", 0xce => "ASSIST_DEBUG", 0xcf => "DEDICATED_GPIO",
            _ => "?",
        }
    }

    pub fn read32(&mut self, addr: u32) -> u32 { mmio::read32(self, addr) }

    pub fn write32(&mut self, addr: u32, v: u32) {
        if addr == 0x6001_3004 && v & (1 << 5) != 0 && self.i2c.has_pinned_devices() {
            let pins = self.i2c_pin(54).zip(self.i2c_pin(53));
            self.i2c.set_pins(pins);
        }
        if mmio::write32(self, addr, v).contains(WriteEffect::SPI_EXEC) { self.spi_exec = true; }
    }

    /// Advance every clocked device by `cycles` CPU cycles (16 MHz systimer, 80 MHz APB, ~150 kHz
    /// RTC slow clock), with delivered-tick accounting so a slow clock never drifts.
    pub fn tick(&mut self, cycles: u64) { Dispatch::tick(self, cycles); }

    pub fn cycles_until_timer(&self) -> u32 { Dispatch::cycles_until_deadline(self) }

    /// Which interrupt sources are asserted right now.
    pub fn source_status(&self) -> [u32; 4] { Dispatch::source_status(self) }

    /// Refresh the interrupt matrix; returns true if any source changed.
    pub fn refresh_lines(&mut self) -> bool {
        let st = self.source_status();
        let changed = st != self.last_status;
        self.last_status = st;
        self.intc.lines.update(&self.intc.map, &st);
        changed
    }
}
