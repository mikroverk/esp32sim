//! Xtensa LX7 (ESP32-S3 configuration) instruction decoder.
//!
//! Instruction words are little-endian. For 24-bit instructions:
//!   b0 = t<<4 | op0,  b1 = r<<4 | s,  b2 = op2<<4 | op1
//! For 16-bit (density) instructions: b0 = t<<4 | op0, b1 = r<<4 | s.
//! Branch/call targets are resolved to absolute addresses at decode time.

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    // --- illegal / control
    Ill, IllN, Nop, NopN, Break, BreakN, Syscall, Simcall, Waiti, Rsil,
    Isync, Rsync, Esync, Dsync, Excw, Memw, Extw,
    // --- jumps / calls / returns
    J, Jx, Call0, Call4, Call8, Call12, Callx0, Callx4, Callx8, Callx12,
    Ret, RetN, Retw, RetwN, Entry, Movsp, Rotw,
    Rfe, Rfue, Rfde, Rfwo, Rfwu, Rfi, Rfme,
    L32e, S32e, S32nb,
    // --- branches
    Beqz, Bnez, Bltz, Bgez, BeqzN, BnezN,
    Beqi, Bnei, Blti, Bgei, Bltui, Bgeui,
    Bnone, Beq, Blt, Bltu, Ball, Bbc, Bbci, Bany, Bne, Bge, Bgeu, Bnall, Bbs, Bbsi,
    Bf, Bt,
    // --- loops
    Loop, Loopnez, Loopgtz,
    // --- loads / stores
    L8ui, L16ui, L16si, L32i, L32iN, L32r, L32ai, S8i, S16i, S32i, S32iN, S32ri, S32c1i,
    // --- cache (no-ops for us)
    Dpfr, Dpfw, Dpfro, Dpfwo, Dhwb, Dhwbi, Dhi, Dii, Ipf, Ihi, Iii, Ipfl, Ihu, Iiu, Dpfl, Dhu, Diu,
    // --- alu
    Movi, MoviN, Mov, MovN, Add, AddN, Addi, AddiN, Addmi, Sub, Addx2, Addx4, Addx8, Subx2, Subx4, Subx8,
    And, Or, Xor, Neg, Abs, Extui, Sext, Clamps, Min, Max, Minu, Maxu,
    Moveqz, Movnez, Movltz, Movgez, Movf, Movt,
    Slli, Srai, Srli, Sll, Srl, Sra, Src, Ssr, Ssl, Ssa8l, Ssa8b, Ssai, Nsa, Nsau,
    Mull, Muluh, Mulsh, Mul16u, Mul16s, Quou, Quos, Remu, Rems, Salt, Saltu,
    // --- special / user registers
    Rsr, Wsr, Xsr, Rur, Wur, Rer, Wer,
    // --- booleans
    Andb, Andbc, Orb, Orbc, Xorb, Any4, All4, Any8, All8,
    // --- TLB (region protection) — accepted, no effect
    Ritlb0, Iitlb, Pitlb, Witlb, Ritlb1, Rdtlb0, Idtlb, Pdtlb, Wdtlb, Rdtlb1,
    // --- FP
    Lsi, Ssi, Lsip, Ssip, Lsx, Ssx, Lsxp, Ssxp,
    AddS, SubS, MulS, MaddS, MsubS, MaddnS, DivnS,
    RoundS, TruncS, FloorS, CeilS, FloatS, UfloatS, UtruncS,
    MovS, AbsS, NegS, Rfr, Wfr,
    Div0S, Nexp01S, ConstS, Recip0S, Rsqrt0S, Sqrt0S, MksadjS, MkdadjS, AddexpS, AddexpmS,
    UnS, OeqS, UeqS, OltS, UltS, OleS, UleS,
    MoveqzS, MovnezS, MovltzS, MovgezS, MovfS, MovtS,
    // --- MAC16
    Mac16,   // details in `Insn::raw`; see exec
    // --- PIE / TIE (ESP32-S3 SIMD), decoded generically
    Pie,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Insn {
    pub op: Op,
    pub r: u8,
    pub s: u8,
    pub t: u8,
    /// immediate / special register number / absolute branch target / literal address
    pub imm: i32,
    /// second immediate (extui mask len, break arg, ...)
    pub imm2: i32,
    pub len: u8,
    /// raw instruction word (little-endian bytes packed), for MAC16/PIE
    pub raw: u32,
}

