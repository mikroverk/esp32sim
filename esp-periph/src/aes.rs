use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;

// ------------------------------------------------------------------ AES accelerator (0x6003A000)
/// The block-mode AES accelerator. Firmware writes the key, the mode and one 16-byte block, pulses
/// TRIGGER and polls STATE until it reads idle again. Used by mbedTLS and — the reason it is here —
/// by the WPA supplicant to unwrap the group key during the four-way handshake.
pub struct Aes { pub key: [u32; 8], pub text_in: [u32; 4], pub text_out: [u32; 4], pub mode: u32, pub blocks: u64,
                 pub dma: bool, pub block_mode: u32, pub num_blocks: u32, pub iv: [u32; 4], pub state: u32,
                 pub dma_pending: bool, pub int_raw: u32, pub int_ena: u32, ram: RegRam }
impl Aes {
    pub fn new() -> Self { Aes { key: [0; 8], text_in: [0; 4], text_out: [0; 4], mode: 0, blocks: 0, dma: false, block_mode: 0,
                                num_blocks: 0, iv: [0; 4], state: 0, dma_pending: false, int_raw: 0, int_ena: 0, ram: RegRam::new() } }
    pub fn irq(&self) -> bool { self.int_raw & self.int_ena != 0 }
    /// key bytes selected by the mode register (0/1/2 = 128/192/256, +4 = decrypt)
    pub fn key_bytes(&self) -> Vec<u8> {
        let words = ((self.mode & 3) + 2) as usize * 2;
        let mut k = Vec::with_capacity(words * 4);
        for w in &self.key[..words.min(8)] { k.extend_from_slice(&w.to_le_bytes()); }
        k
    }
    pub fn decrypting(&self) -> bool { self.mode & 4 != 0 }
    /// Transform DMA input with the selected chaining mode, updating the IV and block counter.
    /// A short final block is zero-padded, matching the register-mode block input.
    pub fn transform_blocks(&mut self, input: &[u8]) -> Vec<u8> {
        let key = self.key_bytes();
        let decrypt = self.decrypting();
        let mut iv = [0u8; 16];
        for (i, word) in self.iv.iter().enumerate() { iv[4 * i..4 * i + 4].copy_from_slice(&word.to_le_bytes()); }
        let mut output = Vec::with_capacity(input.len());
        for chunk in input.chunks(16) {
            let mut block = [0u8; 16];
            block[..chunk.len()].copy_from_slice(chunk);
            let out = match self.block_mode {
                1 => { // CBC
                    let ciphertext = block;
                    if !decrypt { for i in 0..16 { block[i] ^= iv[i]; } }
                    let mut out = crate::crypto::aes_block(&key, &block, decrypt);
                    if decrypt { for i in 0..16 { out[i] ^= iv[i]; } iv = ciphertext; } else { iv = out; }
                    out
                }
                2 | 3 => { // OFB and CTR both encrypt the IV to generate a keystream.
                    let stream = crate::crypto::aes_block(&key, &iv, false);
                    for i in 0..16 { block[i] ^= stream[i]; }
                    if self.block_mode == 2 { iv = stream; }
                    else { for byte in iv.iter_mut().rev() { *byte = byte.wrapping_add(1); if *byte != 0 { break; } } }
                    block
                }
                4 => { // CFB8: one forward-cipher call per byte; the IV shifts in the ciphertext byte.
                    for byte in block.iter_mut() {
                        let stream = crate::crypto::aes_block(&key, &iv, false);
                        let input = *byte;
                        *byte ^= stream[0];
                        iv.copy_within(1.., 0);
                        iv[15] = if decrypt { input } else { *byte };
                    }
                    block
                }
                5 => { // CFB128: the next IV is the ciphertext block. A zero-padded final block leaves the
                       // ciphertext bytes then the keystream in the output, as IDF's partial-block path expects.
                    let ciphertext = block;
                    let stream = crate::crypto::aes_block(&key, &iv, false);
                    for i in 0..16 { block[i] ^= stream[i]; }
                    iv = if decrypt { ciphertext } else { block };
                    block
                }
                _ => crate::crypto::aes_block(&key, &block, decrypt), // ECB (0); GCM (6) is not modelled
            };
            output.extend_from_slice(&out);
            self.blocks += 1;
        }
        for (word, bytes) in self.iv.iter_mut().zip(iv.as_chunks::<4>().0) {
            *word = u32::from_le_bytes(*bytes);
        }
        output
    }
    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            0x00..=0x1c => self.key[(off / 4) as usize],
            0x20..=0x2c => self.text_in[((off - 0x20) / 4) as usize],
            0x30..=0x3c => self.text_out[((off - 0x30) / 4) as usize],
            0x40 => self.mode,
            0x4c => self.state,                          // 0 idle, 2 done (DMA mode waits for done)
            0x50..=0x5c => self.iv[((off - 0x50) / 4) as usize],
            0x90 => self.dma as u32, 0x94 => self.block_mode, 0x98 => self.num_blocks,
            0xb0 => self.int_ena,
            _ => self.ram.read(off),
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x00..=0x1c => self.key[(off / 4) as usize] = v,
            0x20..=0x2c => self.text_in[((off - 0x20) / 4) as usize] = v,
            0x40 => self.mode = v,
            0x50..=0x5c => self.iv[((off - 0x50) / 4) as usize] = v,
            0x90 => self.dma = v & 1 != 0,
            0x94 => self.block_mode = v,
            0x98 => self.num_blocks = v,
            0xac => { if v & 1 != 0 { self.int_raw = 0; } }
            0xb0 => self.int_ena = v,
            0xb8 => { self.state = 0; self.dma_pending = false; }              // DMA_EXIT
            0x48 => if v & 1 != 0 {
                if self.dma { self.state = 1; self.dma_pending = true; }        // the bus walks the descriptors
                else { self.transform(); self.state = 0; }
            },
            _ => self.ram.write(off, v),
        }
    }
    fn transform(&mut self) {
        let key = self.key_bytes();
        let mut block = [0u8; 16];
        for (i, w) in self.text_in.iter().enumerate() { block[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes()); }
        let out = crate::crypto::aes_block(&key, &block, self.decrypting());
        for i in 0..4 { self.text_out[i] = u32::from_le_bytes([out[4 * i], out[4 * i + 1], out[4 * i + 2], out[4 * i + 3]]); }
        self.blocks += 1;
    }
}

