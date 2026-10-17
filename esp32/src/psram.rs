//! Classic external data RAM translation. Cache/device gates are integrated separately.
//! ESP-IDF v4.4.7 and v5.1.4 components/esp_hw_support/port/esp32/cache_sram_mmu.c:
//! PID 0/1 use table index 1152, 32 KiB pages (shift 15), at 0x3f800000.
//! Matched live PRO/APP tables are an emulator qualification restriction, not silicon behavior.
use crate::bus::SocBus;
use xtensa_lx7::bus::Fault;

pub(crate) const LOW: u32 = 0x3f80_0000;
pub(crate) const HIGH: u32 = 0x3fc0_0000;
const SRAM_SLOT: usize = 1152;
const SRAM_SHIFT: u32 = 15;
const SRAM_PAGE: usize = 1 << SRAM_SHIFT;

impl SocBus {
    // Resolve the whole access before touching bytes, including noncontiguous page crossings.
    fn psram_offsets<const N: usize>(&self, addr: u32) -> Result<[usize; N], Fault> {
        let mut out = [0; N];
        for (i, off) in out.iter_mut().enumerate() {
            let a = addr.checked_add(i as u32).filter(|a| (LOW..HIGH).contains(a)).ok_or(Fault::Unmapped)?;
            let slot = SRAM_SLOT + ((a - LOW) >> SRAM_SHIFT) as usize;
            let entry = self.mmu[0][slot];
            let (clock, reset, stall) = self.periph.dport.core1_control();
            let app_live = clock && !reset && !stall;
            if entry & 0x100 != 0 || (app_live && entry != self.mmu[1][slot]) { return Err(Fault::Unmapped); }
            *off = (entry as usize & 255) * SRAM_PAGE + (a as usize & (SRAM_PAGE - 1));
            if *off >= self.psram.len() { return Err(Fault::Unmapped); }
        }
        Ok(out)
    }
    #[cold]
    pub(crate) fn psram_load<const N: usize>(&mut self, addr: u32) -> Result<[u8; N], Fault> {
        match self.psram_offsets::<N>(addr) {
            Ok(offsets) => Ok(offsets.map(|o| self.psram[o])),
            Err(f) => { self.last_fault = Some((addr, false)); Err(f) }
        }
    }
    #[cold]
    pub(crate) fn psram_store<const N: usize>(&mut self, addr: u32, bytes: &[u8; N]) -> Result<(), Fault> {
        match self.psram_offsets::<N>(addr) {
            Ok(offsets) => {
                for (off, byte) in offsets.into_iter().zip(bytes) { self.psram[off] = *byte; }
                Ok(())
            }
            Err(f) => { self.last_fault = Some((addr, true)); Err(f) }
        }
    }
    pub(crate) fn reset_psram(&mut self) {
        self.psram.fill(0);
        for table in &mut self.mmu { table[SRAM_SLOT..SRAM_SLOT + 128].fill(0x100); }
    }
    pub(crate) fn configure_psram(&mut self, bytes: usize) -> Result<(), String> {
        if bytes != 0 && bytes != (4 << 20) { return Err("classic PSRAM supports 0 or 4194304 bytes".into()); }
        // Stopped-machine boundary only. Classic has no fast_mem or published backing pointer.
        self.reset_psram();
        self.psram = vec![0; bytes];
        Ok(())
    }
}

#[cfg(test)]
#[path = "psram_tests.rs"]
mod tests;