const B4CONST: [i32; 16] = [-1, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 32, 64, 128, 256];
const B4CONSTU: [i32; 16] = [32768, 65536, 2, 3, 4, 5, 6, 7, 8, 10, 12, 16, 32, 64, 128, 256];

#[inline]
fn sext(v: u32, bits: u32) -> i32 {
    ((v << (32 - bits)) as i32) >> (32 - bits)
}

impl Insn {
    #[allow(clippy::too_many_arguments, reason = "the constructor names every packed instruction field")]
    fn new(op: Op, r: u8, s: u8, t: u8, imm: i32, imm2: i32, len: u8, raw: u32) -> Self {
        Insn { op, r, s, t, imm, imm2, len, raw }
    }
}

/// Decode the instruction at `pc` from up to 4 bytes. Returns the decoded
/// instruction (an `Op::Ill` of the right length for reserved encodings).
pub const ICACHE_SIZE: usize = 1 << 14;
#[derive(Clone, Copy)]
pub struct CacheEntry { pub pc: u32, pub ver: u32, pub vidx: u32, pub ver2: u32, pub vidx2: u32, pub bytes: [u8; 4], pub insn: Insn, pub max_ar: u8 }
impl CacheEntry {
    pub const EMPTY: CacheEntry = CacheEntry { pc: 1, ver: 0, vidx: 0, ver2: 0, vidx2: 0, bytes: [0; 4], insn: Insn { op: Op::Ill, r: 0, s: 0, t: 0, imm: 0, imm2: 0, len: 0, raw: 0 }, max_ar: 0 };
}
#[inline(always)]
pub fn icache_index(pc: u32) -> usize { ((pc >> 1) ^ (pc >> 17)) as usize & (ICACHE_SIZE - 1) }

fn decode_pie(w: u32) -> Option<Insn> {
    let idx = crate::pie::decode(w)?;
    let max_ar = crate::pie::max_ar(w, idx);
    let (r, s, t) = crate::pie::pack(w, idx);
    Some(Insn::new(Op::Pie, r, s, t, idx as i32, max_ar as i32, crate::pie::OPS[idx].len, w))
}

