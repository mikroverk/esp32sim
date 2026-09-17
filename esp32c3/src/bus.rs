//! ESP32-C3 memory map.
//!
//! Simpler than the S3's: one core, no PSRAM, an 8 MB cache window per bus and a flat 128-entry
//! MMU. SRAM1 is dual-mapped (IRAM `0x4038_0000` and DRAM `0x3FC8_0000` are the same bytes);
//! SRAM0 below it is the instruction cache's, reachable only from the instruction bus.

use crate::periph::{Peripherals, PERIPH_BASE, PERIPH_END};
use esp_soc::StateFile;
use riscv_rv32::bus::{Bus, Fault};
use std::path::PathBuf;

pub const SRAM_SIZE: usize = 400 * 1024;
pub const IRAM_LOW: u32 = 0x4037_C000;
pub const IRAM_HIGH: u32 = 0x403E_0000;
pub const DRAM_LOW: u32 = 0x3FC8_0000;
pub const DRAM_HIGH: u32 = 0x3FCE_0000;
/// SRAM1 starts 16 KiB into the buffer: SRAM0 in front of it is instruction-bus only.
pub const DRAM_IN_SRAM: usize = 0x4000;
pub const IROM_MASK_LOW: u32 = 0x4000_0000;
pub const IROM_MASK_HIGH: u32 = 0x4006_0000;
pub const DROM_MASK_LOW: u32 = 0x3FF0_0000;
pub const DROM_MASK_HIGH: u32 = 0x3FF2_0000;
pub const RTC_SLOW_LOW: u32 = 0x5000_0000;
pub const RTC_SLOW_HIGH: u32 = 0x5000_2000;
pub const DBUS_LOW: u32 = 0x3C00_0000;
pub const DBUS_HIGH: u32 = 0x3C80_0000;
pub const IBUS_LOW: u32 = 0x4200_0000;
pub const IBUS_HIGH: u32 = 0x4280_0000;
pub const MMU_TABLE: u32 = 0x600C_5000;
pub const MMU_ENTRIES: usize = 128;
/// bit 8 marks an entry invalid; bits 7:0 are the 64 KiB flash page
pub const MMU_INVALID: u32 = 1 << 8;
pub const PAGE: u32 = 0x1_0000;

pub struct SocBus {
    pub sram: Vec<u8>,
    pub irom: Vec<u8>,
    pub drom: Vec<u8>,
    pub rtc_slow: Vec<u8>,
    pub flash: Vec<u8>,
    pub mmu: [u32; MMU_ENTRIES],
    pub periph: Peripherals,
    /// a bare module: nothing on the pins
    pub board: esp_soc::Board,
    pub cycles: u64,
    pub last_fault: Option<(u32, bool)>,
    /// a peripheral write may have moved an interrupt line: re-derive before the next instruction
    pub irq_dirty: bool,
    /// GPIO edges for observers, while one wants them: (cycle, pin, level)
    pub gpio_events: Option<Vec<(u64, u8, bool)>>,
    pub debug: esp_soc::DebugFlags,
    flash_state: Option<StateSlot>,
    storage_error: Option<String>,
    storage_generation: u64,
}

struct StateSlot { path: PathBuf, file: Option<StateFile>, loaded: bool }

impl SocBus {
    pub fn new(flash_size: usize, mac: [u8; 6]) -> Self {
        SocBus {
            sram: vec![0; SRAM_SIZE],
            irom: vec![0; (IROM_MASK_HIGH - IROM_MASK_LOW) as usize],
            drom: vec![0; (DROM_MASK_HIGH - DROM_MASK_LOW) as usize],
            rtc_slow: vec![0; (RTC_SLOW_HIGH - RTC_SLOW_LOW) as usize],
            flash: vec![0xff; flash_size],
            mmu: [MMU_INVALID; MMU_ENTRIES],
            periph: Peripherals::new(mac), board: Box::new(esp_soc::NoBoard),
            cycles: 0, last_fault: None, irq_dirty: true, gpio_events: None, debug: Default::default(),
            flash_state: None, storage_error: None, storage_generation: 0,
        }
    }

