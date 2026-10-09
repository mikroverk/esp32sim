//! Classic ESP32 CPU-driven crypto registers; arithmetic is shared with the other chips.
//! ESP-IDF v5.5.4 components/soc/esp32/include/soc/hwcrypto_reg.h:12-64;
//! register/soc/dport_reg.h:98-104 defines clock/reset bits.
//! Register completion is synchronous and inferred, not hardware timing validation.
use esp_periph::{crypto::aes_block, Device, Sha, WriteEffect};

pub struct ClassicAes {
    key: [u32; 8],
    text: [u32; 4],
    mode: u32,
    endian: u32,
    pub blocks: u64,
    debug: bool,
}

impl Default for ClassicAes {
    fn default() -> Self {
        Self::new()
    }
}

impl ClassicAes {
    pub fn new() -> Self {
        Self {
            key: [0; 8],
            text: [0; 4],
            mode: 0,
            endian: 0x3f,
            blocks: 0,
            debug: false,
        }
    }

    pub fn reset(&mut self) { *self = Self::new(); }

    fn endian_words(words: &mut [u32], endian: u32) {
        if endian & 2 == 0 {
            words.reverse();
        }
        if endian & 1 == 0 {
            for word in words {
                *word = word.swap_bytes();
            }
        }
    }

    fn start(&mut self) {
        let key_len = match self.mode & 3 {
            0 => 16,
            1 => 24,
            2 => 32,
            _ => return,
        };
        let mut key = self.key;
        Self::endian_words(&mut key[..key_len / 4], self.endian);
        let key: [u8; 32] = std::array::from_fn(|i| key[i / 4].to_le_bytes()[i % 4]);
        let mut text = self.text;
        Self::endian_words(&mut text, self.endian >> 2);
        let input = std::array::from_fn(|i| text[i / 4].to_le_bytes()[i % 4]);
        let output = aes_block(&key[..key_len], &input, self.mode & 4 != 0);
        self.text = std::array::from_fn(|i| {
            u32::from_le_bytes(output[4 * i..4 * i + 4].try_into().unwrap())
        });
        Self::endian_words(&mut self.text, self.endian >> 4);
        self.blocks += 1;
        if self.debug {
            eprintln!("[esp32-aes] block={} mode={}", self.blocks, self.mode);
        }
    }
}

impl Device for ClassicAes {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x04 => 1,
            0x08 => self.mode,
            0x10..=0x2c => self.key[((off - 0x10) / 4) as usize],
            0x30..=0x3c => self.text[((off - 0x30) / 4) as usize],
            0x40 => self.endian,
            _ => 0,
        }
    }

    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        match off {
            0x00 if v & 1 != 0 => self.start(),
            0x08 => self.mode = v & 7,
            0x10..=0x2c => self.key[((off - 0x10) / 4) as usize] = v,
            0x30..=0x3c => self.text[((off - 0x30) / 4) as usize] = v,
            0x40 => self.endian = v & 0x3f,
            _ => {}
        }
        WriteEffect::NONE
    }

    fn debug(&mut self, on: bool) {
        self.debug = on;
    }
}

pub struct ClassicSha {
    // SHA-384 and SHA-512 share one engine; SHA-1 and SHA-256 keep independent state.
    cores: [Sha; 3],
    text: [u32; 32],
    debug: bool,
}

impl Default for ClassicSha {
    fn default() -> Self {
        Self::new()
    }
}

impl ClassicSha {
    pub fn new() -> Self {
        Self {
            cores: std::array::from_fn(|_| Sha::new()),
            text: [0; 32],
            debug: false,
        }
    }

    pub fn reset(&mut self) { *self = Self::new(); }
}

