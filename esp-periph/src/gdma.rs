use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;

// ------------------------------------------------------------------ GDMA (out/TX channels only for now) + I2S0 TX
pub const GDMA_CHANNELS: usize = 5;
pub const GDMA_CH_STRIDE: u32 = 0xC0;
pub const DMA_ADDR_BASE: u32 = 0x3FC0_0000;

#[derive(Clone, Copy, Default)]
pub struct GdmaOutCh {
    pub conf0: u32, pub conf1: u32, pub int_raw: u32, pub int_ena: u32, pub link: u32, pub peri_sel: u32, pub pri: u32,
    pub desc: u32,            // current descriptor address (0 = none)
    pub buf_pos: u32,         // bytes consumed from the current descriptor
    pub running: bool,
    pub eof_desc: u32,
}
impl GdmaOutCh {
    pub fn irq(&self) -> bool { self.int_raw & self.int_ena != 0 }
}

/// GDMA receive (IN) channel: peripheral -> memory through a descriptor chain.
#[derive(Clone, Copy, Default)]
pub struct GdmaInCh {
    pub conf0: u32, pub conf1: u32, pub int_raw: u32, pub int_ena: u32, pub link: u32, pub peri_sel: u32, pub pri: u32,
    pub desc: u32, pub eof_desc: u32, pub running: bool,
    pub buf_pos: u32,         // bytes filled in the current descriptor
    pub rx_eof_pos: u32,      // peripheral bytes since the last receive EOF
}
impl GdmaInCh { pub fn irq(&self) -> bool { self.int_raw & self.int_ena != 0 } }

pub struct Gdma { pub out: [GdmaOutCh; GDMA_CHANNELS], pub inp: [GdmaInCh; GDMA_CHANNELS], ram: RegRam, pub misc: u32, pub dbg: bool,
                  /// the high bits of a descriptor address: the LINK registers carry 20 (S3 DRAM at 0x3FC0_0000, C6 SRAM at 0x4080_0000)
                  pub addr_base: u32 }