    pub fn configure_flash_state(&mut self, path: impl Into<PathBuf>) -> Result<bool, String> {
        let path = path.into();
        match StateFile::open(&path, self.flash.len())? {
            Some((file, bytes)) => { self.flash = bytes; self.flash_state = Some(StateSlot { path, file: Some(file), loaded: true }); Ok(true) }
            None => { self.flash_state = Some(StateSlot { path, file: None, loaded: false }); Ok(false) }
        }
    }

    pub fn persistent_flash_loaded(&self) -> bool { self.flash_state.as_ref().is_some_and(|state| state.loaded) }
    pub fn initialize_storage(&mut self) -> Result<(), String> {
        if let Some(state) = &mut self.flash_state {
            if state.file.is_none() { state.file = Some(StateFile::create(&state.path, &self.flash)?); }
        }
        Ok(())
    }
    pub fn flush_storage(&mut self) -> Result<(), String> {
        if let Some(file) = self.flash_state.as_mut().and_then(|state| state.file.as_mut()) { file.sync()?; }
        if let Some(error) = self.storage_error.take() { return Err(error); }
        Ok(())
    }
    pub fn storage_generation(&self) -> u64 { self.storage_generation }
    pub fn export_storage(&self, kind: u32) -> Option<Vec<u8>> { (kind == 0).then(|| self.flash.clone()) }
    pub fn import_storage(&mut self, kind: u32, data: &[u8]) -> Result<(), String> {
        if kind != 0 { return Err(format!("unknown persistent state kind {}", kind)); }
        if data.len() != self.flash.len() { return Err(format!("flash state is {} bytes, expected {}", data.len(), self.flash.len())); }
        self.flash.copy_from_slice(data);
        Ok(())
    }
    fn persist_flash(&mut self, offset: usize, len: usize) {
        let end = offset.saturating_add(len).min(self.flash.len());
        if end <= offset { return; }
        self.storage_generation = self.storage_generation.wrapping_add(1);
        if let Some(file) = self.flash_state.as_mut().and_then(|state| state.file.as_mut()) {
            if let Err(error) = file.write_range(offset, &self.flash[offset..end]) { self.storage_error.get_or_insert(error); }
        }
    }

    /// Resolve to (buffer, offset, writable). Cache windows go through the MMU.
    fn resolve(&mut self, addr: u32) -> Option<(&mut Vec<u8>, usize, bool)> {
        match addr {
            DRAM_LOW..=0x3FCD_FFFF => Some((&mut self.sram, (addr - DRAM_LOW) as usize + DRAM_IN_SRAM, true)),
            IRAM_LOW..=0x403D_FFFF => Some((&mut self.sram, (addr - IRAM_LOW) as usize, true)),
            IROM_MASK_LOW..=0x4005_FFFF => Some((&mut self.irom, (addr - IROM_MASK_LOW) as usize, false)),
            DROM_MASK_LOW..=0x3FF1_FFFF => Some((&mut self.drom, (addr - DROM_MASK_LOW) as usize, false)),
            RTC_SLOW_LOW..=0x5000_1FFF => Some((&mut self.rtc_slow, (addr - RTC_SLOW_LOW) as usize, true)),
            DBUS_LOW..=0x3C7F_FFFF | IBUS_LOW..=0x427F_FFFF => {
                // both buses index one flat table; software keeps their page ranges disjoint
                let entry = self.mmu[((addr & 0x7F_FFFF) >> 16) as usize];
                if entry & MMU_INVALID != 0 { return None; }
                let off = (entry & 0xff) as usize * PAGE as usize + (addr & 0xffff) as usize;
                if off < self.flash.len() { Some((&mut self.flash, off, false)) } else { None }
            }
            _ => None,
        }
    }

