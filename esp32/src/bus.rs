//! Classic ESP32 memory map and flash-cache MMUs.
use crate::periph::{Peripherals, PERIPH_BASE, PERIPH_END};
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
const MMU_PRO: u32 = 0x3ff1_0000;
const MMU_APP: u32 = 0x3ff1_2000;
const MMU_INVALID: u32 = 1 << 8;
const PAGE: usize = 0x1_0000;

pub struct SocBus {
    pub dram: Vec<u8>,
    pub iram: Vec<u8>,
    pub cache_ram: Vec<u8>,
    pub irom: Vec<u8>,
    pub drom: Vec<u8>,
    pub rtc_fast: Vec<u8>,
    pub rtc_slow: Vec<u8>,
    pub flash: Vec<u8>,
    pub mmu: [[u32; 2048]; 2],
    pub periph: Peripherals,
    pub board: esp_soc::Board,
    pub cycles: u64,
    pub last_fault: Option<(u32, bool)>,
    pub irq_dirty: bool,
    pub gpio_events: Option<Vec<(u64, u8, bool)>>,
    pub debug: esp_soc::DebugFlags,
    ver: u32,
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
            mmu: [[MMU_INVALID; 2048]; 2],
            periph: Peripherals::new(mac),
            board: Box::new(esp_soc::NoBoard),
            cycles: 0,
            last_fault: None,
            irq_dirty: true,
            gpio_events: None,
            debug: Default::default(),
            ver: 0,
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
        if let Some((cpu, n)) = Self::mmu_slot(addr) {
            return self.mmu[cpu][n];
        }
        let w = self.periph.read32(addr & !3);
        match size {
            1 => (w >> ((addr & 3) * 8)) & 0xff,
            2 => (w >> ((addr & 2) * 8)) & 0xffff,
            _ => w,
        }
    }
    fn periph_write(&mut self, addr: u32, value: u32, size: u32) {
        if let Some((cpu, n)) = Self::mmu_slot(addr) {
            self.mmu[cpu][n] = value & 0x1ff;
            self.ver = self.ver.wrapping_add(1);
            return;
        }
        let a = addr & !3;
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
        self.periph.write32(a, v);
        if self.periph.spi_exec {
            self.run_spi();
        }
        if !self.periph.gpio.0.changes.is_empty() {
            let changes = std::mem::take(&mut self.periph.gpio.0.changes);
            if let Some(events) = &mut self.gpio_events {
                for &(pin, level) in &changes {
                    events.push((self.cycles, pin, level));
                }
            }
            self.board.gpio_changes(&changes);
        }
        self.irq_dirty = true;
    }
    fn run_spi(&mut self) {
        self.periph.spi_exec = false;
        self.periph.spi1.0.execute(&mut self.flash, &mut []);
        self.periph.spi1.0.dirty.clear();
    }
    pub fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> {
        let target = self
            .flash
            .get_mut(offset..)
            .and_then(|d| d.get_mut(..data.len()))
            .ok_or("flash image too large")?;
        target.copy_from_slice(data);
        self.ver = self.ver.wrapping_add(1);
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
        self.ver = self.ver.wrapping_add(1);
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
                self.ver = self.ver.wrapping_add(1);
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
                self.ver = self.ver.wrapping_add(1);
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
                self.ver = self.ver.wrapping_add(1);
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
        std::slice::from_ref(&self.ver)
    }
    fn code_page(&mut self, _pc: u32) -> u32 {
        0
    }
    fn note_pc(&mut self, pc: u32) {
        self.periph.misc.cur_pc = pc;
    }
    fn block_break(&self) -> bool {
        self.irq_dirty
    }
    fn tick(&mut self, cycles: u32) -> u32 {
        self.cycles += cycles as u64;
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
}
