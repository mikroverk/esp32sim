//! S31 SHA register/block engine. ESP-IDF v6.1 fff9895c82d744c7237be8847347bdd1b07c6643:
//! soc/esp32s31/register/soc/{sha,hp_sys_clkrst}_reg.h and
//! esp_hal_security/esp32s31/include/hal/sha_ll.h. Shared compression only; S31 gates,
//! 16-bit DMA block count and algorithm limits are explicit. No SHA3/SM3. DMA mode gathers
//! from the AXI-GDMA OUT channel on PERI_SEL 5 (esp_hal_dma esp32s31 gdma_channel.h:32
//! SOC_GDMA_TRIG_PERIPH_SHA0, bus AXI at :64); the bus runs it (`S31Bus::sha_dma_step`).
//! Crypto root encoding follows S31 sec_ll.h at IDF6.1
//! 274d7ebeaf15cf397347877c34816ea9470d153f (absent at the older header revision).
use emu_core::Fault;
use esp_periph::sha::Sha;
pub const BASE: u32 = 0x2050_9000;
pub const CLOCK: u32 = 0x2058_7058;
pub struct ShaEngine { engine: Sha, clock: u32, dma_blocks: u32, date: u32, free: u32,
    /// SHA_IRQ_ENA bit 0 and the DMA-done raw interrupt (sha_reg.h:144-163, cleared by SHA_CLEAR_IRQ).
    int_ena: u32, int_raw: bool }
