use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;

// ------------------------------------------------------------------ SHA accelerator (register/block mode; SHA-1/224/256)
pub struct Sha { pub mode: u32, pub h: [u32; 16], pub m: [u32; 32], pub busy: bool,
                 pub block_num: u32, pub dma_pending: bool, pub dma_first: bool, pub blocks: u64, ram: RegRam, pub dbg: bool }
impl Sha {
    pub fn new() -> Self { Sha { mode: 2, h: [0; 16], m: [0; 32], busy: false, block_num: 0,
                                 dma_pending: false, dma_first: false, blocks: 0, ram: RegRam::new(), dbg: false } }
    /// 64 bytes for the 32-bit family, 128 for SHA-384/512.
    pub fn block_bytes(&self) -> usize { if self.mode >= 3 { 128 } else { 64 } }
    /// Feed one message block, as the DMA engine does; `first` restarts from the initial state.
    pub fn hash_block(&mut self, bytes: &[u8], first: bool) {
        for (i, c) in bytes.chunks(4).enumerate().take(32) {
            let mut w = [0u8; 4];
            w[..c.len()].copy_from_slice(c);
            self.m[i] = u32::from_le_bytes(w);
        }
        if first { self.init(); }
        self.compress();
    }
    /// Synchronous SHA DMA request, using the S3 crypto descriptor reader.
    /// IDF v5.5.5 components/soc/esp32c6/register/soc/sha_reg.h:50-120:
    /// six-bit block count, BUSY, DMA_START and DMA_CONTINUE. C3 offsets match
    /// components/soc/esp32c3/include/soc/hwcrypto_reg.h:46-53.
    pub fn dma_step(&mut self, memory: &mut impl crate::gdma::DmaMemory) {
        self.dma_pending = false;
        self.busy = false;
        let want = (self.block_num as usize).saturating_mul(self.block_bytes());
        let input = crate::gdma::gather_dma_out(memory, want);
        let Some(input) = input.filter(|bytes| bytes.len() == want) else {
            memory.out_channel().int_raw |= 1 << 2;
            memory.out_channel().running = false;
            return;
        };
        let mut first = self.dma_first;
        for block in input.chunks(self.block_bytes()) {
            self.hash_block(block, first);
            first = false;
        }
    }

    fn init(&mut self) {
        self.h = [0; 16];
        match self.mode {
            0 => self.h[..5].copy_from_slice(&[0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0]),
            1 => self.h[..8].copy_from_slice(&[0xc1059ed8, 0x367cd507, 0x3070dd17, 0xf70e5939, 0xffc00b31, 0x68581511, 0x64f98fa7, 0xbefa4fa4]),
            2 => self.h[..8].copy_from_slice(&[0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]),
            // SHA-384/512: eight 64-bit words, held high half first so H_MEM reads back digest order
            3 => self.set_h64(&[0xcbbb9d5dc1059ed8, 0x629a292a367cd507, 0x9159015a3070dd17, 0x152fecd8f70e5939,
                                0x67332667ffc00b31, 0x8eb44a8768581511, 0xdb0c2e0d64f98fa7, 0x47b5481dbefa4fa4]),
            _ => self.set_h64(&[0x6a09e667f3bcc908, 0xbb67ae8584caa73b, 0x3c6ef372fe94f82b, 0xa54ff53a5f1d36f1,
                                0x510e527fade682d1, 0x9b05688c2b3e6c1f, 0x1f83d9abfb41bd6b, 0x5be0cd19137e2179]),
        }
    }
    fn set_h64(&mut self, v: &[u64; 8]) {
        for (i, x) in v.iter().enumerate() { self.h[2 * i] = (x >> 32) as u32; self.h[2 * i + 1] = *x as u32; }
    }
    fn h64(&self) -> [u64; 8] {
        let mut v = [0u64; 8];
        for (i, vi) in v.iter_mut().enumerate() { *vi = ((self.h[2 * i] as u64) << 32) | self.h[2 * i + 1] as u64; }
        v
    }
    fn compress(&mut self) {
        // message words are written by software as the bytes of the block; interpret big-endian per SHA
        if self.mode >= 3 {
            let mut w = [0u64; 16];
            for (j, wj) in w.iter_mut().enumerate() { *wj = ((self.m[2 * j].swap_bytes() as u64) << 32) | self.m[2 * j + 1].swap_bytes() as u64; }
            let mut h = self.h64();
            sha512_block(&mut h, &w);
            self.set_h64(&h);
        } else {
            let w0: [u32; 16] = std::array::from_fn(|i| self.m[i].swap_bytes());
            match self.mode {
                0 => crate::crypto::sha1_block(&mut self.h, &w0),
                _ => sha256_block(&mut self.h, &w0),
            }
        }
        self.blocks += 1;
    }
    pub fn read(&self, off: u32) -> u32 {
        match off {
            0x0 => self.mode,
            0x0c => self.block_num,
            0x18 => self.busy as u32,
            0x2c => 0x20190402,
            0x40..=0x7c => { let i = ((off - 0x40) / 4) as usize; self.h[i].swap_bytes() }   // H regs read back as big-endian bytes in memory order
            0x80..=0xfc => self.m[((off - 0x80) / 4) as usize],
            _ => self.ram.read(off),
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x0 => self.mode = v & 7,
            0x0c => self.block_num = v & 0x3f, // SHA_DMA_BLOCK_NUM is a six-bit register.
            0x10 => { self.init(); self.compress(); }
            0x14 => self.compress(),
            0x1c => { self.busy = true; self.dma_pending = true; self.dma_first = true; }    // DMA_START
            0x20 => { self.busy = true; self.dma_pending = true; self.dma_first = false; }   // DMA_CONTINUE
            0x40..=0x7c => { let i = ((off - 0x40) / 4) as usize; self.h[i] = v.swap_bytes(); }
            0x80..=0xfc => self.m[((off - 0x80) / 4) as usize] = v,
            _ => { if self.dbg { eprintln!("[sha] write +0x{:02x} = {} (mode {})", off, v, self.mode); } self.ram.write(off, v) }
        }
    }
}