impl Device for ClassicSha {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x00..=0x7c => self.text[(off / 4) as usize],
            _ => 0,
        }
    }

    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        match off {
            0x00..=0x7c => self.text[(off / 4) as usize] = v,
            0x80..=0xb8 if v & 1 != 0 => {
                let algorithm = ((off - 0x80) / 16) as usize;
                let core = &mut self.cores[algorithm.min(2)];
                match off & 15 {
                    0 | 4 => {
                        core.mode = [0, 2, 3, 4][algorithm];
                        for (dst, src) in core.m.iter_mut().zip(self.text) {
                            *dst = src.swap_bytes();
                        }
                        core.write(if off & 15 == 0 { 0x10 } else { 0x14 }, 1);
                        if self.debug {
                            eprintln!(
                                "[esp32-sha] algorithm={} block={} first={}",
                                [1, 256, 384, 512][algorithm],
                                core.blocks,
                                off & 15 == 0
                            );
                        }
                    }
                    8 => {
                        let words = [5, 8, 16, 16][algorithm];
                        self.text[..words].copy_from_slice(&core.h[..words]);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        WriteEffect::NONE
    }

    fn debug(&mut self, on: bool) {
        self.debug = on;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_aes_words(aes: &mut ClassicAes, base: u32, bytes: &[u8], endian: u32) {
        for (i, word) in bytes.as_chunks::<4>().0.iter().enumerate() {
            let index = if endian & 2 != 0 {
                i
            } else {
                bytes.len() / 4 - 1 - i
            };
            let word = *word;
            aes.write(
                base + index as u32 * 4,
                if endian & 1 != 0 {
                    u32::from_le_bytes(word)
                } else {
                    u32::from_be_bytes(word)
                },
            );
        }
    }

    #[test]
    fn aes_fips197_all_key_lengths_directions_and_endianness() {
        let plain = b"\x00\x11\x22\x33\x44\x55\x66\x77\x88\x99\xaa\xbb\xcc\xdd\xee\xff".to_vec();
        let key: Vec<u8> = (0..32).collect();
        for (mode, expected) in [
            b"\x69\xc4\xe0\xd8\x6a\x7b\x04\x30\xd8\xcd\xb7\x80\x70\xb4\xc5\x5a".as_slice(),
            b"\xdd\xa9\x7c\xa4\x86\x4c\xdf\xe0\x6e\xaf\x70\xa0\xec\x0d\x71\x91".as_slice(),
            b"\x8e\xa2\xb7\xca\x51\x67\x45\xbf\xea\xfc\x49\x90\x4b\x49\x60\x89".as_slice(),
        ]
        .iter()
        .enumerate()
        {
            let cipher = expected.to_vec();
            for endian in 0..64 {
                let mut aes = ClassicAes::new();
                assert_eq!(aes.read(4), 1);
                assert_eq!(aes.read(0x40), 0x3f);
                aes.write(0x40, endian);
                write_aes_words(&mut aes, 0x10, &key[..16 + mode * 8], endian);
                for (direction, input, expected) in [(0, &plain, &cipher), (4, &cipher, &plain)] {
                    aes.write(8, mode as u32 | direction);
                    write_aes_words(&mut aes, 0x30, input, endian >> 2);
                    aes.write(0, 0);
                    assert_eq!(aes.blocks, direction as u64 / 4);
                    aes.write(0, 1);
                    let mut got = Vec::new();
                    for i in 0..4 {
                        let index = if endian & 32 != 0 { i } else { 3 - i };
                        let word = aes.read(0x30 + 4 * index);
                        got.extend_from_slice(&if endian & 16 != 0 {
                            word.to_le_bytes()
                        } else {
                            word.to_be_bytes()
                        });
                    }
                    assert_eq!(
                        &got, expected,
                        "mode={mode} endian={endian} direction={direction}"
                    );
                    assert_eq!(aes.read(0), 0);
                    assert_eq!(aes.read(4), 1);
                }
            }
        }
    }

    fn padded(message: &[u8], block: usize) -> Vec<u8> {
        let mut bytes = message.to_vec();
        bytes.push(0x80);
        let length_bytes = if block == 128 { 16 } else { 8 };
        while !(bytes.len() + length_bytes).is_multiple_of(block) {
            bytes.push(0);
        }
        bytes.resize(bytes.len() + length_bytes - 8, 0);
        bytes.extend_from_slice(&(message.len() as u64 * 8).to_be_bytes());
        bytes
    }

    fn sha_block(sha: &mut ClassicSha, algorithm: u32, block: &[u8], first: bool) {
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            sha.write(i as u32 * 4, u32::from_be_bytes(*word));
        }
        sha.write(0x80 + algorithm * 16 + if first { 0 } else { 4 }, 1);
        assert_eq!(sha.read(0x8c + algorithm * 16), 0);
    }

    fn sha_digest(sha: &mut ClassicSha, algorithm: u32) -> Vec<u8> {
        sha.write(0x88 + algorithm * 16, 1);
        (0..[5, 8, 12, 16][algorithm as usize])
            .flat_map(|i| sha.read(i * 4).to_be_bytes())
            .collect()
    }

    #[test]
    fn sha_all_algorithms_start_continue_load() {
        let single = [
            b"\xa9\x99\x3e\x36\x47\x06\x81\x6a\xba\x3e\x25\x71\x78\x50\xc2\x6c\x9c\xd0\xd8\x9d".as_slice(),
            b"\xba\x78\x16\xbf\x8f\x01\xcf\xea\x41\x41\x40\xde\x5d\xae\x22\x23\xb0\x03\x61\xa3\x96\x17\x7a\x9c\xb4\x10\xff\x61\xf2\x00\x15\xad".as_slice(),
            b"\xcb\x00\x75\x3f\x45\xa3\x5e\x8b\xb5\xa0\x3d\x69\x9a\xc6\x50\x07\x27\x2c\x32\xab\x0e\xde\xd1\x63\x1a\x8b\x60\x5a\x43\xff\x5b\xed\x80\x86\x07\x2b\xa1\xe7\xcc\x23\x58\xba\xec\xa1\x34\xc8\x25\xa7".as_slice(),
            b"\xdd\xaf\x35\xa1\x93\x61\x7a\xba\xcc\x41\x73\x49\xae\x20\x41\x31\x12\xe6\xfa\x4e\x89\xa9\x7e\xa2\x0a\x9e\xee\xe6\x4b\x55\xd3\x9a\x21\x92\x99\x2a\x27\x4f\xc1\xa8\x36\xba\x3c\x23\xa3\xfe\xeb\xbd\x45\x4d\x44\x23\x64\x3c\xe8\x0e\x2a\x9a\xc9\x4f\xa5\x4c\xa4\x9f".as_slice(),
        ];
        // 200 ASCII 'a' bytes; independently generated with Python hashlib/OpenSSL.
        let multi = [
            b"\xe6\x1c\xff\xfe\x0d\x91\x95\xa5\x25\xfc\x6c\xf0\x6c\xa2\xd7\x71\x19\xc2\x4a\x40".as_slice(),
            b"\xc2\xa9\x08\xd9\x8f\x5d\xf9\x87\xad\xe4\x1b\x5f\xce\x21\x30\x67\xef\xbc\xc2\x1e\xf2\x24\x02\x12\xa4\x1e\x54\xb5\xe7\xc2\x8a\xe5".as_slice(),
            b"\x06\x91\xb6\xe9\x78\x61\x4b\x67\xd6\x05\x57\xb2\xa2\xcd\xdd\x53\x40\x65\x08\x52\x2e\xfa\x21\xc6\x24\xdb\xbf\xa8\xab\x6e\x72\x6d\x5c\x58\x6b\x48\x9c\x7c\x09\xf2\x41\x09\xa6\x4c\x10\x21\x1d\x48".as_slice(),
            b"\x4b\x11\x45\x9c\x33\xf5\x2a\x22\xee\x82\x36\x78\x27\x14\xc1\x50\xa3\xb2\xc6\x09\x94\xe9\xac\xee\x17\xfe\x68\x94\x7a\x3e\x67\x89\xf3\x1e\x76\x68\x39\x45\x92\xda\x7b\xef\x82\x7c\xdd\xca\x88\xc4\xe6\xf8\x6e\x4d\xf7\xed\x1a\xe6\xcb\xa7\x1f\x3e\x98\xfa\xee\x9f".as_slice(),
        ];
        let mut sha = ClassicSha::new();
        for algorithm in 0..4 {
            let block_len = if algorithm < 2 { 64 } else { 128 };
            for (message, expected) in [
                (b"abc".as_slice(), single[algorithm]),
                (&[b'a'; 200], multi[algorithm]),
            ] {
                for (i, block) in padded(message, block_len)
                    .chunks_exact(block_len)
                    .enumerate()
                {
                    sha_block(&mut sha, algorithm as u32, block, i == 0);
                    assert_eq!(
                        sha.read(0),
                        u32::from_be_bytes(block[..4].try_into().unwrap())
                    );
                    // LOAD exposes the current state without consuming or resetting it.
                    sha_digest(&mut sha, algorithm as u32);
                }
                assert_eq!(sha_digest(&mut sha, algorithm as u32), expected.to_vec());
                assert_eq!(sha_digest(&mut sha, algorithm as u32), expected.to_vec());
            }
        }
        assert_eq!(sha.cores.each_ref().map(|core| core.blocks), [5, 5, 6]);
    }

    #[test]
    fn sha_parallel_engines_and_shared_sha384_sha512_state() {
        let mut sha = ClassicSha::new();
        let message = [b'a'; 200];
        let blocks = padded(&message, 64);
        let sha512 = padded(b"abc", 128);
        for (i, block) in blocks.as_chunks::<64>().0.iter().enumerate() {
            sha_block(&mut sha, 0, block, i == 0);
            sha_block(&mut sha, 1, block, i == 0);
            sha_block(&mut sha, 3, &sha512, true);
        }
        assert_eq!(
            sha_digest(&mut sha, 0),
            b"\xe6\x1c\xff\xfe\x0d\x91\x95\xa5\x25\xfc\x6c\xf0\x6c\xa2\xd7\x71\x19\xc2\x4a\x40".to_vec()
        );
        assert_eq!(
            sha_digest(&mut sha, 1),
            b"\xc2\xa9\x08\xd9\x8f\x5d\xf9\x87\xad\xe4\x1b\x5f\xce\x21\x30\x67\xef\xbc\xc2\x1e\xf2\x24\x02\x12\xa4\x1e\x54\xb5\xe7\xc2\x8a\xe5".to_vec()
        );
        let digest512 = sha_digest(&mut sha, 3);
        assert_eq!(sha_digest(&mut sha, 2), digest512[..48]);
        assert_eq!(
            sha.read(48),
            u32::from_be_bytes(digest512[48..52].try_into().unwrap())
        );
    }

    #[test]
    fn reset_clears_crypto_state_and_reserved_modes_do_not_execute() {
        let mut aes = ClassicAes::new();
        aes.write(0, 1);
        assert_eq!(aes.blocks, 1);
        aes.reset();
        assert_eq!(aes.read(0x30), 0);
        for mode in [3, 7] { aes.write(8, mode); aes.write(0, 1); }
        assert_eq!(aes.blocks, 0);
        let mut sha = ClassicSha::new();
        sha_block(&mut sha, 1, &padded(b"abc", 64), true);
        assert_eq!(sha.cores[1].blocks, 1);
        sha.reset();
        sha.write(0x98, 1);
        assert_eq!(sha.read(0), 0);
        sha.write(0x90, 0);
        assert_eq!(sha.cores[1].blocks, 0);
    }
}
/// Classic RSA memory and command layout. MULT modes 0-7 are Montgomery steps,
/// so IDF can chain them when implementing modular multiplication/exponentiation.
pub struct ClassicRsa {
    pub inner: esp_periph::Rsa,
    modexp_mode: u32,
    mult_mode: u32,
}
impl ClassicRsa {
    pub fn new() -> Self {
        let mut inner = esp_periph::Rsa::new();
        inner.int_ena = 1;
        Self { inner, modexp_mode: 0, mult_mode: 0 }
    }
    pub fn reset(&mut self) { *self = Self::new(); }

    fn montgomery(&self, words: usize) -> Vec<u32> {
        let modulus = &self.inner.mem[..words];
        let mut product =
            esp_periph::crypto::bn_mul(&self.inner.mem[384..384 + words], &self.inner.mem[128..128 + words]);
        product.resize(2 * words + 2, 0);
        for i in 0..words {
            let q = product[i].wrapping_mul(self.inner.m_prime);
            let mut carry = 0u64;
            for (j, &m) in modulus.iter().enumerate() {
                let sum = product[i + j] as u64 + q as u64 * m as u64 + carry;
                product[i + j] = sum as u32;
                carry = sum >> 32;
            }
            let mut j = i + words;
            while carry != 0 {
                let sum = product[j] as u64 + carry;
                product[j] = sum as u32;
                carry = sum >> 32;
                j += 1;
            }
        }
        esp_periph::crypto::bn_mod(&product[words..], modulus)
    }

    fn start(&mut self, exponentiate: bool) {
        if exponentiate {
            self.inner.length = 16 * (self.modexp_mode + 1) - 1;
            self.inner.write(0x80c, 1);
        } else {
            match self.mult_mode {
                0..=7 => {
                    let words = 16 * (self.mult_mode as usize + 1);
                    self.inner.finish(self.montgomery(words), words);
                }
                9 | 11 | 13 | 15 => {
                    self.inner.length = 16 * (self.mult_mode - 7) - 1;
                    self.inner.write(0x814, 1);
                }
                _ => {}
            }
        }
    }
}

impl Default for ClassicRsa {
    fn default() -> Self {
        Self::new()
    }
}

impl Device for ClassicRsa {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x000..=0x7fc => self.inner.mem[(off / 4) as usize],
            0x800 => self.inner.m_prime,
            0x804 => self.modexp_mode,
            0x80c => self.mult_mode,
            0x814 => self.inner.int_raw,
            0x818 => self.inner.int_ena,
            _ => 0,
        }
    }

    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        match off {
            0x000..=0x7fc => self.inner.mem[(off / 4) as usize] = value,
            0x800 => self.inner.m_prime = value,
            0x804 => self.modexp_mode = value & 7,
            0x80c => self.mult_mode = value & 15,
            0x808 if value & 1 != 0 => self.start(true),
            0x810 if value & 1 != 0 => self.start(false),
            0x814 if value & 1 != 0 => self.inner.int_raw = 0,
            _ => (),
        }
        WriteEffect::NONE
    }

    fn irq_sources(&self) -> u64 {
        self.inner.irq() as u64
    }
    fn debug(&mut self, on: bool) {
        self.inner.dbg = on;
    }
}

