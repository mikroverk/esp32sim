//! ECC_MULT, ESP-IDF v5.5.5 components/soc/esp32c6/register/soc/ecc_mult_reg.h
//! lines 14-154: interrupt bit 0, CONF START/RESET/KEY_LENGTH bits 0/1/2,
//! WORK_MODE bits 5..7, verification bit 8, and K/PX/PY parameter RAM.
//! components/hal/esp32c6/include/hal/ecc_ll.h:73-90 selects modes 0/2/3.
//! Completion is synchronous;
//! this models arithmetic and register state, not accelerator latency.
use esp_periph::{
    crypto::{bn_mod, bn_modexp, bn_mul},
    Device, WriteEffect,
};

type Number = Vec<u32>;

fn add(a: &[u32], b: &[u32], p: &[u32]) -> Number {
    let mut out = Vec::new();
    let mut carry = 0u64;
    for i in 0..a.len().max(b.len()) {
        let v =
            a.get(i).copied().unwrap_or(0) as u64 + b.get(i).copied().unwrap_or(0) as u64 + carry;
        out.push(v as u32);
        carry = v >> 32;
    }
    if carry != 0 {
        out.push(carry as u32);
    }
    bn_mod(&out, p)
}
fn sub(a: &[u32], b: &[u32], p: &[u32]) -> Number {
    let a = bn_mod(a, p);
    let b = bn_mod(b, p);
    let mut out = vec![0; p.len() + 1];
    let mut carry = 0u64;
    for i in 0..p.len() {
        let v = a.get(i).copied().unwrap_or(0) as u64 + p[i] as u64 + carry;
        out[i] = v as u32;
        carry = v >> 32;
    }
    out[p.len()] = carry as u32;
    let mut borrow = 0i64;
    for (i, word) in out.iter_mut().enumerate() {
        let v = *word as i64 - b.get(i).copied().unwrap_or(0) as i64 - borrow;
        *word = v as u32;
        borrow = i64::from(v < 0);
    }
    bn_mod(&out, p)
}
fn mul(a: &[u32], b: &[u32], p: &[u32]) -> Number {
    bn_mod(&bn_mul(a, b), p)
}
fn zero(a: &[u32]) -> bool {
    a.iter().all(|v| *v == 0)
}

struct Curve {
    p: Number,
    b: Number,
}
impl Curve {
    fn new() -> Self {
        Self {
            p: vec![0xffffffff, 0xffffffff, 0xffffffff, 0x00000000, 0x00000000, 0x00000000, 0x00000001, 0xffffffff],
            b: vec![0x27d2604b, 0x3bce3c3e, 0xcc53b0f6, 0x651d06b0, 0x769886bc, 0xb3ebbd55, 0xaa3a93e7, 0x5ac635d8],
        }
    }
    fn valid(&self, x: &[u32], y: &[u32]) -> bool {
        let p = &self.p;
        if !(x.iter().rev().cmp(p.iter().rev()).is_lt() && y.iter().rev().cmp(p.iter().rev()).is_lt()) {
            return false;
        }
        let rhs = add(
            &sub(&mul(&mul(x, x, p), x, p), &mul(x, &[3], p), p),
            &self.b,
            p,
        );
        zero(&sub(&mul(y, y, p), &rhs, p))
    }
}

struct Point {
    x: Number,
    y: Number,
    z: Number,
}
impl Point {
    fn infinity() -> Self {
        Self { x: vec![], y: vec![], z: vec![] }
    }
    fn double(&self, p: &[u32]) -> Self {
        if zero(&self.z) || zero(&self.y) {
            return Self::infinity();
        }
        let xx = mul(&self.x, &self.x, p);
        let yy = mul(&self.y, &self.y, p);
        let yyyy = mul(&yy, &yy, p);
        let zz = mul(&self.z, &self.z, p);
        let xy = add(&self.x, &yy, p);
        let s = mul(&sub(&sub(&mul(&xy, &xy, p), &xx, p), &yyyy, p), &[2], p);
        let m = mul(&sub(&xx, &mul(&zz, &zz, p), p), &[3], p);
        let x = sub(&mul(&m, &m, p), &mul(&s, &[2], p), p);
        let y = sub(&mul(&m, &sub(&s, &x, p), p), &mul(&yyyy, &[8], p), p);
        let yz = add(&self.y, &self.z, p);
        let z = sub(&sub(&mul(&yz, &yz, p), &yy, p), &zz, p);
        Self { x, y, z }
    }
    fn add_affine(&self, x: &[u32], y: &[u32], p: &[u32]) -> Self {
        if zero(&self.z) {
            return Self { x: x.to_vec(), y: y.to_vec(), z: vec![1] };
        }
        let zz = mul(&self.z, &self.z, p);
        let h = sub(&mul(x, &zz, p), &self.x, p);
        let r = sub(&mul(&mul(y, &self.z, p), &zz, p), &self.y, p);
        if zero(&h) {
            return if zero(&r) {
                self.double(p)
            } else {
                Self::infinity()
            };
        }
        let hh = mul(&h, &h, p);
        let hhh = mul(&h, &hh, p);
        let v = mul(&self.x, &hh, p);
        let nx = sub(&sub(&mul(&r, &r, p), &hhh, p), &mul(&v, &[2], p), p);
        let ny = sub(&mul(&r, &sub(&v, &nx, p), p), &mul(&self.y, &hhh, p), p);
        Self { x: nx, y: ny, z: mul(&self.z, &h, p) }
    }
    fn affine(self, p: &[u32]) -> (Number, Number) {
        if zero(&self.z) {
            return (vec![], vec![]);
        }
        let inv = bn_modexp(&self.z, &sub(p, &[2], p), p);
        let inv2 = mul(&inv, &inv, p);
        (
            mul(&self.x, &inv2, p),
            mul(&mul(&self.y, &inv2, p), &inv, p),
        )
    }
}

