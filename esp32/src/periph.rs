//! Classic ESP32 peripheral map. Shared models are adapted only where this chip's register layout
//! predates the S3/C3 layout.
use emu_core::{ClockDomain, ClockTree};
use esp_periph::{
    device_set, mmio, Device, DeviceSet, Dispatch, Gpio, Misc, RegRam, RtcCntl, Sha, SpiMem,
    TimerGroup, Uart, UartLayout, WriteEffect,
};

pub const CPU_HZ: u64 = 240_000_000;
pub const PERIPH_BASE: u32 = 0x3ff0_0000;
pub const PERIPH_END: u32 = 0x3ff8_0000;

pub struct Dport {
    pub ram: RegRam,
    pub map: [[u32; 69]; 2],
}
impl Dport {
    fn new() -> Self {
        let mut ram = RegRam::new();
        ram.write(0x2c, 1); // APP CPU held in reset
        Self {
            ram,
            map: [[6; 69]; 2],
        }
    }
    pub fn core1_control(&self) -> (bool, bool, bool) {
        (self.ram.read(0x30) & 1 != 0, self.ram.read(0x2c) & 1 != 0, self.ram.read(0x34) & 1 != 0)
    }
    pub fn cpu_lines(&self, core: usize) -> u32 {
        let mut lines = 0;
        for (source, off) in [(24, 0xdc), (25, 0xe0), (26, 0xe4), (27, 0xe8)] {
            if self.ram.read(off) & 1 != 0 {
                let line = self.map[core][source];
                if line < 32 { lines |= 1 << line; }
            }
        }
        lines
    }
}
impl Device for Dport {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x104..=0x214 => self.map[0][((off - 0x104) / 4) as usize],
            0x218..=0x328 => self.map[1][((off - 0x218) / 4) as usize],
            0x3f0 | 0x418 => 1 << 7, // cache controller idle
            _ => self.ram.read(off),
        }
    }
    fn write(&mut self, off: u32, mut v: u32) -> WriteEffect {
        match off {
            0x104..=0x214 => self.map[0][((off - 0x104) / 4) as usize] = v & 31,
            0x218..=0x328 => self.map[1][((off - 0x218) / 4) as usize] = v & 31,
            0x40 | 0x58 => {
                if v & (1 << 4) != 0 {
                    v = (v & !(1 << 4)) | (1 << 5);
                }
                self.ram.write(off, v);
            }
            _ => self.ram.write(off, v),
        }
        WriteEffect::NONE
    }
}

pub struct ClassicSpi(pub SpiMem);
impl ClassicSpi {
    fn new(command: bool) -> Self {
        let mut s = SpiMem::new(command);
        s.has_psram = false;
        Self(s)
    }
    fn off(off: u32) -> u32 {
        match off {
            0x10 => 0x2c,
            0x1c => 0x18,
            0x20 => 0x1c,
            0x24 => 0x20,
            0x28 => 0x24,
            0x2c => 0x28,
            0x80..=0xbc => off - 0x28,
            _ => off,
        }
    }
}
impl Device for ClassicSpi {
    fn read(&mut self, off: u32) -> u32 {
        self.0.read(Self::off(off))
    }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        if self.0.write(Self::off(off), v) && self.0.is_spi1 {
            WriteEffect::SPI_EXEC
        } else {
            WriteEffect::NONE
        }
    }
    fn debug(&mut self, on: bool) {
        self.0.log = on;
    }
}

pub struct ClassicGpio(pub Gpio);
impl ClassicGpio {
    fn off(off: u32) -> u32 {
        match off {
            0x88..=0x14c => off - 0x14,
            0x130..=0x52c => off + 0x24,
            0x530..=0x5f0 => off + 0x24,
            _ => off,
        }
    }
}
impl Device for ClassicGpio {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x3c => ((self.0.input & !self.0.enable) | (self.0.out & self.0.enable)) as u32,
            0x40 => (((self.0.input & !self.0.enable) | (self.0.out & self.0.enable)) >> 32) as u32,
            _ => self.0.read(Self::off(off)),
        }
    }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        self.0.write(Self::off(off), v);
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 {
        self.0.irq() as u64
    }
}

