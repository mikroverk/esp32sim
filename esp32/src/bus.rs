//! Classic ESP32 memory map and flash-cache MMUs.
//! ESP-IDF v5.5.4 components/soc/esp32/include/soc/soc.h:170-199 gives memory bounds.
//! Flash MMU behavior and the ROM integrity payload mapping are inferred from ROM execution.
use crate::periph::{Peripherals, PERIPH_BASE, PERIPH_END};
use esp_periph::{Device, RegRam, Rng};
use xtensa_lx7::bus::{Bus, Fault};

pub const DRAM_LOW: u32 = 0x3ffa_e000;
pub const DRAM_HIGH: u32 = 0x4000_0000;
// The ECO3 ELF carries one reset-integrity payload 0x504 bytes below documented DRAM.
const ROM_DRAM_LOW: u32 = 0x3ffa_dafc;
pub const IRAM_LOW: u32 = 0x4008_0000;
pub const IRAM_HIGH: u32 = 0x400c_0000;
pub const IROM_MASK_LOW: u32 = 0x4000_0000;
pub const IROM_MASK_HIGH: u32 = 0x4007_0000;
pub const DROM_MASK_LOW: u32 = 0x3ff9_0000;
pub const DROM_MASK_HIGH: u32 = 0x3ffa_0000;
pub const DBUS_LOW: u32 = 0x3f40_0000;
pub const DBUS_HIGH: u32 = 0x3f80_0000;
pub const IBUS_LOW: u32 = 0x400d_0000;
pub const IBUS_HIGH: u32 = 0x4040_0000;
const RTC_FAST_D: u32 = 0x3ff8_0000;
const RTC_FAST_I: u32 = 0x400c_0000;
const RTC_SLOW: u32 = 0x5000_0000;
// DPORT cache-MMU tables: 64 KiB flash pages, bit 8 marks an invalid entry.
const MMU_PRO: u32 = 0x3ff1_0000;
const MMU_APP: u32 = 0x3ff1_2000;
const MMU_INVALID: u32 = 1 << 8;
const PAGE: usize = 0x1_0000;
const WDEV_RND: u32 = 0x6003_5144;
// FIFO AHB windows are aliases, not separate UART register banks.
const UART_FIFO_AHB: [u32; 3] = [0x6000_0000, 0x6001_0000, 0x6002_e000];

pub struct SocBus {
    pub dram: Vec<u8>,
    pub iram: Vec<u8>,
    pub cache_ram: Vec<u8>,
    pub irom: Vec<u8>,
    pub drom: Vec<u8>,
    pub rtc_fast: Vec<u8>,
    pub rtc_slow: Vec<u8>,
    pub flash: Vec<u8>,
    pub ana: RegRam,
    pub rng: Rng,
    pub mmu: [[u32; 2048]; 2],
    pub periph: Peripherals,
    pub board: esp_soc::Board,
    pub cycles: u64,
    pub last_fault: Option<(u32, bool)>,
    pub irq_dirty: bool,
    pub gpio_events: Option<Vec<(u64, u8, bool)>>,
    pub debug: esp_soc::DebugFlags,
    page_ver: Vec<u32>,
}