    #[inline]
    fn is_periph(addr: u32) -> bool { (PERIPH_BASE..PERIPH_END).contains(&addr) }

    fn periph_read(&mut self, addr: u32, size: u32) -> u32 {
        if (MMU_TABLE..MMU_TABLE + (MMU_ENTRIES as u32) * 4).contains(&addr) {
            return self.mmu[((addr - MMU_TABLE) >> 2) as usize];
        }
        let w = self.periph.read32(addr & !3);
        if addr & 0xfff == 0 && matches!((addr - PERIPH_BASE) >> 12, 0x00 | 0x10) { self.irq_dirty = true; }
        match size { 1 => (w >> ((addr & 3) * 8)) & 0xff, 2 => (w >> ((addr & 2) * 8)) & 0xffff, _ => w }
    }

    fn periph_write(&mut self, addr: u32, v: u32, size: u32) {
        if (MMU_TABLE..MMU_TABLE + (MMU_ENTRIES as u32) * 4).contains(&addr) {
            self.mmu[((addr - MMU_TABLE) >> 2) as usize] = v & 0x1ff;
            return;
        }
        let a = addr & !3;
        let v = match size {
            4 => v,
            1 => { let old = self.periph.read32(a); let sh = (addr & 3) * 8; (old & !(0xff << sh)) | ((v & 0xff) << sh) }
            _ => { let old = self.periph.read32(a); let sh = (addr & 2) * 8; (old & !(0xffff << sh)) | ((v & 0xffff) << sh) }
        };
        self.periph.write32(a, v);
        // A SPI flash command must complete before the guest can read its result: firmware kicks
        // the command and polls/reads the data registers a few instructions later, well inside one
        // scheduling quantum. Running it at the quantum boundary instead loses the race and the
        // read returns zeros — which is exactly how `E memspi: no response` showed up on a
        // non-power-on boot while a power-on boot happened to survive it.
        if self.periph.spi_exec { self.run_spi(); }
        self.irq_dirty = true;
    }

    /// Execute a pending SPI1 command against the flash image.
    fn run_spi(&mut self) {
        self.periph.spi_exec = false;
        let mut no_psram = Vec::new();
        self.periph.spi1.execute(&mut self.flash, &mut no_psram);
        for (memory, offset, len) in std::mem::take(&mut self.periph.spi1.dirty) {
            if memory == esp_periph::DirtyMem::Flash { self.persist_flash(offset, len); }
        }
    }

