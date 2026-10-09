use super::{DmaDesc, GdmaInCh};
use emu_core::{Bus, Fault};
pub const GDMA_DESCRIPTOR_STEP_BUDGET: usize = 4096;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmaDescriptorWord {
    Control,
    Buffer,
    Next,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmaDescriptorFault {
    Read { descriptor: u32, word: DmaDescriptorWord, fault: Fault },
    BufferRead { descriptor: u32, address: u32, fault: Fault },
    Writeback { descriptor: u32, fault: Fault },
    NotOwned { descriptor: u32 },
    Cycle { descriptor: u32 },
    StepBudgetExceeded { budget: usize },
    PayloadTooShort { expected: usize, actual: usize },
}

/// Bound work even when guest descriptors make no progress. Streaming rings are valid: the
/// bound applies to one pump call, not the lifetime of an I2S or LCD channel.
pub struct DescriptorWalk { pub remaining: usize, budget: usize }
impl DescriptorWalk {
    pub fn new(budget: usize) -> Self { Self { remaining: budget, budget } }
    pub fn read(&mut self, bus: &mut impl Bus, addr: u32) -> Result<(u32, DmaDesc), DmaDescriptorFault> {
        if self.remaining == 0 { return Err(DmaDescriptorFault::StepBudgetExceeded { budget: self.budget }); }
        self.remaining -= 1;
        try_dma_desc(bus, addr)
    }
}

/// Read a descriptor through the same decoder as the infallible host-memory API.
fn try_dma_desc(bus: &mut impl Bus, addr: u32) -> Result<(u32, DmaDesc), DmaDescriptorFault> {
    let mut word = |offset, word| bus.read32_unpriced(addr.wrapping_add(offset))
        .map_err(|fault| DmaDescriptorFault::Read { descriptor: addr, word, fault });
    let dw0 = word(0, DmaDescriptorWord::Control)?;
    let (buf, next) = (word(4, DmaDescriptorWord::Buffer)?, word(8, DmaDescriptorWord::Next)?);
    Ok((dw0, DmaDesc::decode(addr, dw0, buf, next)))
}

impl GdmaInCh {
    pub fn fail_receive(&mut self, irq_changed: &mut bool) {
        let before = self.int_raw;
        self.running = false;
        self.int_raw |= 1 << 3; // IN_DSCR_ERR
        *irq_changed |= self.int_raw != before;
    }

    /// Scatter input and publish both completion and descriptor-error interrupt changes.
    pub fn receive(&mut self, bus: &mut impl Bus, data: &[u8], eof_bytes: Option<u32>, finish: bool,
        is_periph: impl Fn(u32) -> bool, irq_changed: &mut bool) -> bool {
        let before = self.int_raw;
        let result = self.scatter(bus, data, eof_bytes, finish, is_periph);
        if !result { self.fail_receive(irq_changed); }
        *irq_changed |= self.int_raw != before;
        result
    }

    /// Hand a filled or EOF-ended IN descriptor back to the CPU (its length, owner, SUC_EOF) and
    /// move the channel to the next one. False when the write-back faults.
    pub fn close(&mut self, bus: &mut impl Bus, dw0: u32, next: u32, eof: bool) -> bool {
        let v = (dw0 & !(0xfff << 12) & !(3 << 30)) | (self.buf_pos << 12) | if eof { 1 << 30 } else { 0 };
        if bus.write32_unpriced(self.desc, v).is_err() { return false; }
        self.int_raw |= 1 << 0;                                                  // IN_DONE
        if eof { self.int_raw |= 1 << 1; self.eof_desc = self.desc; self.rx_eof_pos = 0; }                  // IN_SUC_EOF
        self.desc = next; self.buf_pos = 0;
        true
    }

    /// Shared camera/crypto receive path. A malformed destination never reports successful EOF.
    pub fn scatter(&mut self, bus: &mut impl Bus, data: &[u8], eof_bytes: Option<u32>, finish: bool, is_periph: impl Fn(u32) -> bool) -> bool {
        let mut pos = 0usize;
        let mut walk = DescriptorWalk::new(GDMA_DESCRIPTOR_STEP_BUDGET);
        while pos < data.len() || (finish && self.rx_eof_pos != 0) {
            let mut r = *self;
            // IDF v5.5.4 soc/gdma_reg.h: IN_DSCR_EMPTY bit 4 means data remains
            // but there is no more inlink; CHECK_OWNER is bit 12.
            if r.desc == 0 {
                // Belt-and-braces: a live receive with no descriptor must still have data left.
                if pos < data.len() { self.int_raw |= 1 << 4; }
                return false;
            }
            let Ok((control, d)) = walk.read(bus, r.desc) else { return false };
            if (r.conf1 & (1 << 12) != 0 && !d.owner_dma) || d.size == 0 || r.buf_pos > d.size { return false; }
            let Some(until_eof) = eof_bytes.unwrap_or(u32::MAX).checked_sub(r.rx_eof_pos).filter(|n| *n != 0) else { return false };
            let n = (d.size - r.buf_pos).min(until_eof) as usize;
            let n = n.min(data.len() - pos);
            let Some(dest) = d.buf.checked_add(r.buf_pos) else { return false };
            let Some(end) = dest.checked_add(n as u32) else { return false };
            // DMA buffers are memory, never MMIO.
            if n != 0 && (is_periph(dest) || is_periph(end - 1)) { return false; }
            let mut i = 0;
            while i < n && (dest + i as u32) & 3 != 0 {
                if bus.write8_unpriced(dest + i as u32, data[pos + i]).is_err() { return false; }
                i += 1;
            }
            while i + 4 <= n {
                let word = u32::from_le_bytes(data[pos + i..pos + i + 4].try_into().unwrap());
                if bus.write32_unpriced(dest + i as u32, word).is_err() { return false; }
                i += 4;
            }
            while i < n {
                if bus.write8_unpriced(dest + i as u32, data[pos + i]).is_err() { return false; }
                i += 1;
            }
            pos += n;
            r.buf_pos += n as u32;
            r.rx_eof_pos += n as u32;
            let eof = eof_bytes == Some(r.rx_eof_pos) || finish;
            // Keep a final full descriptor pending in VS_EOF mode until VSYNC marks it.
            if (eof || (r.buf_pos == d.size && (eof_bytes.is_some() || pos < data.len())))
                && !r.close(bus, control, d.next, eof) { return false; }
            if r.desc == 0 { r.running = false; }
            *self = r;
        }
        true
    }

}