impl Default for ShaEngine {
    fn default() -> Self { Self { engine: Sha::new(), clock: 0x415555, dma_blocks: 0, date: 539297315, free: 1, int_ena: 0, int_raw: false } }
}
impl ShaEngine {
    pub fn clock_read(&self) -> u32 { self.clock }
    pub fn clock_source(&self) -> u32 { (self.clock >> 20) & 3 }
    pub fn clock_write(&mut self, value: u32) -> Result<(), Fault> {
        if (value >> 20) & 3 == 3 { return Err(Fault::Prohibited); } // Reserved by the S31 LL.
        self.clock = value & 0xf3ffff;
        // The official LL explicitly clears parent/DS/HMAC/ECDSA resets because they
        // hold SHA in reset. Include the enclosing security-domain reset as well.
        if value & 0x2888a != 0 { *self = Self { clock: self.clock, ..Self::default() }; }
        Ok(())
    }
    fn enabled(&self) -> bool { self.clock & (0x45 | 0x2888a) == 0x45 }
    pub fn blocks(&self) -> u64 { self.engine.blocks }
    /// ETS_SHA_INTR_SOURCE level: DMA done with SHA_INTERRUPT_ENA set.
    pub fn irq(&self) -> bool { self.int_raw && self.int_ena & 1 != 0 }
    /// A DMA_START / DMA_CONTINUE waits for its input; while true, BUSY reads 1.
    pub fn dma_pending(&self) -> bool { self.engine.dma_pending }
    /// Bytes the pending DMA request hashes: SHA_DMA_BLOCK_NUM (16 bits on the S31) blocks.
    pub fn dma_bytes(&self) -> usize { self.dma_blocks as usize * self.engine.block_bytes() }
    /// Finish the pending DMA request with the gathered input. DMA_START restarts from the initial
    /// state, DMA_CONTINUE from H_MEM. `None`: the OUT chain failed (short, or a descriptor/buffer
    /// fault). The model never reports a fabricated transfer: nothing is hashed, BUSY stays 1 and
    /// the done interrupt stays low, so IDF hangs visibly in sha_hal_wait_idle instead of reading a
    /// stale digest. Only a reset (or a new DMA_START / DMA_CONTINUE) leaves that state.
    pub fn dma_complete(&mut self, input: Option<&[u8]>) {
        self.engine.dma_pending = false;
        let Some(input) = input else { return };
        self.engine.hash_dma_input(input);
        self.engine.busy = false; self.int_raw = true;
    }
    pub fn read(&self, off: u32) -> Result<u32, Fault> {
        if off & 3 != 0 { return Err(Fault::Misaligned); }
        if !self.enabled() { return Err(Fault::Prohibited); }
        match off {
            0 | 0x18 | 0x40..=0xfc => Ok(self.engine.read(off)),
            0x28 => Ok(self.int_ena),
            0xc => Ok(self.dma_blocks), 0x2c => Ok(self.date), 0x38 => Ok(self.free),
            _ => Err(Fault::Unmapped),
        }
    }
    pub fn write(&mut self, off: u32, value: u32) -> Result<(), Fault> {
        if off & 3 != 0 { return Err(Fault::Misaligned); }
        if !self.enabled() { return Err(Fault::Prohibited); }
        match off {
            0 if value & 15 > 4 => return Err(Fault::Prohibited),
            0 | 0x40..=0xfc => self.engine.write(off, value),
            0x10 | 0x14 => { if value & 1 != 0 { self.engine.write(off, 1); } }
            0xc => self.dma_blocks = value & 0xffff,
            0x2c => self.date = value & 0x3fffffff,
            0x38 => self.free = value & 1,
            0x1c | 0x20 => { if value & 1 != 0 { self.engine.write(off, 1); } }
            0x24 => { if value & 1 != 0 { self.int_raw = false; } }
            0x28 => self.int_ena = value & 1,
            0x30 | 0x34 => {}
            0x18 => {} // RO busy. All supported compression completes synchronously.
            _ => return Err(Fault::Unmapped),
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_known_answer_and_parent_reset_gates() {
        let mut s = ShaEngine::default();
        s.write(0x80, 0x80636261).unwrap(); s.write(0xbc, 0x18000000).unwrap();
        s.write(0x10, 0).unwrap(); assert_eq!(s.blocks(), 0);
        s.write(0x10, 1).unwrap(); assert_eq!(s.blocks(), 1); assert_eq!(s.read(0x18), Ok(0));
        let digest: Vec<_> = (0..8).flat_map(|i| s.read(0x40 + 4 * i).unwrap().to_le_bytes()).collect();
        assert_eq!(digest, [0xba,0x78,0x16,0xbf,0x8f,0x01,0xcf,0xea,0x41,0x41,0x40,0xde,0x5d,0xae,0x22,0x23,0xb0,0x03,0x61,0xa3,0x96,0x17,0x7a,0x9c,0xb4,0x10,0xff,0x61,0xf2,0x00,0x15,0xad]);
        for bit in [0, 2, 6] {
            s.clock_write(0x415555 & !(1 << bit)).unwrap(); assert_eq!(s.read(0x40), Err(Fault::Prohibited));
            s.clock_write(0x415555).unwrap(); assert_eq!(s.read(0x40), Ok(0xbf1678ba));
        }
        for bit in [1, 3, 7, 11, 15, 17] {
            s.clock_write(0x415555 | (1 << bit)).unwrap(); assert_eq!(s.write(0x10, 1), Err(Fault::Prohibited));
            s.clock_write(0x415555).unwrap(); assert_eq!(s.read(0x40), Ok(0));
        }
    }
    #[test]
    fn s31_modes_and_dma_count_do_not_alias_the_older_shared_defaults() {
        let mut s = ShaEngine::default();
        s.write(0xc, 0x12345678).unwrap(); assert_eq!(s.read(0xc), Ok(0x5678));
        for mode in 5..16 { assert_eq!(s.write(0, mode), Err(Fault::Prohibited)); }
        assert_eq!(s.read(0), Ok(2));
        assert_eq!(s.blocks(), 0); assert_eq!(s.read(0x100), Err(Fault::Unmapped));
    }
    #[test]
    fn dma_start_sets_busy_and_irq_ena_gates_the_done_interrupt() {
        let mut s = ShaEngine::default();
        s.write(0xc, 1).unwrap(); s.write(0x1c, 1).unwrap();
        assert_eq!(s.read(0x18), Ok(1)); assert!(s.dma_pending()); assert_eq!(s.dma_bytes(), 64);
        s.write(0x18, 0).unwrap(); assert_eq!(s.read(0x18), Ok(1), "BUSY is read-only");
        s.dma_complete(Some(&[0; 64]));
        assert_eq!(s.read(0x18), Ok(0)); assert_eq!(s.blocks(), 1);
        assert!(!s.irq(), "raw done, IRQ_ENA clear");
        s.write(0x28, 0xffff_ffff).unwrap(); assert_eq!(s.read(0x28), Ok(1)); assert!(s.irq());
        s.write(0x24, 0).unwrap(); assert!(s.irq(), "CLEAR_IRQ needs bit 0");
        s.write(0x24, 1).unwrap(); assert!(!s.irq());
        s.write(0x28, 0).unwrap();
        // A failed chain: BUSY stays, no interrupt, nothing hashed.
        s.write(0x28, 1).unwrap(); s.write(0x20, 1).unwrap(); s.dma_complete(None);
        assert_eq!(s.read(0x18), Ok(1)); assert!(!s.dma_pending()); assert!(!s.irq()); assert_eq!(s.blocks(), 1);
    }
}