impl Gdma {
    pub fn new() -> Self { Gdma { out: [GdmaOutCh::default(); GDMA_CHANNELS], inp: [GdmaInCh::default(); GDMA_CHANNELS], ram: RegRam::new(), misc: 0, dbg: false, addr_base: DMA_ADDR_BASE } }
    pub fn read(&self, off: u32) -> u32 {
        if off < GDMA_CH_STRIDE * GDMA_CHANNELS as u32 {
            let ch = (off / GDMA_CH_STRIDE) as usize; let o = off % GDMA_CH_STRIDE; let c = &self.out[ch]; let r = &self.inp[ch];
            return match o {
                0x00 => r.conf0, 0x04 => r.conf1, 0x08 => r.int_raw, 0x0c => r.int_raw & r.int_ena, 0x10 => r.int_ena,
                0x18 => 1 << 1 | 0x1f,                       // INFIFO_STATUS: empty
                0x20 => r.link & 0xF_FFFF,
                0x24 => if r.running { (r.desc & 0x3ffff) | (1 << 20) } else { 0 },
                0x28 => r.eof_desc, 0x2c => r.eof_desc, 0x30 => r.desc, 0x44 => r.pri, 0x48 => r.peri_sel,
                0x60 => c.conf0, 0x64 => c.conf1, 0x68 => c.int_raw, 0x6c => c.int_raw & c.int_ena, 0x70 => c.int_ena,
                0x78 => 0x1f | (1 << 1),   // OUTFIFO_STATUS: fifo empty
                0x80 => c.link & 0xF_FFFF,
                0x84 => if c.running { (c.desc & 0x3ffff) | (1 << 20) } else { 0 },   // OUT_STATE: dscr addr + state
                0x88 => c.eof_desc, 0x8c => c.eof_desc, 0x90 => c.desc, 0xa4 => c.pri, 0xa8 => c.peri_sel,
                _ => self.ram.read(off),
            };
        }
        match off { 0x3c8 => self.misc, 0x40c => 0x2008250, _ => self.ram.read(off) }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        if off < GDMA_CH_STRIDE * GDMA_CHANNELS as u32 {
            let ch = (off / GDMA_CH_STRIDE) as usize; let o = off % GDMA_CH_STRIDE;
            if o < 0x60 {
                let r = &mut self.inp[ch];
                match o {
                    0x00 => { r.conf0 = v & !1; if v & 1 != 0 { r.running = false; r.desc = 0; r.buf_pos = 0; r.rx_eof_pos = 0; } }   // IN_RST self-clears
                    0x04 => r.conf1 = v, 0x10 => r.int_ena = v, 0x14 => r.int_raw &= !v,
                    0x20 => {
                        r.link = v & 0xF_FFFF;
                        if v & (1 << 22) != 0 || v & (1 << 23) != 0 { r.desc = self.addr_base | (v & 0xF_FFFF); r.buf_pos = 0; r.rx_eof_pos = 0; r.running = true; }   // START / RESTART
                        if v & (1 << 21) != 0 { r.running = false; }                                                                 // STOP
                    }
                    0x44 => r.pri = v, 0x48 => r.peri_sel = v & 0x3f,
                    _ => self.ram.write(off, v),
                }
                return;
            }
            let c = &mut self.out[ch];
            match o {
                0x60 => { c.conf0 = v & !1; if v & 1 != 0 { c.running = false; c.desc = 0; c.buf_pos = 0; } }   // OUT_RST self-clears
                0x64 => c.conf1 = v, 0x70 => c.int_ena = v, 0x74 => c.int_raw &= !v,
                0x80 => {
                    c.link = v & 0xF_FFFF;
                    if v & (1 << 21) != 0 || v & (1 << 22) != 0 { c.desc = self.addr_base | (v & 0xF_FFFF); c.buf_pos = 0; c.running = true; if c.peri_sel == 5 && self.dbg { eprintln!("[lcd] gdma out link {} at {:#010x}", if v & (1 << 22) != 0 { "RESTART" } else { "START" }, c.desc); } }   // START / RESTART
                    if v & (1 << 20) != 0 { c.running = false; }                                                                                 // STOP
                }
                0xa4 => c.pri = v, 0xa8 => c.peri_sel = v & 0x3f,
                _ => self.ram.write(off, v),
            }
            return;
        }
        match off { 0x3c8 => self.misc = v, _ => self.ram.write(off, v) }
    }
    /// Find the out channel bound to peripheral `peri` (GDMA_TRIG_PERIPH_*).
    pub fn out_channel_for(&self, peri: u32) -> Option<usize> { (0..GDMA_CHANNELS).find(|&i| self.out[i].running && self.out[i].peri_sel == peri) }
    /// An armed receive channel is passive. Each producer added here must also
    /// provide an active-cadence term or a deadline in the owning SoC scheduler.
    pub fn in_channel_for(&self, peri: u32) -> Option<usize> { (0..GDMA_CHANNELS).find(|&i| self.inp[i].running && self.inp[i].peri_sel == peri) }
}

impl Default for Gdma { fn default() -> Self { Self::new() } }

/// One DMA descriptor (dma_descriptor_t): dw0 = size[11:0] length[23:12] suc_eof[30] owner[31]; dw1 = buffer; dw2 = next
pub struct DmaDesc { pub addr: u32, pub size: u32, pub length: u32, pub eof: bool, pub owner_dma: bool, pub buf: u32, pub next: u32 }
pub fn read_desc(mem: &dyn Fn(u32) -> u32, addr: u32) -> DmaDesc {
    let dw0 = mem(addr);
    DmaDesc { addr, size: dw0 & 0xfff, length: (dw0 >> 12) & 0xfff, eof: dw0 & (1 << 30) != 0, owner_dma: dw0 & (1 << 31) != 0, buf: mem(addr + 4), next: mem(addr + 8) }
}
impl Device for Gdma {
    fn read(&mut self, off: u32) -> u32 { Gdma::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Gdma::write(self, off, v); WriteEffect::NONE }
    /// bits 0..5 = out channels, bits 5..10 = in channels
    fn debug(&mut self, on: bool) { self.dbg = on; }
    fn irq_sources(&self) -> u64 { (0..GDMA_CHANNELS).fold(0, |m, i| m | ((self.out[i].irq() as u64) << i) | ((self.inp[i].irq() as u64) << (GDMA_CHANNELS + i))) }
}

/// Bounds zero-progress chains and crypto allocation (at most 4095 bytes per descriptor).
pub const GDMA_DESCRIPTOR_STEP_BUDGET: usize = 4096;