/// C6 ECC_MULT: little-endian parameter RAM, P-256 point validation and scalar multiplication.
pub struct Ecc {
    mem: [u32; 24],
    conf: u32,
    raw: u32,
    ena: u32,
}
impl Default for Ecc {
    fn default() -> Self {
        Self { mem: [0; 24], conf: 1 << 31, raw: 0, ena: 0 }
    }
}
impl Ecc {
    fn calculate(&mut self) {
        let mode = (self.conf >> 5) & 7;
        if !matches!(mode, 0 | 2 | 3) || self.conf & 4 == 0 {
            return;
        }
        let curve = Curve::new();
        let n = curve.p.len();
        let x = &self.mem[8..8 + n];
        let y = &self.mem[16..16 + n];
        let valid = curve.valid(x, y);
        self.conf = (self.conf & !(1 | 256)) | if valid { 256 } else { 0 };
        if mode != 2 && (mode == 0 || valid) {
            let mut point = Point::infinity();
            for bit in (0..n * 32).rev() {
                point = point.double(&curve.p);
                if self.mem[bit / 32] >> (bit % 32) & 1 != 0 {
                    point = point.add_affine(x, y, &curve.p);
                }
            }
            let (x, y) = point.affine(&curve.p);
            self.mem[8..].fill(0);
            self.mem[8..8 + x.len()].copy_from_slice(&x);
            self.mem[16..16 + y.len()].copy_from_slice(&y);
        }
        self.raw = 1;
    }
}
impl Device for Ecc {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0xc => self.raw,
            0x10 => self.raw & self.ena,
            0x14 => self.ena,
            0x1c => self.conf,
            0x100..=0x15c => self.mem[((off - 0x100) / 4) as usize],
            _ => 0,
        }
    }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        match off {
            0xc | 0x18 => self.raw &= !value,
            0x14 => self.ena = value & 1,
            0x1c => {
                if value & 2 != 0 {
                    *self = Self::default();
                } else {
                    self.conf = (value & !256) | (self.conf & 256);
                    if value & 1 != 0 {
                        self.calculate();
                    }
                }
            }
            0x100..=0x15c => self.mem[((off - 0x100) / 4) as usize] = value,
            _ => {}
        }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 {
        (self.raw & self.ena) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const G256X: &[u32] = &[0xd898c296, 0xf4a13945, 0x2deb33a0, 0x77037d81, 0x63a440f2, 0xf8bce6e5, 0xe12c4247, 0x6b17d1f2];
    const G256Y: &[u32] = &[0x37bf51f5, 0xcbb64068, 0x6b315ece, 0x2bce3357, 0x7c0f9e16, 0x8ee7eb4a, 0xfe1a7f9b, 0x4fe342e2];
    fn write_number(ecc: &mut Ecc, offset: u32, value: &[u32]) {
        for (i, word) in value.iter().enumerate() {
            ecc.write(offset + i as u32 * 4, *word);
        }
    }
    fn read_number(ecc: &mut Ecc, offset: u32, words: usize) -> Number {
        (0..words)
            .map(|i| ecc.read(offset + i as u32 * 4))
            .collect()
    }
    #[test]
    fn scalar_results_match_independent_openssl_vectors() {
        type Vector = (&'static [u32], &'static [u32], &'static [u32], &'static [u32], &'static [u32]);
        let vectors: &[Vector] = &[
            (
                G256X,
                G256Y,
                &[0x02],
                &[0x47669978, 0xa60b48fc, 0x77f21b35, 0xc08969e2, 0x04b51ac3, 0x8a523803, 0x8d034f7e, 0x7cf27b18],
                &[0x227873d1, 0x9e04b79d, 0x3ce98229, 0xba7dade6, 0x9f7430db, 0x293d9ac6, 0xdb8ed040, 0x07775510],
            ),
            (
                G256X,
                G256Y,
                &[0x89abcdef, 0x1234567],
                &[0xda672482, 0x2257323f, 0xd8e93468, 0x00275bca, 0x92a2ac0b, 0x11d5d1aa, 0xb9f52c7f, 0x3988322a],
                &[0xaacd9918, 0x97732034, 0xc8bd90c7, 0x001e3a0e, 0x3d57dc02, 0x0014311c, 0xf116c19c, 0x855b7389],
            ),
        ];
        for &(x, y, key, expected_x, expected_y) in vectors {
            let mut ecc = Ecc::default();
            write_number(&mut ecc, 0x100, key);
            write_number(&mut ecc, 0x120, x);
            write_number(&mut ecc, 0x140, y);
            ecc.write(0x14, 1);
            ecc.write(0x1c, 4 | (3 << 5) | 1);
            assert_eq!(ecc.read(0xc), 1);
            assert_eq!(ecc.irq_sources(), 1);
            assert_eq!(ecc.read(0x1c) & 257, 256);
            let words = 8;
            assert_eq!(read_number(&mut ecc, 0x120, words), expected_x);
            assert_eq!(read_number(&mut ecc, 0x140, words), expected_y);
            ecc.write(0x18, 1);
            assert_eq!(ecc.irq_sources(), 0);
        }
    }
    #[test]
    fn verification_rejects_off_curve_and_noncanonical_points() {
        let mut ecc = Ecc::default();
        write_number(&mut ecc, 0x120, G256X);
        write_number(&mut ecc, 0x140, G256Y);
        ecc.write(0x1c, 0x45);
        assert_eq!(ecc.read(0x1c) & 256, 256);
        ecc.write(0x140, ecc.mem[16] ^ 1);
        ecc.write(0x1c, 0x45);
        assert_eq!(ecc.read(0x1c) & 256, 0);
        assert_eq!(ecc.read(0xc), 1);
        write_number(
            &mut ecc,
            0x120,
            &[0xffffffff, 0xffffffff, 0xffffffff, 0x00000000, 0x00000000, 0x00000000, 0x00000001, 0xffffffff],
        );
        ecc.write(0x1c, 0x45);
        assert_eq!(ecc.read(0x1c) & 256, 0);
    }
    #[test]
    fn noncanonical_point_congruent_to_valid_point_is_rejected() {
        let mut ecc = Ecc::default();
        // Independently calculated y = sqrt(b) mod p: (0,y) is on P-256.
        write_number(&mut ecc, 0x140, &[0x174f93f4, 0x28bf856a, 0x1dae8717, 0x541c2af3, 0x84a06bb6, 0x2433bd5d, 0x0e2f83d7, 0x66485c78]);
        ecc.write(0x1c, 0x45);
        assert_eq!(ecc.read(0x1c) & 256, 256);
        write_number(&mut ecc, 0x120, &[0xffffffff, 0xffffffff, 0xffffffff, 0x00000000, 0x00000000, 0x00000000, 0x00000001, 0xffffffff]);
        let before = ecc.mem;
        ecc.write(0x1c, 0x65);
        assert_eq!(ecc.read(0x1c) & 256, 0);
        assert_eq!(ecc.mem, before, "failed verification must not multiply");
        assert_eq!(ecc.read(0xc), 1);
    }

    #[test]
    fn verify_only_preserves_parameters_and_result_is_read_only() {
        let mut ecc = Ecc::default();
        ecc.write(0x1c, 256);
        assert_eq!(ecc.read(0x1c) & 256, 0);
        write_number(&mut ecc, 0x120, G256X);
        write_number(&mut ecc, 0x140, G256Y);
        let before = ecc.mem;
        ecc.write(0x1c, 0x45);
        assert_eq!(ecc.mem, before);
        ecc.write(0x1c, 0);
        assert_eq!(ecc.read(0x1c) & 256, 256);
    }

    #[test]
    fn zero_scalar_and_group_order_produce_infinity() {
        for key in [&[0][..],
            &[0xfc632551, 0xf3b9cac2, 0xa7179e84, 0xbce6faad, 0xffffffff, 0xffffffff, 0x00000000, 0xffffffff],
        ] {
            let mut ecc = Ecc::default();
            write_number(&mut ecc, 0x100, key);
            write_number(&mut ecc, 0x120, G256X);
            write_number(&mut ecc, 0x140, G256Y);
            ecc.write(0x1c, 5);
            assert!(zero(&ecc.mem[8..]));
            assert_eq!(ecc.read(0xc), 1);
        }
    }
    #[test]
    fn unsupported_modes_do_not_report_success_and_reset_clears_interrupts() {
        let mut ecc = Ecc::default();
        ecc.write(0x1c, 0x41);
        assert_eq!(ecc.read(0xc), 0);
        ecc.write(0x1c, 0x25);
        assert_eq!(ecc.read(0xc), 0);
        ecc.write(0x14, 1);
        ecc.raw = 1;
        ecc.write(0x1c, 2);
        assert_eq!(ecc.irq_sources(), 0);
        assert_eq!(ecc.read(0x1c), 1 << 31);
    }
}