#[cfg(test)]
mod rsa_tests {
    use super::*;

    fn load(rsa: &mut ClassicRsa, base: u32, words: &[u32]) {
        for (i, &word) in words.iter().enumerate() {
            rsa.write(base + 4 * i as u32, word);
        }
    }

    #[test]
    fn classic_rsa_modexp_maps_every_operand_width() {
        for mode in 0..8 {
            let mut rsa = ClassicRsa::new();
            let words = 16 * (mode + 1);
            for i in 0..words { rsa.write(4 * i, u32::MAX); }
            rsa.write(0x400, 1);
            rsa.write(0x600 + 4 * (words - 1), 0x1234);
            rsa.write(0x804, mode);
            rsa.write(0x808, 1);
            assert_eq!(rsa.read(0x200 + 4 * (words - 1)), 0x1234);
            for i in 0..words - 1 { assert_eq!(rsa.read(0x200 + 4 * i), 0); }
        }
    }

    #[test]
    fn classic_rsa_modexp_registers_interrupt_reset_and_clock() {
        let mut rsa = ClassicRsa::new();
        assert_eq!(rsa.read(0x818), 1);
        rsa.write(0x000, 497);
        rsa.write(0x200, 86); // R² mod 497, R=2^512
        rsa.write(0x800, 0x28b130ef); // -497^-1 mod 2^32
        rsa.write(0x400, 13);
        rsa.write(0x600, 4);
        rsa.write(0x808, 0);
        assert_eq!(rsa.inner.ops, 0);
        rsa.write(0x808, 1);
        assert_eq!(rsa.read(0x200), 445); // Python: pow(4, 13, 497)
        assert_eq!(rsa.read(0x814), 1);
        assert_eq!(rsa.irq_sources(), 1);
        rsa.write(0x814, 0);
        assert_eq!(rsa.read(0x814), 1);
        rsa.write(0x814, 1);
        assert_eq!(rsa.irq_sources(), 0);
        rsa.write(0x400, 0);
        rsa.write(0x200, 86);
        rsa.write(0x808, 1);
        assert_eq!(rsa.read(0x200), 1);
        rsa.write(0x804, u32::MAX);
        rsa.write(0x80c, u32::MAX);
        assert_eq!(rsa.read(0x804), 7);
        assert_eq!(rsa.read(0x80c), 15);
        rsa.reset();
        assert_eq!(rsa.inner.ops, 0);
        assert_eq!(rsa.read(0x818), 1);
        assert_eq!(rsa.read(0x814), 0);
        for off in (0..0x800).step_by(4) {
            assert_eq!(rsa.read(off), 0);
        }
    }