/// Memory accesses used by the shared crypto OUT reader. DMA is not CPU cache traffic.
pub trait DmaMemory {
    fn out_channel(&mut self) -> &mut GdmaOutCh;
    fn descriptor(&mut self, address: u32) -> Option<(u32, DmaDesc)>;
    fn append(&mut self, address: u32, count: usize, out: &mut Vec<u8>) -> Option<()>;
    fn writeback(&mut self, address: u32, value: u32) -> Option<()>;
}

/// C3/C6 crypto DMA can access data SRAM only, never MMIO.
pub struct DmaRam<'a> { pub base: u32, pub bytes: &'a mut [u8], pub channel: &'a mut GdmaOutCh }
impl DmaRam<'_> {
    fn range(&self, address: u32, count: usize) -> Option<std::ops::Range<usize>> {
        let start = address.checked_sub(self.base)? as usize;
        let end = start.checked_add(count).filter(|&n| n <= self.bytes.len())?;
        Some(start..end)
    }
}
impl DmaMemory for DmaRam<'_> {
    fn out_channel(&mut self) -> &mut GdmaOutCh { self.channel }
    fn descriptor(&mut self, address: u32) -> Option<(u32, DmaDesc)> {
        self.range(address, 12)?;
        let word = |a| {
            let start = (a - self.base) as usize;
            u32::from_le_bytes(self.bytes[start..start + 4].try_into().unwrap())
        };
        Some((word(address), read_desc(&word, address)))
    }

    fn append(&mut self, address: u32, count: usize, out: &mut Vec<u8>) -> Option<()> {
        let range = self.range(address, count)?;
        out.extend_from_slice(&self.bytes[range]);
        Some(())
    }
    fn writeback(&mut self, address: u32, value: u32) -> Option<()> {
        let range = self.range(address, 4)?;
        self.bytes[range].copy_from_slice(&value.to_le_bytes());
        Some(())
    }
}

/// Bounded OUT reader shared by S3 AES/SHA and C3/C6 SHA. Preserve S3 completion semantics.
/// IDF v5.5.5 components/soc/{esp32s3,esp32c3,esp32c6}/register/soc/gdma_reg.h:
/// OUT_CONF1 CHECK_OWNER bit 12; OUT_DONE/OUT_EOF/OUT_DSCR_ERR are shared bits 0/1/2.
/// C3's register adapter translates those interrupt bits to its combined IN/OUT layout.
pub fn gather_dma_out(memory: &mut impl DmaMemory, limit: usize) -> Option<Vec<u8>> {
    let mut input = Vec::new();
    let mut desc = memory.out_channel().desc;
    let mut visited = std::collections::HashSet::new();
    while desc != 0 && input.len() < limit {
        if !visited.insert(desc) { return None; }
        if visited.len() > GDMA_DESCRIPTOR_STEP_BUDGET { return None; }
        let (control, d) = memory.descriptor(desc)?;
        if memory.out_channel().conf1 & (1 << 12) != 0 && !d.owner_dma { return None; }
        let take = (d.length as usize).min(limit - input.len());
        memory.append(d.buf, take, &mut input)?;
        memory.writeback(desc, control & !(1 << 31))?;
        memory.out_channel().int_raw |= 1;
        if d.eof {
            memory.out_channel().int_raw |= 1 << 1;
            memory.out_channel().eof_desc = desc;
            break;
        }
        desc = d.next;
    }
    Some(input)
}

#[cfg(test)]
mod crypto_dma_tests {
    use super::*;

    #[test]
    fn zero_progress_chain_stops_at_descriptor_budget() {
        let mut bytes = vec![0; 4097 * 12];
        for i in 0..4097 {
            let offset = i * 12;
            bytes[offset..offset + 4].copy_from_slice(&((1u32 << 31) | if i == 4096 { 1 << 30 } else { 0 }).to_le_bytes());
            bytes[offset + 4..offset + 8].copy_from_slice(&0x1000u32.to_le_bytes());
            bytes[offset + 8..offset + 12].copy_from_slice(&(0x1000u32 + offset as u32 + 12).to_le_bytes());
        }
        let mut channel = GdmaOutCh { desc: 0x1000, ..Default::default() };
        let mut memory = DmaRam { base: 0x1000, bytes: &mut bytes, channel: &mut channel };
        assert!(gather_dma_out(&mut memory, 64).is_none());
    }
}