    /// Write straight into flash (image loaders, not the guest).
    pub fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> {
        if offset + data.len() > self.flash.len() { return Err("flash image too large".into()); }
        self.flash[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    pub fn load_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        for (i, b) in data.iter().enumerate() {
            let a = addr.wrapping_add(i as u32);
            match self.resolve(a) {
                Some((buf, off, _)) if off < buf.len() => buf[off] = *b,
                _ => return Err(format!("load: address {:#010x} not mapped", a)),
            }
        }
        Ok(())
    }

    /// Run the SPI1 controller if the guest just kicked it, then advance device time.
    fn devices(&mut self, cycles: u32) {
        if self.periph.spi_exec { self.run_spi(); }
        self.periph.tick(cycles as u64);
    }
}

macro_rules! rd {
    ($self:ident, $addr:expr, $n:expr, $conv:expr) => {{
        let addr = $addr;
        match $self.resolve(addr) {
            Some((b, o, _)) if b.len().saturating_sub(o) >= $n => Ok($conv(&b[o..o + $n])),
            _ => { $self.last_fault = Some((addr, false)); Err(Fault::Unmapped) }
        }
    }};
}

impl Bus for SocBus {
    fn read8(&mut self, addr: u32) -> Result<u8, Fault> {
        if Self::is_periph(addr) { return Ok(self.periph_read(addr, 1) as u8); }
        rd!(self, addr, 1, |b: &[u8]| b[0])
    }
    fn read16(&mut self, addr: u32) -> Result<u16, Fault> {
        if Self::is_periph(addr) { return Ok(self.periph_read(addr, 2) as u16); }
        rd!(self, addr, 2, |b: &[u8]| u16::from_le_bytes(b.try_into().unwrap()))
    }
    fn read32(&mut self, addr: u32) -> Result<u32, Fault> {
        if Self::is_periph(addr) { return Ok(self.periph_read(addr, 4)); }
        rd!(self, addr, 4, |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn write8(&mut self, addr: u32, v: u8) -> Result<(), Fault> {
        if Self::is_periph(addr) { self.periph_write(addr, v as u32, 1); return Ok(()); }
        match self.resolve(addr) {
            Some((b, o, true)) if o < b.len() => { b[o] = v; Ok(()) }
            _ => { self.last_fault = Some((addr, true)); Err(Fault::Prohibited) }
        }
    }
    fn write16(&mut self, addr: u32, v: u16) -> Result<(), Fault> {
        if Self::is_periph(addr) { self.periph_write(addr, v as u32, 2); return Ok(()); }
        match self.resolve(addr) {
            Some((b, o, true)) if o + 2 <= b.len() => { b[o..o + 2].copy_from_slice(&v.to_le_bytes()); Ok(()) }
            _ => { self.last_fault = Some((addr, true)); Err(Fault::Prohibited) }
        }
    }
    fn write32(&mut self, addr: u32, v: u32) -> Result<(), Fault> {
        if Self::is_periph(addr) { self.periph_write(addr, v, 4); return Ok(()); }
        match self.resolve(addr) {
            Some((b, o, true)) if o + 4 <= b.len() => { b[o..o + 4].copy_from_slice(&v.to_le_bytes()); Ok(()) }
            _ => { self.last_fault = Some((addr, true)); Err(Fault::Prohibited) }
        }
    }
    fn fetch(&mut self, pc: u32) -> Result<[u8; 4], Fault> {
        match self.resolve(pc) {
            Some((b, o, _)) if o < b.len() => {
                let mut r = [0u8; 4];
                for i in 0..4 { if o + i < b.len() { r[i] = b[o + i]; } }
                Ok(r)
            }
            _ => { self.last_fault = Some((pc, false)); Err(Fault::Unmapped) }
        }
    }
    fn tick(&mut self, cycles: u32) -> u32 {
        self.cycles += cycles as u64;
        self.devices(cycles);
        1
    }
    #[inline(always)]
    fn note_pc(&mut self, pc: u32) { self.periph.misc.cur_pc = pc; }
    /// a peripheral write may have moved a line: the core's run stops so the machine re-derives it
    #[inline(always)]
    fn block_break(&self) -> bool { self.irq_dirty }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_path() -> PathBuf {
        std::env::temp_dir().join(format!("esp32sim-c3-flash-state-{}.bin", std::process::id()))
    }

    #[test]
    fn flash_program_survives_a_new_bus() {
        const SPI1: u32 = 0x6000_2000;
        let path = state_path(); let _ = std::fs::remove_file(&path);
        {
            let mut bus = SocBus::new(0x2000, [0; 6]);
            assert!(!bus.configure_flash_state(&path).unwrap()); bus.initialize_storage().unwrap();
            bus.periph.spi1.regs.write(0x4, 0x120); bus.periph.spi1.regs.write(0x24, 31); bus.periph.spi1.w[0] = 0x4433_2211;
            bus.write32(SPI1, 1 << 25).unwrap(); bus.flush_storage().unwrap();
        }
        let mut bus = SocBus::new(0x2000, [0; 6]); assert!(bus.configure_flash_state(&path).unwrap());
        assert_eq!(&bus.flash[0x120..0x124], &[0x11, 0x22, 0x33, 0x44]); std::fs::remove_file(path).unwrap();
    }
}