    #[test]
    fn classic_rsa_montgomery_steps_match_independent_values() {
        let mut rsa = ClassicRsa::new();
        rsa.write(0x000, 97);
        rsa.write(0x800, 0xa0fd5c5f); // -97^-1 mod 2^32
        rsa.write(0x600, 5);
        rsa.write(0x200, 61); // Python: pow(2, 1024, 97), R^2 mod M
        rsa.write(0x810, 1);
        assert_eq!(rsa.read(0x200), 78); // 5*R mod 97, R=2^512
        rsa.write(0x600, 17);
        rsa.write(0x810, 1);
        assert_eq!(rsa.read(0x200), 85); // 5*17 mod 97

        // Python: ((m-1)*(m-2)*pow(2**512, -1, m)) % m, m=2**512-59.
        let mut m = [u32::MAX; 16];
        m[0] -= 58;
        let mut x = m;
        x[0] -= 1;
        let mut y = m;
        y[0] -= 2;
        load(&mut rsa, 0x000, &m);
        load(&mut rsa, 0x600, &x);
        load(&mut rsa, 0x200, &y);
        rsa.write(0x800, 0xa08ad8f3);
        rsa.write(0x810, 1);
        let expected = "49c34115b1e5f75270d0456c797dd49c34115b1e5f75270d0456c797dd49c34115b1e5f75270d0456c797dd49c34115b1e5f75270d0456c797dd49c34115b1d5";
        for (i, chunk) in expected.as_bytes().rchunks(8).enumerate() {
            let word = u32::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap();
            assert_eq!(rsa.read(0x200 + 4 * i as u32), word);
        }
    }

