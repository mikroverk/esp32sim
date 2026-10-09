use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;

/// Which array a SPI command wrote (the SoC maps this to its own buffer ids).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DirtyMem { Flash, Psram }

// ------------------------------------------------------------------ SPI flash controller (SPI_MEM: SPI1 = command engine, SPI0 = cache path)
pub struct SpiMem {
    /// Whether a PSRAM device sits on CS1. True for the S3; the C3 has no PSRAM, and without
    /// this a flash command issued with CS0 disabled is misrouted to the PSRAM branch and
    /// answered with zeros (found on real C3 silicon: `E memspi: no response`).
    pub has_psram: bool,
    pub regs: RegRam,
    /// (memory, offset, len) ranges the last command wrote, for the bus's page versions
    pub dirty: Vec<(DirtyMem, usize, usize)>,
    pub w: [u32; 16],
    pub pending_cmd: u32,
    pub status: u32,
    pub jedec: [u8; 3],
    pub is_spi1: bool,
    pub log: bool,
    /// octal PSRAM (APS6408-like) mode registers MR0..MR8, device on CS1 (SPI1 only)
    pub psram_mr: [u8; 9],
}
impl SpiMem {
    pub fn new(is_spi1: bool) -> Self { SpiMem { has_psram: true, regs: RegRam::new(), dirty: Vec::new(), w: [0; 16], pending_cmd: 0, status: 0x200, jedec: [0x20, 0x40, 0x17], is_spi1, log: false , psram_mr: [0x09, 0x0d, 0x8b, 0x00, 0x20, 0, 0, 0, 0x03] } }
    pub fn read(&self, off: u32) -> u32 {
        match off {
            0x0 => 0,                                   // CMD: always idle after execution
            0x2c => self.status,                        // RD_STATUS
            0x54 => 0,                                  // FSM idle
            0x58..=0x94 => self.w[((off - 0x58) / 4) as usize],
            0xa4 => 0,                                  // SUS_STATUS
            0x3fc => 0x2101040,
            _ => self.regs.read(off),
        }
    }
    /// Returns true if a command must be executed against the flash array.
    pub fn write(&mut self, off: u32, v: u32) -> bool {
        match off {
            0x0 => { self.pending_cmd = v; return v & 0xffff_0003 != 0; }
            0x2c => {}   // RD_STATUS is written by hardware only
            0x58..=0x94 => self.w[((off - 0x58) / 4) as usize] = v,
            _ => self.regs.write(off, v),
        }
        false
    }
    fn w_bytes(&self, n: usize) -> Vec<u8> { let mut o = Vec::with_capacity(n); for i in 0..n { o.push((self.w[(i / 4) & 15] >> ((i % 4) * 8)) as u8); } o }
    fn set_w_bytes(&mut self, data: &[u8]) { for w in self.w.iter_mut() { *w = 0; } for (i, b) in data.iter().enumerate().take(64) { self.w[i / 4] |= (*b as u32) << ((i % 4) * 8); } }

