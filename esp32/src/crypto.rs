//! Classic ESP32 CPU-driven crypto registers; arithmetic is shared with the other chips.
use esp_periph::{crypto::aes_block, Device, Sha, WriteEffect};

pub struct ClassicAes {
    key: [u32; 8],
    text: [u32; 4],
    mode: u32,
    endian: u32,
    pub enabled: bool,
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
            enabled: true,
            blocks: 0,
            debug: false,
        }
    }

    pub fn reset(&mut self) {
        let (enabled, blocks, debug) = (self.enabled, self.blocks, self.debug);
        *self = Self::new();
        (self.enabled, self.blocks, self.debug) = (enabled, blocks, debug);
    }

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
            // ponytail: complete blocks synchronously; add latency if software needs busy timing.
            0x00 if v & 1 != 0 && self.enabled => self.start(),
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
    pub enabled: bool,
    pub blocks: [u64; 4],
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
            enabled: true,
            blocks: [0; 4],
            debug: false,
        }
    }

    pub fn reset(&mut self) {
        let (enabled, blocks, debug) = (self.enabled, self.blocks, self.debug);
        *self = Self::new();
        (self.enabled, self.blocks, self.debug) = (enabled, blocks, debug);
    }
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
            0x80..=0xb8 if v & 1 != 0 && self.enabled => {
                let algorithm = ((off - 0x80) / 16) as usize;
                let core = &mut self.cores[algorithm.min(2)];
                match off & 15 {
                    // ponytail: hash synchronously; add latency if software needs busy timing.
                    0 | 4 => {
                        core.mode = [0, 2, 3, 4][algorithm];
                        for (dst, src) in core.m.iter_mut().zip(self.text) {
                            *dst = src.swap_bytes();
                        }
                        core.write(if off & 15 == 0 { 0x10 } else { 0x14 }, 1);
                        self.blocks[algorithm] += 1;
                        if self.debug {
                            eprintln!(
                                "[esp32-sha] algorithm={} block={} first={}",
                                [1, 256, 384, 512][algorithm],
                                self.blocks[algorithm],
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

    fn hex(s: &str) -> Vec<u8> {
        s.as_bytes()
            .as_chunks::<2>().0.iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

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
        let plain = hex("00112233445566778899aabbccddeeff");
        let key: Vec<u8> = (0..32).collect();
        for (mode, expected) in [
            "69c4e0d86a7b0430d8cdb78070b4c55a",
            "dda97ca4864cdfe06eaf70a0ec0d7191",
            "8ea2b7ca516745bfeafc49904b496089",
        ]
        .iter()
        .enumerate()
        {
            let cipher = hex(expected);
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
            "a9993e364706816aba3e25717850c26c9cd0d89d",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        ];
        // 200 ASCII 'a' bytes; independently generated with Python hashlib/OpenSSL.
        let multi = [
            "e61cfffe0d9195a525fc6cf06ca2d77119c24a40",
            "c2a908d98f5df987ade41b5fce213067efbcc21ef2240212a41e54b5e7c28ae5",
            "0691b6e978614b67d60557b2a2cddd53406508522efa21c624dbbfa8ab6e726d5c586b489c7c09f24109a64c10211d48",
            "4b11459c33f52a22ee8236782714c150a3b2c60994e9acee17fe68947a3e6789f31e7668394592da7bef827cddca88c4e6f86e4df7ed1ae6cba71f3e98faee9f",
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
                assert_eq!(sha_digest(&mut sha, algorithm as u32), hex(expected));
                assert_eq!(sha_digest(&mut sha, algorithm as u32), hex(expected));
            }
        }
        assert_eq!(sha.blocks, [5, 5, 3, 3]);
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
            hex("e61cfffe0d9195a525fc6cf06ca2d77119c24a40")
        );
        assert_eq!(
            sha_digest(&mut sha, 1),
            hex("c2a908d98f5df987ade41b5fce213067efbcc21ef2240212a41e54b5e7c28ae5")
        );
        let digest512 = sha_digest(&mut sha, 3);
        assert_eq!(sha_digest(&mut sha, 2), digest512[..48]);
        assert_eq!(
            sha.read(48),
            u32::from_be_bytes(digest512[48..52].try_into().unwrap())
        );
    }

    #[test]
    fn disabled_commands_and_reset_leave_no_crypto_state() {
        let mut aes = ClassicAes::new();
        aes.enabled = false;
        aes.write(0, 1);
        assert_eq!(aes.blocks, 0);
        aes.enabled = true;
        aes.write(0, 1);
        assert_eq!(aes.blocks, 1);
        aes.reset();
        assert_eq!(aes.read(0x30), 0);
        assert_eq!(aes.blocks, 1);
        for mode in [3, 7] {
            aes.write(8, mode);
            aes.write(0, 1);
        }
        assert_eq!(aes.blocks, 1);
        let mut sha = ClassicSha::new();
        sha_block(&mut sha, 1, &padded(b"abc", 64), true);
        sha.enabled = false;
        sha.write(0x94, 1);
        sha.write(0x98, 1);
        assert_eq!(sha.blocks, [0, 1, 0, 0]);
        assert_eq!(sha.read(0), 0x61626380);
        sha.reset();
        assert!(!sha.enabled);
        sha.enabled = true;
        sha.write(0x98, 1);
        assert_eq!(sha.read(0), 0);
        assert_eq!(sha.blocks, [0, 1, 0, 0]);
        sha.write(0x90, 0);
        assert_eq!(sha.blocks, [0, 1, 0, 0]);
    }
}
/// Classic RSA memory and command layout. MULT modes 0-7 are Montgomery steps,
/// so IDF can chain them when implementing modular multiplication/exponentiation.
pub struct ClassicRsa {
    mem: [u32; 512],
    m_prime: u32,
    modexp_mode: u32,
    mult_mode: u32,
    interrupt: bool,
    pub enabled: bool,
    pub ops: u64,
    dbg: bool,
}

impl ClassicRsa {
    pub fn new() -> Self {
        Self {
            mem: [0; 512],
            m_prime: 0,
            modexp_mode: 0,
            mult_mode: 0,
            interrupt: false,
            enabled: true,
            ops: 0,
            dbg: false,
        }
    }

    pub fn reset(&mut self) {
        let (enabled, ops, dbg) = (self.enabled, self.ops, self.dbg);
        *self = Self::new();
        (self.enabled, self.ops, self.dbg) = (enabled, ops, dbg);
    }

    fn montgomery(&self, words: usize) -> Vec<u32> {
        let modulus = &self.mem[..words];
        let mut product =
            esp_periph::crypto::bn_mul(&self.mem[384..384 + words], &self.mem[128..128 + words]);
        product.resize(2 * words + 2, 0);
        for i in 0..words {
            let q = product[i].wrapping_mul(self.m_prime);
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
        if !self.enabled {
            return;
        }
        let (words, result, operation) = if exponentiate {
            let words = 16 * (self.modexp_mode as usize + 1);
            // ponytail: exact exponentiation assumes valid R²/M′ setup; add fault behavior if needed.
            let result = esp_periph::crypto::bn_modexp(
                &self.mem[384..384 + words],
                &self.mem[256..256 + words],
                &self.mem[..words],
            );
            (words, result, "modexp")
        } else {
            match self.mult_mode {
                0..=7 => {
                    let words = 16 * (self.mult_mode as usize + 1);
                    (words, self.montgomery(words), "montgomery")
                }
                9 | 11 | 13 | 15 => {
                    let words = 8 * (self.mult_mode as usize - 7);
                    let result = esp_periph::crypto::bn_mul(
                        &self.mem[384..384 + words],
                        &self.mem[128 + words..128 + 2 * words],
                    );
                    (2 * words, result, "multiply")
                }
                _ => return,
            }
        };
        self.mem[128..128 + words].fill(0);
        self.mem[128..128 + result.len().min(words)]
            .copy_from_slice(&result[..result.len().min(words)]);
        self.interrupt = true;
        self.ops += 1;
        if self.dbg {
            eprintln!(
                "[rsa] op #{} {} words={} z[0]={:08x}",
                self.ops, operation, words, self.mem[128]
            );
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
            0x000..=0x7fc => self.mem[(off / 4) as usize],
            0x800 => self.m_prime,
            0x804 => self.modexp_mode,
            0x80c => self.mult_mode,
            0x814 => self.interrupt as u32,
            0x818 => self.enabled as u32,
            _ => 0,
        }
    }

    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        match off {
            0x000..=0x7fc => self.mem[(off / 4) as usize] = value,
            0x800 => self.m_prime = value,
            0x804 => self.modexp_mode = value & 7,
            0x80c => self.mult_mode = value & 15,
            0x808 if value & 1 != 0 => self.start(true),
            0x810 if value & 1 != 0 => self.start(false),
            0x814 if value & 1 != 0 => self.interrupt = false,
            _ => (),
        }
        WriteEffect::NONE
    }

    fn irq_sources(&self) -> u64 {
        (self.enabled && self.interrupt) as u64
    }
    fn debug(&mut self, on: bool) {
        self.dbg = on;
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
    fn classic_rsa_modexp_registers_interrupt_reset_and_clock() {
        let mut rsa = ClassicRsa::new();
        assert_eq!(rsa.read(0x818), 1);
        rsa.write(0x000, 497);
        rsa.write(0x200, 86); // R² mod 497, R=2^512
        rsa.write(0x800, 0x28b130ef); // -497^-1 mod 2^32
        rsa.write(0x400, 13);
        rsa.write(0x600, 4);
        rsa.write(0x808, 0);
        assert_eq!(rsa.ops, 0);
        rsa.enabled = false;
        rsa.write(0x808, 1);
        assert_eq!(rsa.ops, 0);
        assert_eq!(rsa.read(0x818), 0);
        rsa.enabled = true;
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
        assert_eq!(rsa.ops, 2);
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
            assert_eq!(rsa.ops, 1);
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
        self.aes.enabled = clock & 1 != 0 && reset & 0x19 == 0;
        self.sha.enabled = clock & 2 != 0 && reset & 0x0a == 0;
        self.rsa.enabled = clock & 4 != 0 && reset & 0x14 == 0 && !rsa_down;
    }
}

#[cfg(test)]
mod dport_tests {
    use crate::periph::Peripherals;

    #[test]
    fn crypto_clocks_resets_power_and_rsa_interrupt_routing() {
        let mut p = Peripherals::new([0; 6]);
        assert!(!p.aes.enabled && !p.sha.enabled && !p.rsa.enabled);
        assert_eq!(p.read32(0x3ff0_2818), 0);
        p.write32(0x3ff0_001c, 7);
        assert!(p.aes.enabled && p.sha.enabled && p.rsa.enabled);
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
                (p.aes.enabled, p.sha.enabled, p.rsa.enabled),
                (aes, sha, rsa)
            );
            p.write32(0x3ff0_0020, 0);
        }
        assert_eq!(p.read32(0x3ff0_1030), 0);
        assert_eq!(p.read32(0x3ff0_3000), 0);
        assert_eq!(p.read32(0x3ff0_2600), 0);
        p.write32(0x3ff0_2600, 42);
        p.write32(0x3ff0_0490, 1);
        assert!(!p.rsa.enabled);
        assert_eq!(p.read32(0x3ff0_2818), 0);
        assert_eq!(p.read32(0x3ff0_2600), 0);
        p.write32(0x3ff0_0490, 0);
        assert!(p.rsa.enabled);
        assert_eq!(p.read32(0x3ff0_2818), 1);
    }
}