pub fn decode(pc: u32, bytes: [u8; 4]) -> Insn {
    let b0 = bytes[0];
    let b1 = bytes[1];
    let b2 = bytes[2];
    let op0 = b0 & 0xf;
    let t = b0 >> 4;
    let s = b1 & 0xf;
    let r = b1 >> 4;
    let op1 = b2 & 0xf;
    let op2 = b2 >> 4;
    let w24 = b0 as u32 | (b1 as u32) << 8 | (b2 as u32) << 16;
    let w16 = b0 as u32 | (b1 as u32) << 8;
    let imm8 = b2 as u32;

    macro_rules! i3 { ($op:expr) => { Insn::new($op, r, s, t, 0, 0, 3, w24) }; }
    macro_rules! i3i { ($op:expr, $imm:expr) => { Insn::new($op, r, s, t, $imm as i32, 0, 3, w24) }; }
    macro_rules! i3ii { ($op:expr, $imm:expr, $imm2:expr) => { Insn::new($op, r, s, t, $imm as i32, $imm2 as i32, 3, w24) }; }
    macro_rules! i2 { ($op:expr) => { Insn::new($op, r, s, t, 0, 0, 2, w16) }; }
    macro_rules! i2i { ($op:expr, $imm:expr) => { Insn::new($op, r, s, t, $imm as i32, 0, 2, w16) }; }
    let ill3 = Insn::new(Op::Ill, r, s, t, 0, 0, 3, w24);

    let br8 = |off: u32| (pc.wrapping_add(4).wrapping_add(sext(off, 8) as u32)) as i32;

    match op0 {
        // ---------------------------------------------------------------- QRST
        0 => match op1 {
            0 => match op2 {
                0 => match r {
                    0 => {
                        let m = t >> 2;
                        let n = t & 3;
                        match (m, n) {
                            (0, 0) => i3!(Op::Ill),
                            (2, 0) => i3!(Op::Ret),
                            (2, 1) => i3!(Op::Retw),
                            (2, 2) => i3!(Op::Jx),
                            (3, 0) => i3!(Op::Callx0),
                            (3, 1) => i3!(Op::Callx4),
                            (3, 2) => i3!(Op::Callx8),
                            (3, 3) => i3!(Op::Callx12),
                            _ => ill3,
                        }
                    }
                    1 => i3!(Op::Movsp),
                    2 => match t {
                        0 => i3!(Op::Isync),
                        1 => i3!(Op::Rsync),
                        2 => i3!(Op::Esync),
                        3 => i3!(Op::Dsync),
                        8 => i3!(Op::Excw),
                        12 => i3!(Op::Memw),
                        13 => i3!(Op::Extw),
                        15 => i3!(Op::Nop),
                        _ => ill3,
                    },
                    3 => match t {
                        0 => match s {
                            0 => i3!(Op::Rfe),
                            1 => i3!(Op::Rfue),
                            2 => i3!(Op::Rfde),
                            4 => i3!(Op::Rfwo),
                            5 => i3!(Op::Rfwu),
                            _ => ill3,
                        },
                        1 => i3i!(Op::Rfi, s),
                        2 => i3!(Op::Rfme),
                        _ => ill3,
                    },
                    4 => i3ii!(Op::Break, s, t),
                    5 => match s { 0 => i3!(Op::Syscall), 1 => i3!(Op::Simcall), _ => ill3 },
                    6 => i3i!(Op::Rsil, s),
                    7 => i3i!(Op::Waiti, s),
                    8 => Insn::new(Op::Any4, r, s & !3, t, 0, 0, 3, w24),
                    9 => Insn::new(Op::All4, r, s & !3, t, 0, 0, 3, w24),
                    10 => Insn::new(Op::Any8, r, s & !7, t, 0, 0, 3, w24),
                    11 => Insn::new(Op::All8, r, s & !7, t, 0, 0, 3, w24),
                    _ => ill3,
                },
                1 => i3!(Op::And),
                2 => i3!(Op::Or),
                3 => i3!(Op::Xor),
                4 => match r {
                    0 => i3!(Op::Ssr),
                    1 => i3!(Op::Ssl),
                    2 => i3!(Op::Ssa8l),
                    3 => i3!(Op::Ssa8b),
                    4 => i3i!(Op::Ssai, s | ((t & 1) << 4)),
                    6 => i3!(Op::Rer),
                    7 => i3!(Op::Wer),
                    8 => i3i!(Op::Rotw, sext(t as u32, 4)),
                    14 => i3!(Op::Nsa),
                    15 => i3!(Op::Nsau),
                    _ => ill3,
                },
                5 => match r {
                    3 => i3!(Op::Ritlb0), 4 => i3!(Op::Iitlb), 5 => i3!(Op::Pitlb), 6 => i3!(Op::Witlb), 7 => i3!(Op::Ritlb1),
                    11 => i3!(Op::Rdtlb0), 12 => i3!(Op::Idtlb), 13 => i3!(Op::Pdtlb), 14 => i3!(Op::Wdtlb), 15 => i3!(Op::Rdtlb1),
                    _ => ill3,
                },
                6 => match s { 0 => i3!(Op::Neg), 1 => i3!(Op::Abs), _ => ill3 },
                8 => i3!(Op::Add),
                9 => i3!(Op::Addx2),
                10 => i3!(Op::Addx4),
                11 => i3!(Op::Addx8),
                12 => i3!(Op::Sub),
                13 => i3!(Op::Subx2),
                14 => i3!(Op::Subx4),
                15 => i3!(Op::Subx8),
                _ => ill3,
            },
            1 => match op2 {
                0 | 1 => i3i!(Op::Slli, 32 - (((op2 & 1) << 4) | t) as i32),
                2 | 3 => i3i!(Op::Srai, ((op2 & 1) << 4) | s),
                4 => i3i!(Op::Srli, s),
                6 => i3i!(Op::Xsr, ((r as u32) << 4) | s as u32),
                8 => i3!(Op::Src),
                9 => i3!(Op::Srl),
                10 => i3!(Op::Sll),
                11 => i3!(Op::Sra),
                12 => i3!(Op::Mul16u),
                13 => i3!(Op::Mul16s),
                _ => ill3,
            },
            2 => match op2 {
                0 => i3!(Op::Andb), 1 => i3!(Op::Andbc), 2 => i3!(Op::Orb), 3 => i3!(Op::Orbc), 4 => i3!(Op::Xorb),
                6 => i3!(Op::Saltu), 7 => i3!(Op::Salt),
                8 => i3!(Op::Mull), 10 => i3!(Op::Muluh), 11 => i3!(Op::Mulsh),
                12 => i3!(Op::Quou), 13 => i3!(Op::Quos), 14 => i3!(Op::Remu), 15 => i3!(Op::Rems),
                _ => ill3,
            },
            3 => match op2 {
                0 => i3i!(Op::Rsr, ((r as u32) << 4) | s as u32),
                1 => i3i!(Op::Wsr, ((r as u32) << 4) | s as u32),
                2 => i3i!(Op::Sext, t as i32 + 7),
                3 => i3i!(Op::Clamps, t as i32 + 7),
                4 => i3!(Op::Min), 5 => i3!(Op::Max), 6 => i3!(Op::Minu), 7 => i3!(Op::Maxu),
                8 => i3!(Op::Moveqz), 9 => i3!(Op::Movnez), 10 => i3!(Op::Movltz), 11 => i3!(Op::Movgez),
                12 => i3!(Op::Movf), 13 => i3!(Op::Movt),
                14 => i3i!(Op::Rur, ((s as u32) << 4) | t as u32),
                15 => i3i!(Op::Wur, ((r as u32) << 4) | s as u32),
                _ => ill3,
            },
            4 | 5 => i3ii!(Op::Extui, ((op1 & 1) << 4) | s, op2 + 1),
            6 | 7 => decode_pie(w24).unwrap_or(ill3), // S3's EE.LDF.64.XP / EE.STF.64.XP.
            8 => match op2 {
                0 => i3!(Op::Lsx), 1 => i3!(Op::Lsxp), 4 => i3!(Op::Ssx), 5 => i3!(Op::Ssxp),
                _ => ill3,
            },
            9 => match op2 {
                0 => i3i!(Op::L32e, (r as i32) * 4 - 64),
                4 => i3i!(Op::S32e, (r as i32) * 4 - 64),
                5 => i3i!(Op::S32nb, (r as i32) * 4),
                _ => ill3,
            },
            10 => match op2 {
                0 => i3!(Op::AddS), 1 => i3!(Op::SubS), 2 => i3!(Op::MulS),
                4 => i3!(Op::MaddS), 5 => i3!(Op::MsubS), 6 => i3!(Op::MaddnS), 7 => i3!(Op::DivnS),
                8 => i3i!(Op::RoundS, t), 9 => i3i!(Op::TruncS, t), 10 => i3i!(Op::FloorS, t), 11 => i3i!(Op::CeilS, t),
                12 => i3i!(Op::FloatS, t), 13 => i3i!(Op::UfloatS, t), 14 => i3i!(Op::UtruncS, t),
                15 => match t {
                    0 => i3!(Op::MovS), 1 => i3!(Op::AbsS), 3 => i3i!(Op::ConstS, s), 4 => i3!(Op::Rfr), 5 => i3!(Op::Wfr), 6 => i3!(Op::NegS),
                    7 => i3!(Op::Div0S), 8 => i3!(Op::Recip0S), 9 => i3!(Op::Sqrt0S), 10 => i3!(Op::Rsqrt0S), 11 => i3!(Op::Nexp01S),
                    12 => i3!(Op::MksadjS), 13 => i3!(Op::MkdadjS), 14 => i3!(Op::AddexpS), 15 => i3!(Op::AddexpmS),
                    _ => ill3,
                },
                _ => ill3,
            },
            11 => match op2 {
                1 => i3!(Op::UnS), 2 => i3!(Op::OeqS), 3 => i3!(Op::UeqS), 4 => i3!(Op::OltS), 5 => i3!(Op::UltS), 6 => i3!(Op::OleS), 7 => i3!(Op::UleS),
                8 => i3!(Op::MoveqzS), 9 => i3!(Op::MovnezS), 10 => i3!(Op::MovltzS), 11 => i3!(Op::MovgezS), 12 => i3!(Op::MovfS), 13 => i3!(Op::MovtS),
                _ => ill3,
            },
            _ => ill3,
        },
        // ---------------------------------------------------------------- L32R
        1 => {
            let off = (0xFFFF_0000u32 | ((b1 as u32) | ((b2 as u32) << 8))) << 2;
            let addr = ((pc.wrapping_add(3)) & !3).wrapping_add(off);
            Insn::new(Op::L32r, r, s, t, addr as i32, 0, 3, w24)
        }
        // ---------------------------------------------------------------- LSAI
        2 => match r {
            0 => i3i!(Op::L8ui, imm8),
            1 => i3i!(Op::L16ui, imm8 << 1),
            2 => i3i!(Op::L32i, imm8 << 2),
            4 => i3i!(Op::S8i, imm8),
            5 => i3i!(Op::S16i, imm8 << 1),
            6 => i3i!(Op::S32i, imm8 << 2),
            7 => match t {
                0 => i3i!(Op::Dpfr, imm8 << 2), 1 => i3i!(Op::Dpfw, imm8 << 2), 2 => i3i!(Op::Dpfro, imm8 << 2), 3 => i3i!(Op::Dpfwo, imm8 << 2),
                4 => i3i!(Op::Dhwb, imm8 << 2), 5 => i3i!(Op::Dhwbi, imm8 << 2), 6 => i3i!(Op::Dhi, imm8 << 2), 7 => i3i!(Op::Dii, imm8 << 2),
                8 => match op1 {   // DCE: op1 selects
                    0 => i3i!(Op::Dpfl, (imm8 >> 4) << 4), 2 => i3i!(Op::Dhu, (imm8 >> 4) << 4), 3 => i3i!(Op::Diu, (imm8 >> 4) << 4),
                    _ => ill3,
                },
                12 => i3i!(Op::Ipf, imm8 << 2),
                13 => match op1 { 0 => i3i!(Op::Ipfl, (imm8 >> 4) << 4), 2 => i3i!(Op::Ihu, (imm8 >> 4) << 4), 3 => i3i!(Op::Iiu, (imm8 >> 4) << 4), _ => ill3 },
                14 => i3i!(Op::Ihi, imm8 << 2),
                15 => i3i!(Op::Iii, imm8 << 2),
                _ => ill3,
            },
            9 => i3i!(Op::L16si, imm8 << 1),
            10 => i3i!(Op::Movi, sext(((s as u32) << 8) | imm8, 12)),
            11 => i3i!(Op::L32ai, imm8 << 2),
            12 => i3i!(Op::Addi, sext(imm8, 8)),
            13 => i3i!(Op::Addmi, sext(imm8, 8) << 8),
            14 => i3i!(Op::S32c1i, imm8 << 2),
            15 => i3i!(Op::S32ri, imm8 << 2),
            _ => ill3,
        },
        // ---------------------------------------------------------------- LSCI
        3 => match r {
            0 => i3i!(Op::Lsi, imm8 << 2),
            4 => i3i!(Op::Ssi, imm8 << 2),
            8 => i3i!(Op::Lsip, imm8 << 2),
            12 => i3i!(Op::Ssip, imm8 << 2),
            _ => ill3,
        },
        // ---------------------------------------------------------------- MAC16 / PIE (op0 = 4)
        4 => {
            // PIE shares op0 = 4 with MAC16; recognize configured PIE encodings first.
            // ESP-IDF's esp32s3 core-isa.h sets XCHAL_HAVE_MAC16 = 1.
            decode_pie(w24).unwrap_or_else(|| Insn::new(Op::Mac16, r, s, t, 0, 0, 3, w24))
        }
        0xe | 0xf => {
            let w32 = u32::from_le_bytes(bytes);
            decode_pie(w32).unwrap_or(ill3)
        }
        // ---------------------------------------------------------------- CALLN
        5 => {
            let n = (b0 >> 4) & 3;
            let off = sext(w24 >> 6, 18);
            let target = ((pc & !3) as i32).wrapping_add(off << 2).wrapping_add(4);
            let op = match n { 0 => Op::Call0, 1 => Op::Call4, 2 => Op::Call8, _ => Op::Call12 };
            Insn::new(op, r, s, t, target, 0, 3, w24)
        }
        // ---------------------------------------------------------------- SI
        6 => {
            let n = (b0 >> 4) & 3;
            let m = (b0 >> 6) & 3;
            match n {
                0 => {
                    let off = sext(w24 >> 6, 18);
                    Insn::new(Op::J, r, s, t, (pc as i32).wrapping_add(4).wrapping_add(off), 0, 3, w24)
                }
                1 => {
                    let target = (pc as i32).wrapping_add(4).wrapping_add(sext(w24 >> 12, 12));
                    let op = match m { 0 => Op::Beqz, 1 => Op::Bnez, 2 => Op::Bltz, _ => Op::Bgez };
                    Insn::new(op, r, s, t, target, 0, 3, w24)
                }
                2 => {
                    let op = match m { 0 => Op::Beqi, 1 => Op::Bnei, 2 => Op::Blti, _ => Op::Bgei };
                    Insn::new(op, r, s, t, br8(imm8), B4CONST[r as usize], 3, w24)
                }
                _ => match m {
                    0 => Insn::new(Op::Entry, r, s, t, ((w24 >> 12) << 3) as i32, 0, 3, w24),
                    1 => {
                        let tgt_u = (pc as i32).wrapping_add(4).wrapping_add(imm8 as i32);
                        match r {
                            0 => Insn::new(Op::Bf, r, s, t, br8(imm8), 0, 3, w24),
                            1 => Insn::new(Op::Bt, r, s, t, br8(imm8), 0, 3, w24),
                            8 => Insn::new(Op::Loop, r, s, t, tgt_u, 0, 3, w24),
                            9 => Insn::new(Op::Loopnez, r, s, t, tgt_u, 0, 3, w24),
                            10 => Insn::new(Op::Loopgtz, r, s, t, tgt_u, 0, 3, w24),
                            _ => ill3,
                        }
                    }
                    2 => Insn::new(Op::Bltui, r, s, t, br8(imm8), B4CONSTU[r as usize], 3, w24),
                    _ => Insn::new(Op::Bgeui, r, s, t, br8(imm8), B4CONSTU[r as usize], 3, w24),
                },
            }
        }
        // ---------------------------------------------------------------- B
        7 => {
            let target = br8(imm8);
            let (op, imm2) = match r {
                0 => (Op::Bnone, 0), 1 => (Op::Beq, 0), 2 => (Op::Blt, 0), 3 => (Op::Bltu, 0), 4 => (Op::Ball, 0), 5 => (Op::Bbc, 0),
                6 | 7 => (Op::Bbci, (((r & 1) << 4) | t) as i32),
                8 => (Op::Bany, 0), 9 => (Op::Bne, 0), 10 => (Op::Bge, 0), 11 => (Op::Bgeu, 0), 12 => (Op::Bnall, 0), 13 => (Op::Bbs, 0),
                _ => (Op::Bbsi, (((r & 1) << 4) | t) as i32),
            };
            Insn::new(op, r, s, t, target, imm2, 3, w24)
        }
        // ---------------------------------------------------------------- 16-bit
        8 => i2i!(Op::L32iN, (r as i32) << 2),
        9 => i2i!(Op::S32iN, (r as i32) << 2),
        10 => i2!(Op::AddN),
        11 => i2i!(Op::AddiN, if t == 0 { -1 } else { t as i32 }),
        12 => {
            if t & 8 == 0 {
                let mut imm = (((t & 7) as i32) << 4) | r as i32;
                if imm & 0x60 == 0x60 { imm |= !0x7f; }
                i2i!(Op::MoviN, imm)
            } else {
                let imm6 = (((t & 3) as i32) << 4) | r as i32;
                let target = (pc as i32).wrapping_add(4).wrapping_add(imm6);
                if t & 4 == 0 { i2i!(Op::BeqzN, target) } else { i2i!(Op::BnezN, target) }
            }
        }
        13 => match r {
            0 => i2!(Op::MovN),
            15 => match t {
                0 => i2!(Op::RetN), 1 => i2!(Op::RetwN), 2 => i2i!(Op::BreakN, s), 3 => i2!(Op::NopN), 6 => i2!(Op::IllN),
                _ => Insn::new(Op::IllN, r, s, t, 0, 0, 2, w16),
            },
            _ => Insn::new(Op::IllN, r, s, t, 0, 0, 2, w16),
        },
        // PIE encodings were handled before the base op0 match.
        _ => ill3,
    }
}