    pub fn execute(&mut self, flash: &mut [u8], psram: &mut [u8]) {
        let cmd = self.pending_cmd; self.pending_cmd = 0;
        let user = self.regs.read(0x18); let user1 = self.regs.read(0x1c); let user2 = self.regs.read(0x20);
        let addr_reg = self.regs.read(0x4);
        let miso_bytes = ((self.regs.read(0x28) & 0x3ff) as usize + 8) / 8;
        let mosi_bytes = ((self.regs.read(0x24) & 0x3ff) as usize + 8) / 8;
        let addr_bits = ((user1 >> 26) & 0x3f) + 1;
        let addr = if addr_bits > 24 { addr_reg } else { addr_reg & 0xff_ffff };
        let fsize = flash.len();
        let rd = |a: u32, n: usize| -> Vec<u8> { (0..n).map(|i| { let x = a as usize + i; if x < fsize { flash[x] } else { 0xff } }).collect() };
        // IDF v5.5.5 components/soc/esp32s3/register/soc/spi_mem_reg.h:
        // USR bit 18 (110-113), MISO/MOSI bits 28/27 (383-391), CS disable (560-570).
        let misc = self.regs.read(0x34);
        if self.has_psram && cmd & (1 << 18) != 0 && misc & 1 != 0 && misc & 2 == 0 {   // USR command with CS0 disabled, CS1 enabled: PSRAM
            let c16 = user2 & 0xffff;
            let has_miso = user & (1 << 28) != 0; let has_mosi = user & (1 << 27) != 0;
            if self.log { eprintln!("[spi1] psram cmd {:#06x} addr {:#x} miso {} mosi {}", c16, addr, if has_miso { miso_bytes } else { 0 }, if has_mosi { mosi_bytes } else { 0 }); }
            let psize = psram.len();
            if psize == 0 {
                if has_miso { self.set_w_bytes(&vec![0xff; miso_bytes]); }
                return;
            }
            // IDF v5.5.5 components/esp_psram/device/esp_quad_psram_defs_ap.h:21-60;
            // v4.4.8 components/esp_hw_support/port/esp32s3/spiram_psram.c:46-83
            // use the same commands, KGD and 2/4/8 MiB density encoding.
            // Revision bits and absent-device 0xff are inferred model choices.
            match c16 {
                0x9f => {
                    let density = match psize {
                        0x200000 => 0x02, 0x400000 => 0x22, 0x800000 => 0x42,
                        _ => { self.set_w_bytes(&vec![0xff; miso_bytes]); return; }
                    };
                    self.set_w_bytes(&[0x0d, 0x5d, density][..miso_bytes.min(3)]);
                }
                0x4040 => { let i = (addr & 0xf) as usize; let d: Vec<u8> = (0..miso_bytes).map(|k| *self.psram_mr.get(i + k).unwrap_or(&0)).collect(); self.set_w_bytes(&d); }   // mode register read
                0xC0C0 => { let d = self.w_bytes(mosi_bytes); let i = (addr & 0xf) as usize; for (k, b) in d.iter().enumerate() { if i + k == 0 || i + k == 8 { self.psram_mr[i + k] = *b; } } }   // mode register write (MR0/MR8 writable)
                0x02 | 0x38 | 0x8080 => { let d = self.w_bytes(mosi_bytes); self.dirty.push((DirtyMem::Psram, addr as usize, d.len())); for (k, b) in d.iter().enumerate() { let x = addr as usize + k; if x < psize { psram[x] = *b; } } }   // sync write
                0x03 | 0x0b | 0xeb | 0x0000 => { let d: Vec<u8> = (0..miso_bytes).map(|k| { let x = addr as usize + k; if x < psize { psram[x] } else { 0 } }).collect(); self.set_w_bytes(&d); }   // sync read
                _ => { if has_miso { self.set_w_bytes(&vec![0u8; miso_bytes]); } }
            }
            return;
        }
        if cmd & (1 << 18) != 0 {   // USR: command from USER2
            let c = if user & (1 << 31) != 0 { (user2 & 0xff) as u8 } else { 0 };
            let has_addr = user & (1 << 30) != 0;
            let has_miso = user & (1 << 28) != 0;
            let has_mosi = user & (1 << 27) != 0;
            if self.log { eprintln!("[spi1] usr cmd {:#04x} addr {:#x}{} miso {} mosi {}", c, addr, if has_addr { "" } else { " (no addr)" }, if has_miso { miso_bytes } else { 0 }, if has_mosi { mosi_bytes } else { 0 }); }
            match c {
                0x03 | 0x0b | 0x3b | 0x6b | 0xbb | 0xeb => { let d = rd(addr, miso_bytes); self.set_w_bytes(&d); }
                0x9f => { let j = self.jedec; self.set_w_bytes(&j); }
                0x05 => { let s = self.status; self.set_w_bytes(&[s as u8]); }
                0x35 => { let s = self.status; self.set_w_bytes(&[(s >> 8) as u8]); }
                0x06 => self.status |= 0x02,                       // WREN: set WEL
                0x04 => self.status &= !0x02,                      // WRDI
                0x01 | 0x31 | 0x11 => self.status &= !0x02,        // WRSR*: latch consumed (keep QE set)
                0x15 => self.set_w_bytes(&[0x00]),
                0x02 | 0x32 | 0x38 => { let d = self.w_bytes(mosi_bytes); for (i, b) in d.iter().enumerate() { let x = addr as usize + i; if x < fsize { flash[x] &= *b; } } self.dirty.push((DirtyMem::Flash, addr as usize, d.len())); self.status &= !0x02; }
                0x20 => { let a = (addr as usize) & !0xfff; for b in flash.iter_mut().take((a + 0x1000).min(fsize)).skip(a) { *b = 0xff; } self.dirty.push((DirtyMem::Flash, a, 0x1000)); self.status &= !0x02; }
                0x52 => { let a = (addr as usize) & !0x7fff; for b in flash.iter_mut().take((a + 0x8000).min(fsize)).skip(a) { *b = 0xff; } self.dirty.push((DirtyMem::Flash, a, 0x8000)); self.status &= !0x02; }
                0xd8 => { let a = (addr as usize) & !0xffff; for b in flash.iter_mut().take((a + 0x10000).min(fsize)).skip(a) { *b = 0xff; } self.dirty.push((DirtyMem::Flash, a, 0x10000)); self.status &= !0x02; }
                0xc7 | 0x60 => { for b in flash.iter_mut() { *b = 0xff; } self.dirty.push((DirtyMem::Flash, 0, fsize)); self.status &= !0x02; }
                _ => { if has_miso { self.set_w_bytes(&vec![0u8; miso_bytes]); } }
            }
            return;
        }
        if cmd & (1 << 31) != 0 { let d = rd(addr, miso_bytes); self.set_w_bytes(&d); }                       // FLASH_READ
        if cmd & (1 << 28) != 0 { let j = self.jedec; self.set_w_bytes(&j); }                                   // RDID
        if cmd & (1 << 30) != 0 { self.status |= 0x02; }                                                         // WREN
        if cmd & (1 << 29) != 0 { self.status &= !0x02; }                                                        // WRDI
        if cmd & (1 << 26) != 0 { self.status &= !0x02; }                                                        // WRSR
        // RDSR (bit 27): RD_STATUS already reflects the live status register
        if cmd & (1 << 25) != 0 {                                                                                // PP
            let n = if addr_reg >> 24 != 0 { (addr_reg >> 24) as usize } else { mosi_bytes };
            let d = self.w_bytes(n);
            for (i, b) in d.iter().enumerate() { let x = (addr & 0xff_ffff) as usize + i; if x < fsize { flash[x] &= *b; } }
            self.dirty.push((DirtyMem::Flash, (addr & 0xff_ffff) as usize, d.len()));
            self.status &= !0x02;
        }
        if cmd & (1 << 24) != 0 { let a = (addr as usize) & !0xfff; for b in flash.iter_mut().take((a + 0x1000).min(fsize)).skip(a) { *b = 0xff; } self.dirty.push((DirtyMem::Flash, a, 0x1000)); self.status &= !0x02; }      // SE
        if cmd & (1 << 23) != 0 { let a = (addr as usize) & !0xffff; for b in flash.iter_mut().take((a + 0x10000).min(fsize)).skip(a) { *b = 0xff; } self.dirty.push((DirtyMem::Flash, a, 0x10000)); }    // BE
        if cmd & (1 << 22) != 0 { for b in flash.iter_mut() { *b = 0xff; } self.dirty.push((DirtyMem::Flash, 0, fsize)); }                          // CE
    }
}

