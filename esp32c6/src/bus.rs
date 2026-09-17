//! ESP32-C6 memory map.
//!
//! One address space for instructions and data: the 320 KB mask ROM at `0x4000_0000`, 512 KB of
//! HP SRAM at `0x4080_0000`, 16 KB of LP SRAM at `0x5000_0000`, and a single 16 MB flash cache
//! window at `0x4200_0000` behind a 256-entry MMU. The MMU is programmed through two SPI0
//! registers (item index / item content), not a memory-mapped table as on the C3.

use crate::periph::{Peripherals, CPU_SUB_BASE, CPU_SUB_END, PERIPH_BASE, PERIPH_END};
use esp_periph::read_desc;
use esp_soc::StateFile;
use riscv_rv32::bus::{Bus, Fault};
use std::path::PathBuf;

/// GPIO matrix output signal of RMT TX channel 0 (soc/gpio_sig_map.h); channel n is this + n.
pub const RMT_SIG_OUT0: u32 = 71;
pub const ROM_LOW: u32 = 0x4000_0000;
pub const ROM_HIGH: u32 = 0x4005_0000;
pub const SRAM_LOW: u32 = 0x4080_0000;
pub const SRAM_HIGH: u32 = 0x4088_0000;
pub const LP_SRAM_LOW: u32 = 0x5000_0000;
pub const LP_SRAM_HIGH: u32 = 0x5000_4000;
pub const FLASH_LOW: u32 = 0x4200_0000;
pub const FLASH_HIGH: u32 = 0x4300_0000;
pub const MMU_ENTRIES: usize = 256;
/// bit 9 marks an entry valid; bits 8:0 are the flash page
pub const MMU_VALID: u32 = 1 << 9;
/// SPI0 registers the MMU is driven through
pub const SPI_MMU_ITEM_CONTENT: u32 = 0x37c;
pub const SPI_MMU_ITEM_INDEX: u32 = 0x380;
pub const SPI_MMU_POWER_CTRL: u32 = 0x384;

pub struct SocBus {
    pub rom: Vec<u8>,
    pub sram: Vec<u8>,
    pub lp_sram: Vec<u8>,
    pub flash: Vec<u8>,
    pub mmu: [u32; MMU_ENTRIES],
    pub mmu_index: u32,
    /// SPI_MEM_MMU_POWER_CTRL: bits 4:3 select the page size (0 = 64 KB, 1 = 32, 2 = 16, 3 = 8)
    pub mmu_power_ctrl: u32,
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
            rom: vec![0; (ROM_HIGH - ROM_LOW) as usize],
            sram: vec![0; (SRAM_HIGH - SRAM_LOW) as usize],
            lp_sram: vec![0; (LP_SRAM_HIGH - LP_SRAM_LOW) as usize],
            flash: vec![0xff; flash_size],
            mmu: [0; MMU_ENTRIES], mmu_index: 0, mmu_power_ctrl: 0,
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

    /// log2 of the MMU page size
    #[inline]
    pub fn page_shift(&self) -> u32 { 16 - ((self.mmu_power_ctrl >> 3) & 3) }

    /// Resolve to (buffer, offset, writable). The flash window goes through the MMU.
    fn resolve(&mut self, addr: u32) -> Option<(&mut Vec<u8>, usize, bool)> {
        match addr {
            SRAM_LOW..=0x4087_FFFF => Some((&mut self.sram, (addr - SRAM_LOW) as usize, true)),
            ROM_LOW..=0x4004_FFFF => Some((&mut self.rom, (addr - ROM_LOW) as usize, false)),
            LP_SRAM_LOW..=0x5000_3FFF => Some((&mut self.lp_sram, (addr - LP_SRAM_LOW) as usize, true)),
            FLASH_LOW..=0x42FF_FFFF => {
                let shift = self.page_shift();
                let idx = ((addr - FLASH_LOW) >> shift) as usize;
                if idx >= MMU_ENTRIES { return None; }
                let entry = self.mmu[idx];
                if entry & MMU_VALID == 0 { return None; }
                let off = (((entry & 0x1ff) as usize) << shift) + (addr & ((1 << shift) - 1)) as usize;
                if off < self.flash.len() { Some((&mut self.flash, off, false)) } else { None }
            }
            _ => None,
        }
    }