impl Default for Aes { fn default() -> Self { Self::new() } }

impl Device for Aes {
    fn read(&mut self, off: u32) -> u32 { Aes::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Aes::write(self, off, v); WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { self.irq() as u64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn accelerator(block_mode: u32, decrypt: bool, iv: &str) -> Aes {
        let mut aes = Aes::new();
        let key = hex("2b7e151628aed2a6abf7158809cf4f3c");
        for (i, word) in key.as_chunks::<4>().0.iter().enumerate() {
            aes.write(i as u32 * 4, u32::from_le_bytes(*word));
        }
        for (i, word) in hex(iv).as_chunks::<4>().0.iter().enumerate() {
            aes.write(0x50 + i as u32 * 4, u32::from_le_bytes(*word));
        }
        aes.write(0x40, if decrypt { 4 } else { 0 });
        aes.write(0x94, block_mode);
        aes
    }

    #[test]
    fn chaining_matches_nist_vectors_across_dma_requests() {
        // NIST SP 800-38A appendix F: first two AES-128 blocks for ECB, CBC, OFB, CTR, CFB8 (F.3.7, which
        // lists 18 bytes; the rest computed with pyca/cryptography 44.0.3) and CFB128 (F.3.13).
        // https://nvlpubs.nist.gov/nistpubs/Legacy/SP/nistspecialpublication800-38a.pdf
        let plain = hex("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51");
        for (mode, initial_iv, cipher, final_iv) in [
            (0, "000102030405060708090a0b0c0d0e0f",
                "3ad77bb40d7a3660a89ecaf32466ef97f5d3d58503b9699de785895a96fdbaaf", "000102030405060708090a0b0c0d0e0f"),
            (1, "000102030405060708090a0b0c0d0e0f",
                "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2", "5086cb9b507219ee95db113a917678b2"),
            (2, "000102030405060708090a0b0c0d0e0f",
                "3b3fd92eb72dad20333449f8e83cfb4a7789508d16918f03f53c52dac54ed825", "d9a4dada0892239f6b8b3d7680e15674"),
            (3, "f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff",
                "874d6191b620e3261bef6864990db6ce9806f66b7970fdff8617187bb9fffdff", "f0f1f2f3f4f5f6f7f8f9fafbfcfdff01"),
            (4, "000102030405060708090a0b0c0d0e0f",
                "3b79424c9c0dd436bace9e0ed4586a4f32b9ded50ae3ba69d472e88267fb5052", "32b9ded50ae3ba69d472e88267fb5052"),
            (5, "000102030405060708090a0b0c0d0e0f",
                "3b3fd92eb72dad20333449f8e83cfb4ac8a64537a0b3a93fcde3cdad9f1ce58b", "c8a64537a0b3a93fcde3cdad9f1ce58b"),
        ] {
            let cipher = hex(cipher);
            for decrypt in [false, true] {
                let (input, expected) = if decrypt { (&cipher, &plain) } else { (&plain, &cipher) };
                let mut aes = accelerator(mode, decrypt, initial_iv);
                let mut output = aes.transform_blocks(&input[..16]);
                output.extend(aes.transform_blocks(&input[16..]));
                assert_eq!(&output, expected, "mode {mode}, decrypt {decrypt}");
                assert_eq!(aes.blocks, 2);
                let iv: Vec<u8> = aes.iv.iter().flat_map(|word| word.to_le_bytes()).collect();
                assert_eq!(iv, hex(final_iv), "mode {mode}, decrypt {decrypt}");
            }
        }
    }

    #[test]
    fn cfb128_short_final_block_leaves_the_keystream_after_the_data() {
        // IDF's esp_aes_crypt_cfb128 (v5.5.4, port/aes/dma/esp_aes.c) sends a partial final block through
        // DMA and keeps the whole output block as its next IV: ciphertext bytes, then keystream.
        let plain = hex("6bc1bee22e409f96e93d7e117393172aae2d8a57");
        let mut aes = accelerator(5, false, "000102030405060708090a0b0c0d0e0f");
        assert_eq!(aes.transform_blocks(&plain),
                   hex("3b3fd92eb72dad20333449f8e83cfb4ac8a64537beb005a35354a201dab36bda"));
    }
}