impl Device for SpiMem {
    fn read(&mut self, off: u32) -> u32 { SpiMem::read(self, off) }
    /// Only SPI1 is the command engine; SPI0 is the cache path and its command bit is ignored.
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { if SpiMem::write(self, off, v) && self.is_spi1 { WriteEffect::SPI_EXEC } else { WriteEffect::NONE } }
    fn debug(&mut self, on: bool) { self.log = on; }
}

#[cfg(test)]
mod tests {
    use super::SpiMem;

    #[test]
    fn cs1_quad_id_tracks_capacity_and_keeps_octal_and_flash_ids() {
        for (size, id) in [(0, 0xffffff), (0x100000, 0xffffff), (0x200000, 0x025d0d), (0x400000, 0x225d0d), (0x800000, 0x425d0d)] {
            let mut spi = SpiMem::new(true);
            let mut psram = vec![0; size];
            spi.write(0x18, (1 << 31) | (1 << 28));
            spi.write(0x20, 0x9f);
            spi.write(0x28, 47);
            spi.write(0x34, 1);
            spi.write(0, 1 << 18);
            spi.execute(&mut [], &mut psram);
            assert_eq!(spi.w[0] & 0xffffff, id);
            if size == 0 {
                spi.write(0x20, 0x4040);
                spi.write(0, 1 << 18);
                spi.execute(&mut [], &mut psram);
                assert_eq!(spi.w[0], u32::MAX);
            } else {
                spi.write(0x20, 0x4040);
                spi.write(0, 1 << 18);
                spi.execute(&mut [], &mut psram);
                assert_eq!(spi.w[0] & 0xffffff, 0x8b0d09);
                for write in [0x02, 0x38, 0x8080] {
                    spi.write(0x24, 31);
                    let value = 0x12340000 | write;
                    spi.write(0x58, value);
                    spi.write(0x20, write);
                    spi.write(0, 1 << 18);
                    spi.execute(&mut [], &mut psram);
                    for read in [0x03, 0x0b, 0xeb, 0x0000] {
                        spi.write(0x58, 0);
                        spi.write(0x20, read);
                        spi.write(0, 1 << 18);
                        spi.execute(&mut [], &mut psram);
                        assert_eq!(spi.w[0], value);
                    }
                }
            }
            spi.write(0x34, 2);
            spi.write(0x20, 0x9f);
            spi.write(0, 1 << 18);
            spi.execute(&mut [], &mut psram);
            assert_eq!(spi.w[0] & 0xffffff, 0x174020);
        }
    }
}