    #[inline]
    fn is_periph(addr: u32) -> bool { (PERIPH_BASE..PERIPH_END).contains(&addr) || (CPU_SUB_BASE..CPU_SUB_END).contains(&addr) }

    fn periph_read(&mut self, addr: u32, size: u32) -> u32 {
        let w = if (CPU_SUB_BASE..CPU_SUB_END).contains(&addr) {
            self.periph.cpu_sub_read(addr - CPU_SUB_BASE)
        } else if (addr & !0xfff) == PERIPH_BASE + 0x2000 && matches!(addr & 0xfff, SPI_MMU_ITEM_CONTENT | SPI_MMU_ITEM_INDEX | SPI_MMU_POWER_CTRL) {
            match addr & 0xfff {
                SPI_MMU_ITEM_CONTENT => self.mmu[(self.mmu_index as usize) & (MMU_ENTRIES - 1)],
                SPI_MMU_ITEM_INDEX => self.mmu_index,
                _ => self.mmu_power_ctrl,
            }
        } else {
            self.periph.read32(addr & !3)
        };
        if (PERIPH_BASE..PERIPH_END).contains(&addr) && addr & 0xfff == 0 && matches!((addr - PERIPH_BASE) >> 12, 0x00 | 0x10) { self.irq_dirty = true; }
        match size { 1 => (w >> ((addr & 3) * 8)) & 0xff, 2 => (w >> ((addr & 2) * 8)) & 0xffff, _ => w }
    }

    fn periph_write(&mut self, addr: u32, v: u32, size: u32) {
        let a = addr & !3;
        let merge = |old: u32| match size {
            4 => v,
            1 => { let sh = (addr & 3) * 8; (old & !(0xff << sh)) | ((v & 0xff) << sh) }
            _ => { let sh = (addr & 2) * 8; (old & !(0xffff << sh)) | ((v & 0xffff) << sh) }
        };
        if (CPU_SUB_BASE..CPU_SUB_END).contains(&a) {
            let old = self.periph.cpu_sub_read(a - CPU_SUB_BASE);
            self.periph.cpu_sub_write(a - CPU_SUB_BASE, merge(old));
            self.irq_dirty = true;
            return;
        }
        if (a & !0xfff) == PERIPH_BASE + 0x2000 {
            match a & 0xfff {
                SPI_MMU_ITEM_CONTENT => { self.mmu[(self.mmu_index as usize) & (MMU_ENTRIES - 1)] = merge(self.mmu[(self.mmu_index as usize) & (MMU_ENTRIES - 1)]) & 0x7ff; return; }
                SPI_MMU_ITEM_INDEX => { self.mmu_index = merge(self.mmu_index) & 0xff; return; }
                SPI_MMU_POWER_CTRL => { self.mmu_power_ctrl = merge(self.mmu_power_ctrl); return; }
                _ => {}
            }
        }
        let v = if size == 4 { v } else { merge(self.periph.read32(a)) };
        self.periph.write32(a, v);
        // A SPI flash command must complete before the guest reads its result (see the C3 notes:
        // running it at the quantum boundary loses the race and reads back zeros).
        if self.periph.spi_exec { self.run_spi(); }
        // TX_START: the radio's DMA reads the frame now, at the instruction that started it
        if self.periph.radio.tx_request.is_some() { self.radio_tx_fetch(); }
        // A GP-SPI transfer reaches the board now, after the GPIO edges that preceded it: the
        // display's D/C line is a GPIO the driver sets right before each transaction.
        if self.periph.spi2.dma_tx_pending.is_some() { self.spi2_dma_tx(); }
        if self.periph.spi2.has_pending_transfer() || !self.periph.gpio.changes.is_empty() { self.deliver_board_events(); }
        self.irq_dirty = true;
    }