/// Special register names (RSR/WSR/XSR), objdump spelling.
pub fn sr_name(n: u32) -> Option<&'static str> {
    Some(match n {
        0 => "lbeg", 1 => "lend", 2 => "lcount", 3 => "sar", 4 => "br", 5 => "litbase", 12 => "scompare1",
        16 => "acclo", 17 => "acchi", 32 => "m0", 33 => "m1", 34 => "m2", 35 => "m3",
        72 => "windowbase", 73 => "windowstart", 83 => "ptevaddr", 89 => "mmid", 90 => "rasid", 91 => "itlbcfg", 92 => "dtlbcfg",
        96 => "ibreakenable", 97 => "memctl", 98 => "cacheattr", 99 => "atomctl", 104 => "ddr", 106 => "mepc", 107 => "meps", 108 => "mesave", 109 => "mesr", 110 => "mecr", 111 => "mevaddr",
        128 => "ibreaka0", 129 => "ibreaka1", 144 => "dbreaka0", 145 => "dbreaka1", 160 => "dbreakc0", 161 => "dbreakc1",
        176 => "configid0", 177 => "epc1", 178 => "epc2", 179 => "epc3", 180 => "epc4", 181 => "epc5", 182 => "epc6", 183 => "epc7",
        192 => "depc", 194 => "eps2", 195 => "eps3", 196 => "eps4", 197 => "eps5", 198 => "eps6", 199 => "eps7",
        208 => "configid1", 209 => "excsave1", 210 => "excsave2", 211 => "excsave3", 212 => "excsave4", 213 => "excsave5", 214 => "excsave6", 215 => "excsave7",
        224 => "cpenable", 226 => "interrupt", 227 => "intclear", 228 => "intenable", 230 => "ps", 231 => "vecbase", 232 => "exccause", 233 => "debugcause",
        234 => "ccount", 235 => "prid", 236 => "icount", 237 => "icountlevel", 238 => "excvaddr",
        240 => "ccompare0", 241 => "ccompare1", 242 => "ccompare2", 244 => "misc0", 245 => "misc1", 246 => "misc2", 247 => "misc3",
        _ => return None,
    })
}

/// User register names (RUR/WUR), ESP32-S3 (PIE registers + FPU + THREADPTR).
pub fn ur_name(n: u32) -> Option<&'static str> {
    Some(match n {
        0 => "accx_0", 1 => "accx_1", 2 => "qacc_h_0", 3 => "qacc_h_1", 4 => "qacc_h_2", 5 => "qacc_h_3", 6 => "qacc_h_4",
        7 => "qacc_l_0", 8 => "qacc_l_1", 9 => "qacc_l_2", 10 => "qacc_l_3", 11 => "qacc_l_4", 12 => "gpio_out",
        13 => "sar_byte", 14 => "fft_bit_width", 15 => "ua_state_0", 16 => "ua_state_1", 17 => "ua_state_2", 18 => "ua_state_3",
        231 => "threadptr", 232 => "fcr", 233 => "fsr", 234 => "f64r_lo", 235 => "f64r_hi", 236 => "f64s",
        _ => return None,
    })
}