pub struct ClassicRtc(pub RtcCntl);
impl ClassicRtc {
    fn new() -> Self {
        Self(RtcCntl::new())
    }
    fn off(off: u32) -> u32 {
        match off {
            0x34 => 0x38,
            0x8c..=0xa4 => off + 0xc,
            0xb0..=0xbc => off + 0x10,
            _ => off,
        }
    }
}
impl Device for ClassicRtc {
    fn read(&mut self, off: u32) -> u32 {
        self.0.read(Self::off(off))
    }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        self.0.write(Self::off(off), v);
        WriteEffect::NONE
    }
    fn clock(&self) -> Option<ClockDomain> {
        Some(ClockDomain::RtcSlow)
    }
    fn tick(&mut self, ticks: u64) {
        self.0.slow_ticks += ticks;
        self.0.wdt_tick(ticks);
    }
}

pub struct ClassicEfuse {
    pub ram: RegRam,
    cmd: u32,
}
impl ClassicEfuse {
    fn new(mac: [u8; 6]) -> Self {
        let mut r = RegRam::new();
        r.write(0x04, u32::from_be_bytes([mac[2], mac[3], mac[4], mac[5]]));
        r.write(0x08, (mac[0] as u32) << 8 | mac[1] as u32);
        r.write(0x0c, 1 << 15); // ECO1+
        r.write(0x14, 1 << 20); // ECO2+; together these identify ECO3
        Self { ram: r, cmd: 0 }
    }
}
impl Device for ClassicEfuse {
    fn read(&mut self, off: u32) -> u32 {
        if off == 0x104 {
            return std::mem::take(&mut self.cmd);
        }
        self.ram.read(off)
    }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        if off == 0x104 {
            self.cmd = v;
        } else {
            self.ram.write(off, v);
        }
        WriteEffect::NONE
    }
}

pub struct ClassicSha {
    core: Sha,
    text: [u32; 32],
}
impl ClassicSha {
    fn new() -> Self {
        Self { core: Sha::new(), text: [0; 32] }
    }
    fn run_sha256(&mut self, first: bool) {
        self.core.mode = 2;
        for i in 0..16 {
            self.core.m[i] = self.text[i].swap_bytes();
        }
        self.core.write(if first { 0x10 } else { 0x14 }, 1);
    }
}
impl Device for ClassicSha {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x00..=0x7c => self.text[(off / 4) as usize],
            0x8c | 0x9c | 0xac | 0xbc => 0,
            _ => 0,
        }
    }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        match off {
            0x00..=0x7c => self.text[(off / 4) as usize] = v,
            0x90 => self.run_sha256(true),
            0x94 => self.run_sha256(false),
            0x98 => self.text[..8].copy_from_slice(&self.core.h[..8]),
            _ => {}
        }
        WriteEffect::NONE
    }
}

pub struct Peripherals {
    pub dport: Dport,
    pub uart: [Uart; 3],
    pub spi0: ClassicSpi,
    pub spi1: ClassicSpi,
    pub gpio: ClassicGpio,
    pub rtc: ClassicRtc,
    pub efuse: ClassicEfuse,
    pub sha: ClassicSha,
    pub timg: [TimerGroup; 2],
    pub misc: Misc,
    pub spi_exec: bool,
    clock: ClockTree<2>,
}

device_set! { Peripherals; clock: (clock) CPU_HZ, [(ClockDomain::Apb, 3), (ClockDomain::RtcSlow, 1600)];
    0x00 "DPORT" (dport) => [];
    0x03 "SHA" (sha) => [];
    0x40 "UART0" (uart[0]) => [];
    0x42 "SPI1" (spi1) => [];
    0x43 "SPI0" (spi0) => [];
    0x44 "GPIO" (gpio) => [];
    0x48 "RTCCNTL" (rtc) => [];
    0x50 "UART1" (uart[1]) => [];
    0x5a "EFUSE" (efuse) => [];
    0x5f "TIMG0" (timg[0]) => [];
    0x60 "TIMG1" (timg[1]) => [];
    0x6e "UART2" (uart[2]) => [];
}

impl DeviceSet for Peripherals {
    const BASE: u32 = PERIPH_BASE;
    fn block_name(block: u32) -> &'static str {
        Self::block_name(block)
    }
    fn misc(&self) -> &Misc {
        &self.misc
    }
    fn misc_mut(&mut self) -> &mut Misc {
        &mut self.misc
    }
}