    /// A word of SRAM for the DMA engines (descriptors and buffers live there).
    fn sram32(&self, addr: u32) -> u32 {
        let o = addr.wrapping_sub(SRAM_LOW) as usize;
        if o + 4 <= self.sram.len() { u32::from_le_bytes(self.sram[o..o + 4].try_into().unwrap()) } else { 0 }
    }

    /// The 802.15.4 TX DMA: `buf[0]` is the PSDU length including the 2-byte FCS the hardware
    /// appends, `buf[1..]` the MAC frame. A buffer outside SRAM or a length under 2 is a driver
    /// bug on real silicon too; here it is reported and the transmission carries no bytes.
    fn radio_tx_fetch(&mut self) {
        let Some(addr) = self.periph.radio.tx_request.take() else { return };
        let o = addr.wrapping_sub(SRAM_LOW) as usize;
        let psdu = match self.sram.get(o) {
            Some(&len) => {
                let mac = (len & 0x7f).saturating_sub(2) as usize;
                if o + 1 + mac <= self.sram.len() { self.sram[o + 1..o + 1 + mac].to_vec() } else { Vec::new() }
            }
            None => Vec::new(),
        };
        if psdu.is_empty() { eprintln!("[802.15.4] TX_START with an empty or unmapped frame at {:#010x}", addr); }
        self.periph.radio.tx_loaded(psdu);
    }

    /// The 802.15.4 RX DMA: the completed frame, RSSI and LQI go where `DMA_RX_ADDR` points.
    fn radio_rx_store(&mut self) {
        let Some((addr, buf)) = self.periph.radio.rx_write.take() else { return };
        let o = addr.wrapping_sub(SRAM_LOW) as usize;
        if o + buf.len() <= self.sram.len() { self.sram[o..o + buf.len()].copy_from_slice(&buf); }
        else { eprintln!("[802.15.4] RX_DONE: DMA_RX_ADDR {:#010x} is not in SRAM, frame lost", addr); }
    }

    /// A frame from the medium (MAC header + payload, no FCS): started `Some(cycles)` ago (RX_DONE
    /// when its air time is up) or complete now (`None`; the buffer is written before the next
    /// instruction). Returns whether the radio took it; the interrupt lines are re-derived before
    /// the next instruction either way.
    pub fn radio_receive(&mut self, frame: &[u8], rssi: i8, lqi: u8, started_ago: Option<u64>) -> bool {
        let taken = self.periph.radio.receive(frame, rssi, lqi, started_ago);
        if self.periph.radio.rx_write.is_some() { self.radio_rx_store(); }
        self.irq_dirty = true;
        taken
    }

    /// GP-SPI2's MOSI data phase through the GDMA out-channel bound to it (trigger 0): walk the
    /// descriptor chain, hand the bytes to the SPI model, and complete the channel.
    fn spi2_dma_tx(&mut self) {
        let Some(bits) = self.periph.spi2.dma_tx_pending else { return };
        let want = (bits as usize).div_ceil(8);
        let Some(ch) = self.periph.gdma.gdma.out_channel_for(0) else { return };   // not started yet: the round's end retries
        let mut data = Vec::with_capacity(want);
        let (mut desc, mut last) = (self.periph.gdma.gdma.out[ch].desc, self.periph.gdma.gdma.out[ch].desc);
        let mut guard = 0;
        while desc != 0 && data.len() < want && guard < 4096 {
            guard += 1;
            let d = read_desc(&|a| self.sram32(a), desc);
            let n = (d.length as usize).min(want - data.len());
            let o = d.buf.wrapping_sub(SRAM_LOW) as usize;
            if o + n <= self.sram.len() { data.extend_from_slice(&self.sram[o..o + n]); } else { break; }
            last = desc;
            if d.eof { break; }
            desc = d.next;
        }
        let c = &mut self.periph.gdma.gdma.out[ch];
        c.running = false; c.desc = 0; c.eof_desc = last;
        c.int_raw |= (1 << 0) | (1 << 1) | (1 << 3);                 // OUT_DONE, OUT_EOF, OUT_TOTAL_EOF
        self.periph.spi2.complete_dma_tx(&data);
    }