/// SHA-512 compression (also SHA-384; they differ only in the initial state and truncation).
fn sha512_block(h: &mut [u64; 8], w0: &[u64; 16]) {
    const K: [u64; 80] = [
        0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc, 0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
        0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2, 0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
        0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65, 0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
        0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4, 0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
        0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df, 0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
        0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30, 0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
        0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8, 0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
        0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec, 0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
        0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178, 0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
        0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c, 0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817];
    let mut w = [0u64; 80];
    w[..16].copy_from_slice(w0);
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
    let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
    for (i, &wi) in w.iter().enumerate() {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(wi);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g; g = f; f = e; e = d.wrapping_add(t1);
        d = c; c = b; b = a; a = t1.wrapping_add(t2);
    }
    for (i, v) in [a, b, c, d, e, f, g, hh].iter().enumerate() { h[i] = h[i].wrapping_add(*v); }
}

fn sha256_block(h: &mut [u32; 16], w0: &[u32]) {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2];
    let mut w = [0u32; 64];
    w[..16].copy_from_slice(&w0[..16]);
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
    }
    let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) = (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g; g = f; f = e; e = d.wrapping_add(t1); d = c; c = b; b = a; a = t1.wrapping_add(t2);
    }
    h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b); h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f); h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
}

impl Device for Sha {
    fn read(&mut self, off: u32) -> u32 { Sha::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Sha::write(self, off, v); WriteEffect::NONE }
    fn debug(&mut self, on: bool) { self.dbg = on; }
}

impl Default for Sha { fn default() -> Self { Self::new() } }

#[cfg(test)]
mod sha_tests {
    use super::*;

    /// Hash one padded block through the register interface and read the digest back the way
    /// `sha_ll_read_digest` does — a plain word copy, no swapping in software.
    fn digest(mode: u32, msg: &[u8], out_bytes: usize) -> String {
        let block = if mode >= 3 { 128 } else { 64 };
        let mut b = vec![0u8; block];
        b[..msg.len()].copy_from_slice(msg);
        b[msg.len()] = 0x80;
        let bits = (msg.len() as u64) * 8;
        b[block - 8..].copy_from_slice(&bits.to_be_bytes());
        let mut s = Sha::new();
        s.write(0x0, mode);
        s.hash_block(&b, true);
        let mut out = Vec::new();
        for i in 0..out_bytes / 4 { out.extend_from_slice(&s.read(0x40 + 4 * i as u32).to_le_bytes()); }
        out.iter().map(|x| format!("{:02x}", x)).collect()
    }

    #[test]
    fn known_answer_vectors() {
        assert_eq!(digest(0, b"abc", 20), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(digest(1, b"abc", 28), "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7");
        assert_eq!(digest(2, b"abc", 32), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(digest(3, b"abc", 48), "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7");
        assert_eq!(digest(4, b"abc", 64), "ddaf35a193617abacc417349ae204131\
12e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f");
    }

    #[test]
    fn dma_block_count_ignores_reserved_bits() {
        // ESP32-S3 TRM, register 18.12: SHA_DMA_BLOCK_NUM[5:0].
        let mut sha = Sha::new();
        sha.write(0x0c, u32::MAX);
        assert_eq!(sha.block_num, 63);
        assert_eq!(sha.read(0x0c), 63);
    }
}