impl SocBus {
    pub fn new(flash_size: usize, mac: [u8; 6]) -> Self {
        Self {
            dram: vec![0; (DRAM_HIGH - ROM_DRAM_LOW) as usize],
            iram: vec![0; (IRAM_HIGH - IRAM_LOW) as usize],
            cache_ram: vec![0; 0x1_0000],
            irom: vec![0; (IROM_MASK_HIGH - IROM_MASK_LOW) as usize],
            drom: vec![0; (DROM_MASK_HIGH - DROM_MASK_LOW) as usize],
            rtc_fast: vec![0; 0x2000],
            rtc_slow: vec![0; 0x2000],
            flash: vec![0xff; flash_size],
            ana: RegRam::new(),
            rng: Rng::new(),
            mmu: [[MMU_INVALID; 2048]; 2],
            periph: Peripherals::new(mac),
            board: Box::new(esp_soc::NoBoard),
            cycles: 0,
            last_fault: None,
            irq_dirty: true,
            gpio_events: None,
            debug: Default::default(),
            page_ver: vec![0; 0x10021],
        }
    }
    // 256-byte decode pages. RTC FAST's data and instruction windows share backing bytes.
    fn version_page(addr: u32) -> usize {
        let addr = if (RTC_FAST_D..RTC_FAST_D + 0x2000).contains(&addr) { RTC_FAST_I + addr - RTC_FAST_D } else { addr };
        match addr {
            0x3f40_0000..=0x403f_ffff => ((addr - 0x3f40_0000) >> 8) as usize,
            RTC_SLOW..=0x5000_1fff => 0x10000 + ((addr - RTC_SLOW) >> 8) as usize,
            _ => 0x10020,
        }
    }
    fn written(&mut self, addr: u32, len: usize) {
        if len == 0 { return; }
        let first = Self::version_page(addr);
        let last = Self::version_page(addr + len as u32 - 1);
        let first = if addr & 255 < emu_core::bus::PREV_PAGE_BYTES { first.saturating_sub(1) } else { first };
        for version in &mut self.page_ver[first..=last] { *version = version.wrapping_add(1); }
    }
    fn flash_written(&mut self, off: usize, len: usize) {
        if len == 0 { return; }
        for slot in 0..128 {
            let entry = self.mmu[0][slot];
            if entry & MMU_INVALID != 0 { continue; }
            let start = (entry as usize & 255) * PAGE;
            if off < start + PAGE && start < off + len {
                if let Some(addr) = Self::mmu_address(slot) { self.written(addr, PAGE); }
            }
        }
    }
    fn mmu_address(slot: usize) -> Option<u32> {
        match slot {
            0..=63 => Some(DBUS_LOW + slot as u32 * PAGE as u32),
            77..=127 => Some(0x4000_0000 + (slot as u32 - 64) * PAGE as u32),
            _ => None,
        }
    }
    fn flash_off(&self, addr: u32) -> Option<usize> {
        let (table, index) = if (DBUS_LOW..DBUS_HIGH).contains(&addr) {
            (0, ((addr - DBUS_LOW) >> 16) as usize)
        } else if (IBUS_LOW..IBUS_HIGH).contains(&addr) {
            (64, ((addr - 0x4000_0000) >> 16) as usize)
        } else {
            return None;
        };
        let entry = self.mmu[0][table + index];
        if entry & MMU_INVALID != 0 {
            return None;
        }
        let off = (entry as usize & 0xff) * PAGE + (addr as usize & 0xffff);
        (off < self.flash.len()).then_some(off)
    }
    fn memory(&mut self, addr: u32) -> Option<(&mut Vec<u8>, usize, bool)> {
        match addr {
            IROM_MASK_LOW..=0x4006_ffff => {
                Some((&mut self.irom, (addr - IROM_MASK_LOW) as usize, false))
            }
            DROM_MASK_LOW..=0x3ff9_ffff => {
                Some((&mut self.drom, (addr - DROM_MASK_LOW) as usize, false))
            }
            ROM_DRAM_LOW..=0x3fff_ffff => {
                Some((&mut self.dram, (addr - ROM_DRAM_LOW) as usize, true))
            }
            0x4007_0000..=0x4007_ffff => {
                Some((&mut self.cache_ram, (addr - 0x4007_0000) as usize, true))
            }
            IRAM_LOW..=0x400b_ffff => Some((&mut self.iram, (addr - IRAM_LOW) as usize, true)),
            0x3ff8_0000..=0x3ff8_1fff => {
                Some((&mut self.rtc_fast, (addr - RTC_FAST_D) as usize, true))
            }
            0x400c_0000..=0x400c_1fff => {
                Some((&mut self.rtc_fast, (addr - RTC_FAST_I) as usize, true))
            }
            0x5000_0000..=0x5000_1fff => {
                Some((&mut self.rtc_slow, (addr - RTC_SLOW) as usize, true))
            }
            _ => self
                .flash_off(addr)
                .map(|off| (&mut self.flash, off, false)),
        }
    }
    fn is_periph(addr: u32) -> bool {
        (PERIPH_BASE..PERIPH_END).contains(&addr)
            || (0x6000_e000..0x6000_f000).contains(&addr)
            || (addr & !3) == WDEV_RND
            || UART_FIFO_AHB.contains(&(addr & !3))
    }
    fn mmu_slot(addr: u32) -> Option<(usize, usize)> {
        for (cpu, base) in [(0, MMU_PRO), (1, MMU_APP)] {
            if (base..base + 0x2000).contains(&addr) {
                return Some((cpu, ((addr - base) >> 2) as usize));
            }
        }
        None
    }
    fn periph_read(&mut self, addr: u32, size: u32) -> u32 {
        self.deliver_board_inputs();
        if let Some((cpu, n)) = Self::mmu_slot(addr) {
            return self.mmu[cpu][n];
        }
        let a = addr & !3;
        let w = if a == WDEV_RND {
            self.rng.now = self.cycles as u32;
            self.rng.read(0)
        } else if let Some(n) = UART_FIFO_AHB.iter().position(|&fifo| fifo == a) {
            self.periph.uart[n].read(0)
        } else if (0x6000_e000..0x6000_f000).contains(&a) {
            self.ana.read(a - 0x6000_e000)
        } else {
            self.periph.read32(a)
        };
        match size {
            1 => (w >> ((addr & 3) * 8)) & 0xff,
            2 => (w >> ((addr & 2) * 8)) & 0xffff,
            _ => w,
        }
    }
    fn periph_write(&mut self, addr: u32, value: u32, size: u32) {
        if let Some((cpu, n)) = Self::mmu_slot(addr) {
            self.mmu[cpu][n] = value & 0x1ff;
            if cpu == 0 { if let Some(addr) = Self::mmu_address(n) { self.written(addr, PAGE); } }
            return;
        }
        let a = addr & !3;
        if a == WDEV_RND {
            return;
        }
        if let Some(n) = UART_FIFO_AHB.iter().position(|&fifo| fifo == a) {
            self.periph.uart[n].write(0, value);
            self.board.uart_tx(self.cycles, self.periph.uart_route(n), value as u8);
            self.irq_dirty = true;
            return;
        }
        if (0x6000_e000..0x6000_f000).contains(&a) {
            let old = self.ana.read(a - 0x6000_e000);
            let v = match size {
                4 => value,
                1 => {
                    let sh = (addr & 3) * 8;
                    (old & !(0xff << sh)) | ((value & 0xff) << sh)
                }
                _ => {
                    let sh = (addr & 2) * 8;
                    (old & !(0xffff << sh)) | ((value & 0xffff) << sh)
                }
            };
            self.ana.write(a - 0x6000_e000, v);
            return;
        }
        let v = match size {
            4 => value,
            1 => {
                let old = self.periph.read32(a);
                let sh = (addr & 3) * 8;
                (old & !(0xff << sh)) | ((value & 0xff) << sh)
            }
            _ => {
                let old = self.periph.read32(a);
                let sh = (addr & 2) * 8;
                (old & !(0xffff << sh)) | ((value & 0xffff) << sh)
            }
        };
        let old_enable = self.periph.gpio.gpio.enable;
        self.periph.write32(a, v);
        if let Some(port) = [0x3ff4_0000, 0x3ff5_0000, 0x3ff6_e000].iter().position(|&base| a == base) {
            self.board.uart_tx(self.cycles, self.periph.uart_route(port), v as u8);
        }
        if old_enable != self.periph.gpio.gpio.enable && self.periph.gpio.gpio.changes.is_empty() {
            self.board.gpio_output_at(self.cycles, &[], self.periph.gpio.gpio.enable, self.periph.gpio.gpio.out);
        }
        if self.periph.spi_exec {
            self.run_spi();
        }
        if !self.periph.gpio.gpio.changes.is_empty() {
            let changes = std::mem::take(&mut self.periph.gpio.gpio.changes);
            if let Some(events) = &mut self.gpio_events {
                for &(pin, level) in &changes {
                    events.push((self.cycles, pin, level));
                }
            }
            self.board.gpio_output_at(self.cycles, &changes, self.periph.gpio.gpio.enable, self.periph.gpio.gpio.out);
            self.deliver_board_inputs();
        }
        self.irq_dirty = true;
    }
    fn deliver_board_inputs(&mut self) {
        self.board.advance_to(self.cycles);
        for edge in self.board.take_edges() {
            self.periph.gpio.set_input(edge.pin, edge.level);
            self.irq_dirty = true;
            if let Some(events) = &mut self.gpio_events { events.push((edge.cycle, edge.pin, edge.level)); }
        }
        for pin in self.board.released_inputs() { esp_soc::SocBus::gpio_release_input(self, pin); }
    }
    fn run_spi(&mut self) {
        self.periph.spi_exec = false;
        self.periph.spi1.0.execute(&mut self.flash, &mut []);
        for (_, off, len) in std::mem::take(&mut self.periph.spi1.0.dirty) { self.flash_written(off, len); }
    }
    pub fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> {
        let target = self
            .flash
            .get_mut(offset..)
            .and_then(|d| d.get_mut(..data.len()))
            .ok_or("flash image too large")?;
        target.copy_from_slice(data);
        self.flash_written(offset, data.len());
        Ok(())
    }
    pub fn load_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        for (i, &v) in data.iter().enumerate() {
            let a = addr.wrapping_add(i as u32);
            match self.memory(a) {
                Some((b, o, _)) if o < b.len() => b[o] = v,
                _ => return Err(format!("load: address {a:#010x} not mapped")),
            }
        }
        self.written(addr, data.len());
        Ok(())
    }
}