    #[test]
    fn classic_rsa_all_montgomery_lengths_carry_and_zero() {
        for mode in 0..8 {
            let words = 16 * (mode + 1);
            let mut rsa = ClassicRsa::new();
            let m = vec![u32::MAX; words]; // M=R-1, so R^-1=1 mod M.
            let mut x = m.clone();
            x[0] -= 1;
            let mut y = m.clone();
            y[0] -= 2;
            load(&mut rsa, 0, &m);
            load(&mut rsa, 0x600, &x);
            load(&mut rsa, 0x200, &y);
            rsa.write(0x800, 1);
            rsa.write(0x80c, mode as u32);
            rsa.write(0x810, 1);
            assert_eq!(rsa.read(0x200), 2); // (-1)*(-2) mod M
            for i in 1..words {
                assert_eq!(rsa.read(0x200 + 4 * i as u32), 0);
            }
            load(&mut rsa, 0x600, &vec![0; words]);
            rsa.write(0x810, 1);
            for i in 0..words {
                assert_eq!(rsa.read(0x200 + 4 * i as u32), 0);
            }
        }
    }

    #[test]
    fn classic_rsa_plain_multiply_lengths_and_invalid_mode() {
        for mode in [9, 11, 13, 15] {
            let words = 8 * (mode - 7);
            let mut rsa = ClassicRsa::new();
            load(&mut rsa, 0x600, &vec![u32::MAX; words]);
            load(&mut rsa, 0x200 + 4 * words as u32, &vec![u32::MAX; words]);
            rsa.write(0x80c, mode as u32);
            rsa.write(0x810, 1);
            assert_eq!(rsa.read(0x200), 1); // (R-1)^2=R^2-2R+1
            for i in 1..words {
                assert_eq!(rsa.read(0x200 + 4 * i as u32), 0);
            }
            assert_eq!(rsa.read(0x200 + 4 * words as u32), u32::MAX - 1);
            for i in words + 1..2 * words {
                assert_eq!(rsa.read(0x200 + 4 * i as u32), u32::MAX);
            }
            rsa.write(0x80c, 8);
            rsa.write(0x810, 1);
            assert_eq!(rsa.inner.ops, 1);
        }
    }
}