impl Peripherals {
    pub fn new(mac: [u8; 6]) -> Self {
        let uart = UartLayout {
            thrhd_mask: 0x7f,
            rxfifo_rst: 1 << 17,
            rxfifo_cnt_mask: 0xff,
        };
        let mut gpio = Gpio::new();
        gpio.strap = 0x13; // normal SPI-fast-flash boot, with ROM messages enabled
        Self {
            dport: Dport::new(),
            uart: [Uart::new(uart), Uart::new(uart), Uart::new(uart)],
            spi0: ClassicSpi::new(false),
            spi1: ClassicSpi::new(true),
            gpio: ClassicGpio(gpio),
            rtc: ClassicRtc::new(),
            efuse: ClassicEfuse::new(mac),
            sha: ClassicSha::new(),
            timg: [TimerGroup::new(), TimerGroup::new()],
            misc: Misc::new(),
            spi_exec: false,
            clock: Self::new_clock(),
        }
    }
    pub fn block_name(block: u32) -> &'static str {
        match block {
            0x03 => "SHA",
            0x00..=0x13 => "DPORT",
            0x40 => "UART0",
            0x42 => "SPI1",
            0x43 => "SPI0",
            0x44 => "GPIO",
            0x48 => "RTCCNTL",
            0x49 => "IO_MUX",
            0x50 => "UART1",
            0x53 => "I2C0",
            0x56 => "RMT",
            0x59 => "LEDC",
            0x5a => "EFUSE",
            0x5f => "TIMG0",
            0x60 => "TIMG1",
            0x64 => "SPI2",
            0x65 => "SPI3",
            0x66 => "SYSCON",
            0x67 => "I2C1",
            0x6e => "UART2",
            _ => "?",
        }
    }
    pub fn read32(&mut self, addr: u32) -> u32 {
        mmio::read32(self, addr)
    }
    pub fn write32(&mut self, addr: u32, v: u32) {
        if mmio::write32(self, addr, v).contains(WriteEffect::SPI_EXEC) {
            self.spi_exec = true;
        }
    }
    pub fn tick(&mut self, cycles: u64) {
        Dispatch::tick(self, cycles);
    }
    pub fn cycles_until_timer(&self) -> u32 {
        Dispatch::cycles_until_deadline(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn classic_spi_and_gpio_offsets_reach_shared_models() {
        let mut p = Peripherals::new([0; 6]);
        p.write32(0x3ff4_2080, 0x1122_3344);
        assert_eq!(p.read32(0x3ff4_2080), 0x1122_3344);
        p.write32(0x3ff4_4024, 1 << 2);
        p.write32(0x3ff4_4008, 1 << 2);
        assert_eq!(p.read32(0x3ff4_403c) & (1 << 2), 1 << 2);
    }

    #[test]
    fn app_cpu_follows_dport_control_bits() {
        let mut dport = Dport::new();
        assert_eq!(dport.core1_control(), (false, true, false));
        dport.write(0x30, 1);
        dport.write(0x2c, 0);
        assert_eq!(dport.core1_control(), (true, false, false));
    }

    #[test]
    fn dport_routes_cross_core_interrupts() {
        let mut dport = Dport::new();
        dport.write(0x104 + 24 * 4, 7);
        dport.write(0xdc, 1);
        assert_eq!(dport.cpu_lines(0), 1 << 7);
        dport.write(0xdc, 0);
        assert_eq!(dport.cpu_lines(0), 0);
    }

    #[test]
    fn classic_sha256_layout_hashes_one_block() {
        let mut sha = ClassicSha::new();
        let mut block = [0u8; 64];
        block[..3].copy_from_slice(b"abc");
        block[3] = 0x80;
        block[63] = 24;
        for (i, word) in block.chunks_exact(4).enumerate() {
            sha.write((i * 4) as u32, u32::from_be_bytes(word.try_into().unwrap()));
        }
        sha.write(0x90, 1);
        sha.write(0x98, 1);
        let got: Vec<u8> = (0..8).flat_map(|i| sha.read(i * 4).to_be_bytes()).collect();
        assert_eq!(got, [0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea,
                         0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
                         0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c,
                         0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad]);
    }
}