macro_rules! read {
    ($self:ident, $addr:expr, $n:expr, $conv:expr) => {{
        let addr = $addr;
        match $self.memory(addr) {
            Some((b, o, _)) if b.len().saturating_sub(o) >= $n => Ok($conv(&b[o..o + $n])),
            _ => {
                $self.last_fault = Some((addr, false));
                Err(Fault::Unmapped)
            }
        }
    }};
}
impl Bus for SocBus {
    fn note_code_page(&mut self, _vidx: u32) {}
    fn read8(&mut self, a: u32) -> Result<u8, Fault> {
        if Self::is_periph(a) {
            Ok(self.periph_read(a, 1) as u8)
        } else {
            read!(self, a, 1, |b: &[u8]| b[0])
        }
    }
    fn read16(&mut self, a: u32) -> Result<u16, Fault> {
        if Self::is_periph(a) {
            Ok(self.periph_read(a, 2) as u16)
        } else {
            read!(self, a, 2, |b: &[u8]| u16::from_le_bytes(
                b.try_into().unwrap()
            ))
        }
    }
    fn read32(&mut self, a: u32) -> Result<u32, Fault> {
        if Self::is_periph(a) {
            Ok(self.periph_read(a, 4))
        } else {
            read!(self, a, 4, |b: &[u8]| u32::from_le_bytes(
                b.try_into().unwrap()
            ))
        }
    }
    fn write8(&mut self, a: u32, v: u8) -> Result<(), Fault> {
        if Self::is_periph(a) {
            self.periph_write(a, v as u32, 1);
            return Ok(());
        }
        match self.memory(a) {
            Some((b, o, true)) if o < b.len() => {
                b[o] = v;
                self.written(a, 1);
                Ok(())
            }
            _ => {
                self.last_fault = Some((a, true));
                Err(Fault::Prohibited)
            }
        }
    }
    fn write16(&mut self, a: u32, v: u16) -> Result<(), Fault> {
        if Self::is_periph(a) {
            self.periph_write(a, v as u32, 2);
            return Ok(());
        }
        match self.memory(a) {
            Some((b, o, true)) if o + 2 <= b.len() => {
                b[o..o + 2].copy_from_slice(&v.to_le_bytes());
                self.written(a, 2);
                Ok(())
            }
            _ => {
                self.last_fault = Some((a, true));
                Err(Fault::Prohibited)
            }
        }
    }
    fn write32(&mut self, a: u32, v: u32) -> Result<(), Fault> {
        if Self::is_periph(a) {
            self.periph_write(a, v, 4);
            return Ok(());
        }
        match self.memory(a) {
            Some((b, o, true)) if o + 4 <= b.len() => {
                b[o..o + 4].copy_from_slice(&v.to_le_bytes());
                self.written(a, 4);
                Ok(())
            }
            _ => {
                self.last_fault = Some((a, true));
                Err(Fault::Prohibited)
            }
        }
    }
    fn fetch(&mut self, pc: u32) -> Result<[u8; 4], Fault> {
        match self.memory(pc) {
            Some((b, o, _)) if o < b.len() => {
                let mut out = [0; 4];
                for (i, v) in out.iter_mut().enumerate() {
                    if let Some(x) = b.get(o + i) {
                        *v = *x;
                    }
                }
                Ok(out)
            }
            _ => {
                self.last_fault = Some((pc, false));
                Err(Fault::Unmapped)
            }
        }
    }
    fn page_versions(&self) -> &[u32] {
        &self.page_ver
    }
    fn code_page(&mut self, pc: u32) -> u32 {
        Self::version_page(pc) as u32
    }
    fn note_pc(&mut self, pc: u32) {
        self.periph.misc.cur_pc = pc;
    }
    fn block_break(&self) -> bool {
        self.irq_dirty
    }
    fn tick(&mut self, cycles: u32) -> u32 {
        self.cycles += cycles as u64;
        self.deliver_board_inputs();
        for input in self.board.uart_rx(self.cycles) { self.periph.uart_pin_input(&input); self.irq_dirty = true; }
        self.periph.tick(cycles as u64);
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pro_mmu_maps_classic_drom_and_irom() {
        let mut b = SocBus::new(4 << 20, [0; 6]);
        b.flash[0x12345] = 0x5a;
        b.mmu[0][0] = 1;
        b.mmu[0][77] = 1;
        assert_eq!(b.read8(DBUS_LOW + 0x2345), Ok(0x5a));
        assert_eq!(b.read8(0x4000_0000 + 0x2345), Ok(b.irom[0x2345])); // mask ROM wins below app IROM
        assert_eq!(b.read8(IBUS_LOW + 0x2345), Ok(0x5a));
    }

    #[test]
    fn analog_config_register_is_read_write() {
        let mut b = SocBus::new(4 << 20, [0; 6]);
        b.write32(0x6000_e044, 0x1234).unwrap();
        assert_eq!(b.read32(0x6000_e044), Ok(0x1234));
    }

    #[test]
    fn uart_ahb_alias_reaches_fifo() {
        let mut b = SocBus::new(4 << 20, [0; 6]);
        b.write32(UART_FIFO_AHB[0], b'X' as u32).unwrap();
        assert_eq!(b.periph.uart[0].tx_out, b"X");
    }


    #[test]
    fn reboot_preserves_rtc_reset_hint_and_publishes_cause() {
        let mut b = SocBus::new(4 << 20, [0; 6]);
        let hint = 0x8006_0006;
        b.periph.rtc.0.ram.write(0xc8, hint);
        b.periph.rtc.0.reset_cause = 12;
        assert_eq!(esp_soc::SocBus::reboot(&mut b, [0; 6]), 12);
        assert_eq!(b.read32(0x3ff4_80b8), Ok(hint));
        assert_eq!(b.read32(0x3ff4_8034), Ok(12 | (12 << 6)));
    }
    #[test]
    fn stores_invalidate_only_the_written_code_pages_and_rtc_alias() {
        let mut b = SocBus::new(4 << 20, [0; 6]);
        let rom = b.code_page(IROM_MASK_LOW) as usize;
        let code = b.code_page(IRAM_LOW + 0x100) as usize;
        b.write32(DRAM_LOW + 0x100, 42).unwrap();
        assert_eq!(b.page_versions()[rom], 0);
        assert_eq!(b.page_versions()[code], 0);
        b.write16(IRAM_LOW + 0x1ff, 42).unwrap();
        assert_eq!(b.page_versions()[code], 1);
        assert_eq!(b.page_versions()[code + 1], 1);
        b.write8(IRAM_LOW + 0x200, 42).unwrap();
        assert_eq!(b.page_versions()[code], 2, "straddling instruction on previous page");
        let rtc = b.code_page(RTC_FAST_I) as usize;
        b.write32(RTC_FAST_D, 42).unwrap();
        assert_eq!(b.code_page(RTC_FAST_D) as usize, rtc);
        assert_eq!(b.page_versions()[rtc], 1);
    }
    #[test]
    fn flash_remapping_and_programming_invalidate_aliases_without_touching_rom() {
        let mut b = SocBus::new(4 << 20, [0; 6]);
        let rom = b.code_page(IROM_MASK_LOW) as usize;
        b.write32(MMU_PRO + 77 * 4, 1).unwrap();
        b.write32(MMU_PRO + 78 * 4, 1).unwrap();
        let code = b.code_page(IBUS_LOW + 0x100) as usize;
        let alias = b.code_page(IBUS_LOW + PAGE as u32 + 0x100) as usize;
        let before = b.page_versions()[code];
        b.write_flash(PAGE + 0x100, &[42]).unwrap();
        assert!(b.page_versions()[code] > before);
        assert_eq!(b.page_versions()[code], b.page_versions()[alias]);
        let before = b.page_versions()[code];
        b.write32(MMU_PRO + 77 * 4, 2).unwrap();
        assert!(b.page_versions()[code] > before);
        assert_eq!(b.page_versions()[rom], 0);
    }

}