impl crate::periph::Peripherals {
    pub(crate) fn sync_crypto(&mut self) {
        let clock = self.dport.ram.read(0x1c);
        let reset = self.dport.ram.read(0x20);
        let rsa_down = self.dport.ram.read(0x490) & 1 != 0;
        // Secure boot holds AES/SHA in reset; digital signature holds AES/RSA.
        if reset & 0x19 != 0 {
            self.aes.reset();
        }
        if reset & 0x0a != 0 {
            self.sha.reset();
        }
        if reset & 0x14 != 0 || rsa_down {
            self.rsa.reset();
        }
        self.crypto_enabled = [clock & 1 != 0 && reset & 0x19 == 0,
            clock & 2 != 0 && reset & 0x0a == 0,
            clock & 4 != 0 && reset & 0x14 == 0 && !rsa_down];
        self.rsa.inner.int_ena = self.crypto_enabled[2] as u32;
    }
}

#[cfg(test)]
mod dport_tests {
    use crate::periph::Peripherals;

    #[test]
    fn each_crypto_command_obeys_its_own_dport_clock() {
        for clock in 0..8 {
            let mut p = Peripherals::new([0; 6]);
            p.write32(0x3ff0_001c, clock);
            p.write32(0x3ff0_2000, 13);
            p.write32(0x3ff0_1000, 1);
            p.write32(0x3ff0_3090, 1);
            p.write32(0x3ff0_2808, 1);
            assert_eq!(p.aes.blocks, u64::from(clock & 1 != 0));
            assert_eq!(p.sha.cores[1].blocks, u64::from(clock & 2 != 0));
            assert_eq!(p.rsa.inner.ops, u64::from(clock & 4 != 0));
        }
    }