    /// Pin-level events to the board, in order: GPIO edges first, then what went out on the
    /// SPI, then completed RMT frames.
    fn deliver_board_events(&mut self) {
        if !self.periph.gpio.changes.is_empty() {
            let ch = std::mem::take(&mut self.periph.gpio.changes);
            if let Some(ev) = &mut self.gpio_events { for &(pin, level) in &ch { ev.push((self.cycles, pin, level)); } }
            self.board.gpio_changes(&ch);
        }
        if let Some(transfer) = self.periph.spi2.take_transfer() {
            let rx = self.board.spi_transfer(2, &transfer.tx, transfer.rx_len);
            self.periph.spi2.finish_transfer(transfer, &rx);
        }
        if !self.periph.rmt.rmt.done.is_empty() { for (ch, bits) in std::mem::take(&mut self.periph.rmt.rmt.done) { let pin = self.periph.gpio.pin_for_signal(RMT_SIG_OUT0 + ch as u32).unwrap_or(u8::MAX); self.board.rmt_frame(pin, &bits); } self.irq_dirty = true; }
    }

    /// Execute a pending SPI1 command against the flash image.
    fn run_spi(&mut self) {
        self.periph.spi_exec = false;
        let mut no_psram = Vec::new();
        self.periph.spi1.0.execute(&mut self.flash, &mut no_psram);
        for (memory, offset, len) in std::mem::take(&mut self.periph.spi1.0.dirty) {
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
        // The mask ROM ELF links its .eh_frame at the flash window; nothing ever reads it.
        if (FLASH_LOW..FLASH_HIGH).contains(&addr) { return Ok(()); }
        for (i, b) in data.iter().enumerate() {
            let a = addr.wrapping_add(i as u32);
            match self.resolve(a) {
                Some((buf, off, _)) if off < buf.len() => buf[off] = *b,
                _ => return Err(format!("load: address {:#010x} not mapped", a)),
            }
        }
        Ok(())
    }

    /// Run the SPI1 controller if the guest just kicked it, advance device time, deliver what
    /// the devices produced to the board.
    fn devices(&mut self, cycles: u32) {
        if self.periph.spi_exec { self.run_spi(); }
        self.periph.tick(cycles as u64);
        if self.periph.radio.rx_write.is_some() { self.radio_rx_store(); }
        if self.periph.spi2.dma_tx_pending.is_some() { self.spi2_dma_tx(); }
        self.deliver_board_events();
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
        std::env::temp_dir().join(format!("esp32sim-c6-flash-state-{}.bin", std::process::id()))
    }

    #[test]
    fn flash_program_survives_a_new_bus() {
        const SPI1: u32 = 0x6000_3000;
        let path = state_path(); let _ = std::fs::remove_file(&path);
        {
            let mut bus = SocBus::new(0x2000, [0; 6]);
            assert!(!bus.configure_flash_state(&path).unwrap()); bus.initialize_storage().unwrap();
            bus.periph.spi1.0.regs.write(0x4, 0x120); bus.periph.spi1.0.regs.write(0x24, 31); bus.periph.spi1.0.w[0] = 0x4433_2211;
            bus.write32(SPI1, 1 << 25).unwrap(); bus.flush_storage().unwrap();
        }
        let mut bus = SocBus::new(0x2000, [0; 6]); assert!(bus.configure_flash_state(&path).unwrap());
        assert_eq!(&bus.flash[0x120..0x124], &[0x11, 0x22, 0x33, 0x44]); std::fs::remove_file(path).unwrap();
    }
}
