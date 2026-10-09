use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;

// ------------------------------------------------------------------ RSA/MPI accelerator (0x6003C000)
/// The big-number accelerator mbedTLS uses for every RSA and ECC operation (`CONFIG_MBEDTLS_HARDWARE_MPI`).
/// Four 4096-bit memory blocks — M at 0x000, Z at 0x200, Y at 0x400, X at 0x600 — plus three start
/// registers. The silicon works in the Montgomery domain, which is why firmware also loads M' and
/// R^-1; here the operations are computed exactly, so those two are accepted and ignored.
///   Z = X * Y          (0x814, LENGTH = 2*words-1, Y read from the Z block's upper half)
///   Z = X * Y mod M    (0x810, LENGTH = words-1)
///   Z = X ^ Y mod M    (0x80c, LENGTH = words-1)
pub struct Rsa { pub mem: Vec<u32>, pub m_prime: u32, pub length: u32, pub int_ena: u32, pub int_raw: u32,
                 pub constant_time: u32, pub search_open: u32, pub search_pos: u32, pub ops: u64, ram: RegRam, pub dbg: bool }
impl Rsa {
    pub fn new() -> Self { Rsa { mem: vec![0; 512], m_prime: 0, length: 0, int_ena: 0, int_raw: 0,
                                 constant_time: 0, search_open: 0, search_pos: 0, ops: 0, ram: RegRam::new(), dbg: false } }
    pub fn irq(&self) -> bool { self.int_raw != 0 && self.int_ena != 0 }
    fn block(&self, base: usize, words: usize) -> Vec<u32> { self.mem[base..base + words.min(128)].to_vec() }
    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            0x000..=0x7fc => self.mem[(off / 4) as usize],
            0x800 => self.m_prime,
            0x804 => self.length,
            0x808 => 1,                                   // memory initialised: firmware spins on this
            0x818 => 1,                                   // idle: operations here complete before the next read
            0x820 => self.constant_time, 0x824 => self.search_open, 0x828 => self.search_pos,
            0x82c => self.int_ena,
            _ => self.ram.read(off),
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x000..=0x7fc => self.mem[(off / 4) as usize] = v,
            0x800 => self.m_prime = v,
            0x804 => self.length = v & 0x7f, // RSA_LENGTH is seven bits; larger operands do not fit the RAM.
            0x80c => { let n = self.length as usize + 1; let z = crate::crypto::bn_modexp(&self.block(384, n), &self.block(256, n), &self.block(0, n)); self.finish(z, n); }
            0x810 => { let n = self.length as usize + 1; let z = crate::crypto::bn_mod(&crate::crypto::bn_mul(&self.block(384, n), &self.block(256, n)), &self.block(0, n)); self.finish(z, n); }
            0x814 => { let n = (self.length as usize).div_ceil(2); let z = crate::crypto::bn_mul(&self.block(384, n), &self.block(128 + n, n)); self.finish(z, 2 * n); }
            0x81c => self.int_raw = 0,                    // clears the interrupt signal, not the idle status
            0x820 => self.constant_time = v, 0x824 => self.search_open = v, 0x828 => self.search_pos = v,
            0x82c => self.int_ena = v,
            _ => self.ram.write(off, v),
        }
    }
    /// Publish a result in the Z block, zero-padded to `words`, and raise the completion flag.
    pub fn finish(&mut self, z: Vec<u32>, words: usize) {
        if self.dbg {
            eprintln!("[rsa] op #{} len={} -> {} words, z[0]={:08x} int_ena={}", self.ops, self.length, words, z.first().copied().unwrap_or(0), self.int_ena);
        }
        let words = words.min(128);
        for i in 0..words { self.mem[128 + i] = *z.get(i).unwrap_or(&0); }
        self.int_raw = 1;
        self.ops += 1;
    }
}

impl Default for Rsa { fn default() -> Self { Self::new() } }

impl Device for Rsa {
    fn read(&mut self, off: u32) -> u32 { Rsa::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Rsa::write(self, off, v); WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { self.irq() as u64 }
    fn debug(&mut self, on: bool) { self.dbg = on; }
}

#[cfg(test)]
mod rsa_tests {
    use super::*;

    #[test]
    fn multiplication_length_ignores_reserved_bits() {
        // ESP32-S3 TRM, register 20.2: LENGTH[6:0]. Maximum multiply uses two 64-word operands.
        let mut rsa = Rsa::new();
        rsa.write(0x804, u32::MAX);
        assert_eq!(rsa.read(0x804), 127);
        rsa.write(0x600, 3);
        rsa.write(0x300, 7);
        rsa.write(0x814, 1);
        assert_eq!(rsa.read(0x200), 21);
        for off in (0x204..0x400).step_by(4) { assert_eq!(rsa.read(off), 0); }
        assert_eq!(rsa.int_raw, 1);
    }

    /// Program the block the way `bignum_alt.c` does and check the three operations.
    #[test]
    fn hardware_ops_match_arithmetic() {
        let mut r = Rsa::new();
        let words = 4usize;
        let x: Vec<u32> = vec![0x0123_4567, 0x89ab_cdef, 0xfedc_ba98, 0x7654_3210];
        let y: Vec<u32> = vec![0x1111_2222, 0x3333_4444, 0x5555_6666, 0x0000_0007];
        let m: Vec<u32> = vec![0xffff_fff1, 0x1234_5678, 0x9abc_def0, 0x8000_0001];  // odd modulus
        let load = |r: &mut Rsa, base: u32, v: &[u32]| { for (i, w) in v.iter().enumerate() { r.write(base + 4 * i as u32, *w); } };

        // Z = X * Y: X in the X block, Y left-extended into the Z block, LENGTH = 2*words-1
        load(&mut r, 0x600, &x);
        load(&mut r, 0x200 + 4 * words as u32, &y);
        r.write(0x804, words as u32 * 2 - 1);
        r.write(0x814, 1);
        assert_eq!(r.read(0x818), 1, "the block reports idle once the operation is done");
        assert!(r.int_raw != 0, "completion raises the interrupt latch");
        let z: Vec<u32> = (0..2 * words).map(|i| r.read(0x200 + 4 * i as u32)).collect();
        assert_eq!(z, crate::crypto::bn_mul(&x, &y));
        r.write(0x81c, 1);
        assert_eq!(r.int_raw, 0, "interrupt clear drops the latch");
        assert_eq!(r.read(0x818), 1, "but the block still reads idle");

        // Z = X * Y mod M, LENGTH = words-1
        load(&mut r, 0x600, &x); load(&mut r, 0x400, &y); load(&mut r, 0x000, &m);
        r.write(0x804, words as u32 - 1);
        r.write(0x810, 1);
        let z: Vec<u32> = (0..words).map(|i| r.read(0x200 + 4 * i as u32)).collect();
        let expect = crate::crypto::bn_mod(&crate::crypto::bn_mul(&x, &y), &m);
        assert_eq!(z[..expect.len()], expect[..]);

        // Z = X ^ Y mod M
        r.write(0x81c, 1);
        r.write(0x80c, 1);
        let z: Vec<u32> = (0..words).map(|i| r.read(0x200 + 4 * i as u32)).collect();
        let expect = crate::crypto::bn_modexp(&x, &y, &m);
        assert_eq!(z[..expect.len()], expect[..]);
        assert!(r.read(0x808) == 1, "memory-init query must read back ready");
    }
}