    #[test]
    fn crypto_clocks_resets_power_and_rsa_interrupt_routing() {
        let mut p = Peripherals::new([0; 6]);
        assert!(!p.crypto_enabled[0] && !p.crypto_enabled[1] && !p.crypto_enabled[2]);
        assert_eq!(p.read32(0x3ff0_2818), 0);
        p.write32(0x3ff0_001c, 7);
        assert!(p.crypto_enabled[0] && p.crypto_enabled[1] && p.crypto_enabled[2]);
        assert_eq!(p.read32(0x3ff0_2818), 1);

        p.write32(0x3ff0_1030, 0x1234);
        p.write32(0x3ff0_3000, 0x5678);
        p.write32(0x3ff0_2600, 5);
        p.write32(0x3ff0_2400, 3);
        p.write32(0x3ff0_2000, 13);
        p.write32(0x3ff0_2200, 3); // R² mod 13, R=2^512
        p.write32(0x3ff0_2800, 0x3b13b13b); // -13^-1 mod 2^32
        p.write32(0x3ff0_01d0, 7); // RSA source 51, PRO CPU
        p.write32(0x3ff0_02e4, 8); // RSA source 51, APP CPU
        p.write32(0x3ff0_2808, 1);
        assert_eq!(p.read32(0x3ff0_2200), 8);
        assert_ne!(p.cpu_lines(0) & (1 << 7), 0);
        assert_ne!(p.cpu_lines(1) & (1 << 8), 0);
        p.write32(0x3ff0_2814, 1);
        assert_eq!(p.cpu_lines(0) & (1 << 7), 0);
        assert_eq!(p.cpu_lines(1) & (1 << 8), 0);
        assert_eq!(p.read32(0x3ff0_2818), 1);

        p.write32(0x3ff0_001c, 0);
        p.write32(0x3ff0_1000, 1);
        p.write32(0x3ff0_3090, 1);
        p.write32(0x3ff0_2808, 1);
        assert_eq!(p.read32(0x3ff0_1030), 0x1234);
        assert_eq!(p.read32(0x3ff0_2814), 0);
        p.write32(0x3ff0_001c, 7);
        assert_eq!(p.read32(0x3ff0_3000), 0x5678);

        for (reset, aes, sha, rsa) in [
            (1, false, true, true),
            (2, true, false, true),
            (4, true, true, false),
            (8, false, false, true),
            (16, false, true, false),
        ] {
            p.write32(0x3ff0_0020, reset);
            assert_eq!(
                (p.crypto_enabled[0], p.crypto_enabled[1], p.crypto_enabled[2]),
                (aes, sha, rsa)
            );
            p.write32(0x3ff0_0020, 0);
        }
        assert_eq!(p.read32(0x3ff0_1030), 0);
        assert_eq!(p.read32(0x3ff0_3000), 0);
        assert_eq!(p.read32(0x3ff0_2600), 0);
        p.write32(0x3ff0_2600, 42);
        p.write32(0x3ff0_0490, 1);
        assert!(!p.crypto_enabled[2]);
        assert_eq!(p.read32(0x3ff0_2818), 0);
        assert_eq!(p.read32(0x3ff0_2600), 0);
        p.write32(0x3ff0_0490, 0);
        assert!(p.crypto_enabled[2]);
        assert_eq!(p.read32(0x3ff0_2818), 1);
    }
}
