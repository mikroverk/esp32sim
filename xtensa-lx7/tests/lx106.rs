//! Hermetic LX106 corpus, generated with the pinned toolchains in corpus/build-lx106.py.
use emu_core::{Bus, Core, StepKind};
use xtensa_lx7::{decode, lx106::supports, state::{exc, ps, sr}, FlatRam, Lx106, Op, Trap};
#[allow(dead_code)]
mod corpus { include!("corpus/lx106-generated.rs"); }
const BASE: u32 = 0x4010_0000;

fn machine(pc: u32) -> (Lx106, FlatRam) {
    let mut c = Lx106::new();
    c.set_pc(pc); c.write_sr(sr::PS, 0).unwrap();
    let mut b = FlatRam::new(BASE, 0x10000);
    b.mem[..corpus::PROGRAM.len()].copy_from_slice(corpus::PROGRAM);
    (c, b)
}

#[test]
fn call0_callx0_density_stack_arguments_and_returns() {
    let (mut c, mut b) = machine(corpus::START);
    c.set_ar(1, BASE + 0x8000); c.set_ar(0, BASE + 0x9000);
    let mut visited = Vec::new();
    for _ in 0..40 {
        if c.pc() == corpus::DONE { break; }
        visited.push(c.pc());
        assert_eq!(c.step(&mut b).result(), Ok(()));
    }
    assert!(visited.contains(&corpus::LEAF));
    assert!(visited.contains(&corpus::INDIRECT));
    assert_eq!(c.pc(), corpus::DONE);
    assert_eq!(c.get_ar(2), 32); // leaf: 7 + 9 + stack arg 11; indirect: 5
    assert_eq!(c.get_ar(0), BASE + 0x9000);
    assert_eq!(c.get_ar(1), BASE + 0x8000);
    assert_eq!(c.state().windowbase, 0);
    assert_eq!(c.state().insn_count, 20);
    // Synthetic call0 returns preserve all 32 address bits (not windowed RA encoding).
    c.set_ar(0, 0x12345678); c.return_from_stub(&mut b, 42);
    assert_eq!((c.pc(), c.get_ar(2)), (0x12345678, 42));
    b.write32(c.get_ar(1), 0xdeadbeef).unwrap();
    assert_eq!(c.arg(&mut b, 6), 0xdeadbeef);
}

#[test]
fn signed_unsigned_multiply() {
    for (x, y) in [(0xffff_fffe, 3), (0x1234_ffff, 0xabcd_fffe), (0x8000_0000, 2)] {
        let (mut c, mut b) = machine(corpus::ARITHMETIC);
        c.set_ar(2, x); c.set_ar(3, y);
        assert_eq!(c.run(&mut b, 3), (3, None));
        assert_eq!(c.get_ar(4), x.wrapping_mul(y));
        assert_eq!(c.get_ar(5), (x as i16 as i32).wrapping_mul(y as i16 as i32) as u32);
        assert_eq!(c.get_ar(6), (x & 0xffff) * (y & 0xffff));
    }
}

#[test]
fn pc_relative_and_absolute_literals() {
    let (mut c, mut b) = machine(corpus::LITERAL_INSN);
    assert_eq!(c.step(&mut b).result(), Ok(()));
    assert_eq!(c.get_ar(2), 0x12345678);
    // Encoded displacement is -4. Use a different literal region, not the PC's pool.
    c.set_pc(corpus::LITERAL_INSN);
    c.write_sr(5, BASE + 0x2001).unwrap();
    b.write32(BASE + 0x1ffc, 0xaabbccdd).unwrap();
    assert_eq!(c.step(&mut b).result(), Ok(()));
    assert_eq!(c.get_ar(2), 0xaabbccdd);
    assert_eq!(c.read_sr(5), Some(BASE + 0x2001));
}

/// One instance of every LX106 opcode the profile executes: (QEMU line, mnemonic, op, bytes).
/// Membership: QEMU v10.1.0 target/xtensa/core-lx106/xtensa-modules.c.inc, the opcodes[]
/// table at the given line (docs/evidence/lx106-core/isa-membership.json pins the file).
/// Bytes: GNU as 2.43.1 (esp-14.2.0_20260121) with xtensa_esp8266.so, linked at 0x40100100,
/// except rer/wer (that config gives them no operand fields) and l32r (displacement -4),
/// which are hand-encoded from the Xtensa ISA formats. The `mov` alias decodes as OR.
const LX106_OPS: &[(u32, &str, Op, &[u8])] = &[
    (6037, "nop", Op::Nop, &[0xf0, 0x20, 0x00]), (5863, "nop.n", Op::NopN, &[0x3d, 0xf0]),
    (6289, "break 1, 2", Op::Break, &[0x20, 0x41, 0x00]), (6292, "break.n 1", Op::BreakN, &[0x2d, 0xf1]),
    (5833, "syscall", Op::Syscall, &[0x00, 0x50, 0x00]), (6268, "waiti 0", Op::Waiti, &[0x00, 0x70, 0x00]),
    (6106, "rsil a2, 1", Op::Rsil, &[0x20, 0x61, 0x00]), (6094, "isync", Op::Isync, &[0x00, 0x20, 0x00]),
    (6097, "rsync", Op::Rsync, &[0x10, 0x20, 0x00]), (6100, "esync", Op::Esync, &[0x20, 0x20, 0x00]),
    (6103, "dsync", Op::Dsync, &[0x30, 0x20, 0x00]), (5824, "excw", Op::Excw, &[0x80, 0x20, 0x00]),
    (6088, "memw", Op::Memw, &[0xc0, 0x20, 0x00]), (6091, "extw", Op::Extw, &[0xd0, 0x20, 0x00]),
    (5995, "j .+3", Op::J, &[0xc6, 0xff, 0xff]), (5998, "jx a3", Op::Jx, &[0xa0, 0x03, 0x00]),
    (5983, "call0 .+8", Op::Call0, &[0x45, 0x00, 0x00]), (5986, "callx0 a3", Op::Callx0, &[0xc0, 0x03, 0x00]),
    (6040, "ret", Op::Ret, &[0x80, 0x00, 0x00]), (5866, "ret.n", Op::RetN, &[0x0d, 0xf0]),
    (5827, "rfe", Op::Rfe, &[0x00, 0x30, 0x00]), (5830, "rfde", Op::Rfde, &[0x00, 0x32, 0x00]),
    (6265, "rfi 2", Op::Rfi, &[0x10, 0x32, 0x00]), (6265, "rfi 3", Op::Rfi, &[0x10, 0x33, 0x00]),
    (5971, "beqz a2, .+3", Op::Beqz, &[0x16, 0xf2, 0xff]), (5974, "bnez a2, .+3", Op::Bnez, &[0x56, 0xf2, 0xff]),
    (5980, "bltz a2, .+3", Op::Bltz, &[0x96, 0xf2, 0xff]), (5977, "bgez a2, .+3", Op::Bgez, &[0xd6, 0xf2, 0xff]),
    (5845, "beqz.n a2, .+4", Op::BeqzN, &[0x8c, 0x02]), (5848, "bnez.n a2, .+4", Op::BnezN, &[0xcc, 0x02]),
    (5911, "beqi a2, 1, .+3", Op::Beqi, &[0x26, 0x12, 0xff]), (5914, "bnei a2, 1, .+3", Op::Bnei, &[0x66, 0x12, 0xff]),
    (5920, "blti a2, 1, .+3", Op::Blti, &[0xa6, 0x12, 0xff]), (5917, "bgei a2, 1, .+3", Op::Bgei, &[0xe6, 0x12, 0xff]),
    (5932, "bltui a2, 2, .+3", Op::Bltui, &[0xb6, 0x22, 0xff]), (5929, "bgeui a2, 2, .+3", Op::Bgeui, &[0xf6, 0x22, 0xff]),
    (5956, "bnone a2, a3, .+3", Op::Bnone, &[0x37, 0x02, 0xff]), (5935, "beq a2, a3, .+3", Op::Beq, &[0x37, 0x12, 0xff]),
    (5944, "blt a2, a3, .+3", Op::Blt, &[0x37, 0x22, 0xff]), (5950, "bltu a2, a3, .+3", Op::Bltu, &[0x37, 0x32, 0xff]),
    (5959, "ball a2, a3, .+3", Op::Ball, &[0x37, 0x42, 0xff]), (5965, "bbc a2, a3, .+3", Op::Bbc, &[0x37, 0x52, 0xff]),
    (5923, "bbci a2, 5, .+3", Op::Bbci, &[0x57, 0x62, 0xff]), (5953, "bany a2, a3, .+3", Op::Bany, &[0x37, 0x82, 0xff]),
    (5938, "bne a2, a3, .+3", Op::Bne, &[0x37, 0x92, 0xff]), (5941, "bge a2, a3, .+3", Op::Bge, &[0x37, 0xa2, 0xff]),
    (5947, "bgeu a2, a3, .+3", Op::Bgeu, &[0x37, 0xb2, 0xff]), (5962, "bnall a2, a3, .+3", Op::Bnall, &[0x37, 0xc2, 0xff]),
    (5968, "bbs a2, a3, .+3", Op::Bbs, &[0x37, 0xd2, 0xff]), (5926, "bbsi a2, 5, .+3", Op::Bbsi, &[0x57, 0xe2, 0xff]),
    (6013, "l8ui a2, a3, 1", Op::L8ui, &[0x22, 0x03, 0x01]), (6001, "l16ui a2, a3, 2", Op::L16ui, &[0x22, 0x13, 0x01]),
    (6004, "l16si a2, a3, 2", Op::L16si, &[0x22, 0x93, 0x01]), (6007, "l32i a2, a3, 4", Op::L32i, &[0x22, 0x23, 0x01]),
    (5854, "l32i.n a2, a3, 4", Op::L32iN, &[0x28, 0x13]), (6010, "l32r a2, .-4", Op::L32r, &[0x21, 0xff, 0xff]),
    (6049, "s8i a2, a3, 1", Op::S8i, &[0x22, 0x43, 0x01]), (6043, "s16i a2, a3, 2", Op::S16i, &[0x22, 0x53, 0x01]),
    (6046, "s32i a2, a3, 4", Op::S32i, &[0x22, 0x63, 0x01]), (5869, "s32i.n a2, a3, 4", Op::S32iN, &[0x29, 0x13]),
    (6016, "movi a2, 100", Op::Movi, &[0x22, 0xa0, 0x64]), (5860, "movi.n a2, 10", Op::MoviN, &[0x0c, 0xa2]),
    (5857, "mov.n a2, a3", Op::MovN, &[0x2d, 0x03]), (5878, "add a2, a3, a4", Op::Add, &[0x40, 0x23, 0x80]),
    (5839, "add.n a2, a3, a4", Op::AddN, &[0x4a, 0x23]), (5872, "addi a2, a3, 5", Op::Addi, &[0x22, 0xc3, 0x05]),
    (5842, "addi.n a2, a3, 5", Op::AddiN, &[0x5b, 0x23]), (5875, "addmi a2, a3, 0x100", Op::Addmi, &[0x22, 0xd3, 0x01]),
    (5881, "sub a2, a3, a4", Op::Sub, &[0x40, 0x23, 0xc0]), (5884, "addx2 a2, a3, a4", Op::Addx2, &[0x40, 0x23, 0x90]),
    (5887, "addx4 a2, a3, a4", Op::Addx4, &[0x40, 0x23, 0xa0]), (5890, "addx8 a2, a3, a4", Op::Addx8, &[0x40, 0x23, 0xb0]),
    (5893, "subx2 a2, a3, a4", Op::Subx2, &[0x40, 0x23, 0xd0]), (5896, "subx4 a2, a3, a4", Op::Subx4, &[0x40, 0x23, 0xe0]),
    (5899, "subx8 a2, a3, a4", Op::Subx8, &[0x40, 0x23, 0xf0]), (5902, "and a2, a3, a4", Op::And, &[0x40, 0x23, 0x10]),
    (5905, "or a2, a3, a4", Op::Or, &[0x40, 0x23, 0x20]), (5908, "xor a2, a3, a4", Op::Xor, &[0x40, 0x23, 0x30]),
    (6031, "neg a2, a3", Op::Neg, &[0x30, 0x20, 0x60]), (6034, "abs a2, a3", Op::Abs, &[0x30, 0x21, 0x60]),
    (5989, "extui a2, a3, 4, 8", Op::Extui, &[0x30, 0x24, 0x74]), (6019, "moveqz a2, a3, a4", Op::Moveqz, &[0x40, 0x23, 0x83]),
    (6022, "movnez a2, a3, a4", Op::Movnez, &[0x40, 0x23, 0x93]), (6025, "movltz a2, a3, a4", Op::Movltz, &[0x40, 0x23, 0xa3]),
    (6028, "movgez a2, a3, a4", Op::Movgez, &[0x40, 0x23, 0xb3]), (6079, "slli a2, a3, 5", Op::Slli, &[0xb0, 0x23, 0x11]),
    (6082, "srai a2, a3, 5", Op::Srai, &[0x30, 0x25, 0x21]), (6085, "srli a2, a3, 5", Op::Srli, &[0x30, 0x25, 0x41]),
    (6067, "sll a2, a3", Op::Sll, &[0x00, 0x23, 0xa1]), (6073, "srl a2, a3", Op::Srl, &[0x30, 0x20, 0x91]),
    (6076, "sra a2, a3", Op::Sra, &[0x30, 0x20, 0xb1]), (6070, "src a2, a3, a4", Op::Src, &[0x40, 0x23, 0x81]),
    (6052, "ssr a3", Op::Ssr, &[0x00, 0x03, 0x40]), (6055, "ssl a3", Op::Ssl, &[0x00, 0x13, 0x40]),
    (6058, "ssa8l a3", Op::Ssa8l, &[0x00, 0x23, 0x40]), (6061, "ssa8b a3", Op::Ssa8b, &[0x00, 0x33, 0x40]),
    (6064, "ssai 5", Op::Ssai, &[0x00, 0x45, 0x40]), (6424, "nsa a2, a3", Op::Nsa, &[0x20, 0xe3, 0x40]),
    (6427, "nsau a2, a3", Op::Nsau, &[0x20, 0xf3, 0x40]), (6262, "mull a2, a3, a4", Op::Mull, &[0x40, 0x23, 0x82]),
    (6256, "mul16u a2, a3, a4", Op::Mul16u, &[0x40, 0x23, 0xc1]), (6259, "mul16s a2, a3, a4", Op::Mul16s, &[0x40, 0x23, 0xd1]),
    (6430, "rer a2, a3", Op::Rer, &[0x20, 0x63, 0x40]), (6433, "wer a2, a3", Op::Wer, &[0x20, 0x73, 0x40]),
    (6415, "ritlb0 a2, a3", Op::Ritlb0, &[0x20, 0x33, 0x50]), (6409, "iitlb a3", Op::Iitlb, &[0x00, 0x43, 0x50]),
    (6412, "pitlb a2, a3", Op::Pitlb, &[0x20, 0x53, 0x50]), (6421, "witlb a2, a3", Op::Witlb, &[0x20, 0x63, 0x50]),
    (6418, "ritlb1 a2, a3", Op::Ritlb1, &[0x20, 0x73, 0x50]), (6400, "rdtlb0 a2, a3", Op::Rdtlb0, &[0x20, 0xb3, 0x50]),
    (6394, "idtlb a3", Op::Idtlb, &[0x00, 0xc3, 0x50]), (6397, "pdtlb a2, a3", Op::Pdtlb, &[0x20, 0xd3, 0x50]),
    (6406, "wdtlb a2, a3", Op::Wdtlb, &[0x20, 0xe3, 0x50]), (6403, "rdtlb1 a2, a3", Op::Rdtlb1, &[0x20, 0xf3, 0x50]),
    (6109, "rsr.sar a2", Op::Rsr, &[0x20, 0x03, 0x03]), (6112, "wsr.sar a2", Op::Wsr, &[0x20, 0x03, 0x13]),
    (6115, "xsr.sar a2", Op::Xsr, &[0x20, 0x03, 0x61]),
];

/// Instructions the shared decoder knows but the LX106 opcode table lacks (cache ops, SALT,
/// MAC16, PIE), plus SIMCALL: in the table, but QEMU makes it illegal on hardware before
/// RE.3 (translate.c:2240-2253; LX106 XCHAL_HW_VERSION 230001). Bytes: mula.dd.ll from
/// GNU as 2.43.1 with xtensa_esp32.so; salt, saltu and ee.vadds.s8 with xtensa_esp32s3.so;
/// dhi, ihi and dpfr hand-encoded from the ISA CACHE format (no config here has caches).
/// xtensa_esp8266.so rejects all of them except simcall.
const EXTRA_FORBIDDEN: &[(&str, Op, &[u8])] = &[
    ("mula.dd.ll m0, m2", Op::Mac16, &[0x04, 0x00, 0x28]), ("dhi a2, 0", Op::Dhi, &[0x62, 0x72, 0x00]),
    ("ihi a2, 0", Op::Ihi, &[0xe2, 0x72, 0x00]), ("dpfr a2, 0", Op::Dpfr, &[0x02, 0x72, 0x00]),
    ("salt a2, a3, a4", Op::Salt, &[0x40, 0x23, 0x72]), ("saltu a2, a3, a4", Op::Saltu, &[0x40, 0x23, 0x62]),
    ("simcall", Op::Simcall, &[0x00, 0x51, 0x00]), ("ee.vadds.s8 q0, q1, q2", Op::Pie, &[0x84, 0x11, 0x8e]),
];

fn word(raw: &[u8]) -> [u8; 4] { let mut b = [0; 4]; b[..raw.len()].copy_from_slice(raw); b }

#[test]
fn every_lx106_opcode_is_supported_and_executes() {
    // Deleting an op from supports() makes its instance trap IllegalInstruction here.
    let at = BASE + 0x100;
    for &(_, name, op, raw) in LX106_OPS {
        let i = decode(at, word(raw));
        assert_eq!((i.op, i.len as usize), (op, raw.len()), "{name}");
        assert!(supports(&i), "{name}");
        let (mut c, mut b) = machine(at);
        b.mem[0x100..0x100 + raw.len()].copy_from_slice(raw);
        for n in 0..16 { c.set_ar(n, BASE + 0x200); }
        let expected = match op {
            Op::Syscall => Err(Trap::Exception(exc::SYSCALL)),
            Op::Break | Op::BreakN => Err(Trap::Exception(0x108)),
            Op::Rer | Op::Wer => Err(Trap::Unimplemented(at, i.raw)),
            _ => Ok(()),
        };
        assert_eq!(c.step(&mut b).result(), expected, "{name}");
    }
}

#[test]
fn supported_encodings_are_only_lx106_opcodes() {
    // Every 24-bit word (narrow forms included): supports() accepts no op outside the table,
    // so a new decoder op cannot silently become an LX106 instruction.
    let (mut table, mut seen) = ([false; 1024], [false; 1024]);
    for e in LX106_OPS { table[e.2 as usize] = true; }
    for w in 0..1u32 << 24 {
        let i = decode(BASE, [w as u8, (w >> 8) as u8, (w >> 16) as u8, 0]);
        if supports(&i) {
            assert!(table[i.op as usize], "{:?} accepted ({w:06x})", i.op);
            seen[i.op as usize] = true;
        }
    }
    assert_eq!(seen, table);
}

#[test]
fn forbidden_options_raise_illegal_before_any_effect() {
    for &(name, op, raw) in EXTRA_FORBIDDEN {
        assert_eq!(decode(BASE, word(raw)).op, op, "{name}");
    }
    let extra = EXTRA_FORBIDDEN.iter().map(|&(name, _, raw)| (name, raw));
    for (name, raw) in corpus::FORBIDDEN.iter().copied().chain(extra) {
        for mask in [0, ps::UM, ps::EXCM] {
            let (mut c, mut b) = machine(BASE);
            b.mem[..4].fill(0); b.mem[..raw.len()].copy_from_slice(raw);
            c.write_sr(sr::PS, mask).unwrap();
            c.set_ar(1, BASE + 0x8000); c.set_ar(2, 0x1234); c.set_ar(3, BASE + 0x100);
            let mut bytes = [0; 4]; bytes[..raw.len()].copy_from_slice(raw);
            assert!(!supports(&decode(BASE, bytes)), "{name}");
            assert_eq!(c.step(&mut b).result(), Err(Trap::Exception(exc::ILLEGAL)), "{name}");
            assert_eq!(c.state().exccause, exc::ILLEGAL, "{name}");
            assert_eq!(c.get_ar(1), BASE + 0x8000, "{name}");
            assert_eq!(c.get_ar(2), 0x1234, "{name}");
            assert_eq!(b.read32(BASE + 0x100).unwrap(), 0, "{name}");
            let offset = if mask & ps::EXCM != 0 { 0x70 } else if mask & ps::UM != 0 { 0x50 } else { 0x30 };
            assert_eq!(c.pc(), 0x4000_0000 + offset, "{name}");
            assert_eq!(if mask & ps::EXCM != 0 { c.state().depc } else { c.state().epc[1] }, BASE);
        }
    }
}

#[test]
fn register_options_and_reset_are_not_s3() {
    let mut c = Lx106::new();
    assert_eq!(c.pc(), 0x40000080);
    assert_eq!(c.state().vecbase, 0x40000000);
    assert_eq!(c.state().configid, [0xc28cdafa, 0x1082b6f6]);
    for n in [0, 1, 2, 4, 12, 16, 17, 32, 72, 73, 129, 145, 161, 180, 196, 212, 224, 241, 242, 244] {
        assert_eq!(c.read_sr(n), None, "SR {n}");
        assert_eq!(c.write_sr(n, 0xffffffff), None, "SR {n}");
    }
    c.write_sr(sr::PS, u32::MAX).unwrap();
    assert_eq!(c.state().ps, 0x3f);
    for n in [194, 195] {
        c.write_sr(n, u32::MAX).unwrap();
        assert_eq!(c.read_sr(n), Some(0x3f));
    }
    c.write_sr(sr::INTENABLE, u32::MAX).unwrap();
    assert_eq!(c.state().intenable, 0x7fff);
    c.write_sr(sr::CCOUNT, 123).unwrap(); c.set_ar(15, 42);
    c.reset();
    assert_eq!((c.state().ccount, c.get_ar(15), c.state().intenable), (0, 0, 0));
}

#[test]
fn compare_wrap_equal_clear_and_waiti_wake() {
    let (mut c, mut b) = machine(corpus::WAIT);
    c.write_sr(sr::INTENABLE, 1 << 6).unwrap();
    c.write_sr(sr::CCOUNT, u32::MAX - 1).unwrap();
    c.write_sr(sr::CCOMPARE0, 1).unwrap();
    assert_eq!(c.step(&mut b).result(), Ok(()));
    assert!(c.waiting()); assert_eq!(c.cycles_until_wake(), Some(2));
    c.advance_cycles(1); assert!(!c.irq_pending());
    c.advance_cycles(1); assert!(c.irq_pending());
    assert_eq!(c.state().interrupt, 1 << 6);
    assert_eq!(c.step(&mut b).trap(), Some(Trap::Interrupt(6)));
    assert_eq!(c.pc(), 0x40000030); assert!(!c.waiting());
    c.write_sr(sr::CCOMPARE0, 1).unwrap();
    assert_eq!(c.state().interrupt, 0);
    c.write_sr(sr::PS, 0).unwrap(); c.set_pc(corpus::WAIT);
    c.write_sr(sr::CCOUNT, 0).unwrap();
    c.step(&mut b); // CCOUNT == CCOMPARE, pending
    c.write_sr(sr::CCOMPARE0, 1).unwrap();
    assert_eq!(c.cycles_until_wake(), Some(1u64 << 32));
    c.advance_cycles(0); assert!(!c.irq_pending());
    c.advance_cycles(u32::MAX); assert!(!c.irq_pending());
    c.advance_cycles(1); assert!(c.irq_pending());
    // A masked timer cannot wake WAITI.
    c.write_sr(sr::CCOMPARE0, 10).unwrap(); c.write_sr(sr::PS, 1).unwrap();
    assert_eq!(c.cycles_until_wake(), None);
}

#[test]
fn interrupt_types_excm_nmi_and_return() {
    let (mut c, mut b) = machine(BASE);
    c.set_irq(u32::MAX); c.set_irq(0);
    assert_eq!(c.state().interrupt, 0x7f00); // edge + NMI, no timers/software invented
    c.write_sr(sr::INTCLEAR, u32::MAX).unwrap();
    assert_eq!(c.state().interrupt, 0x4000); // NMI cannot be software-cleared
    c.write_sr(sr::PS, 0x1f).unwrap();
    assert_eq!(c.step(&mut b).trap(), Some(Trap::Interrupt(14)));
    assert_eq!(c.pc(), 0x40000020);
    assert_eq!((c.state().epc[3], c.state().eps[3]), (BASE, 0x1f));
    // RFI 3 restores NMI context. No level 7 slot.
    c.set_pc(corpus::RETURN_NMI);
    assert_eq!(c.step(&mut b).result(), Ok(()));
    assert_eq!((c.pc(), c.state().ps), (BASE, 0x1f));
    c.write_sr(sr::PS, 0).unwrap(); c.write_sr(sr::INTENABLE, u32::MAX).unwrap();
    c.set_irq(1); assert!(c.irq_pending());
    c.write_sr(sr::INTCLEAR, 1).unwrap(); assert!(c.irq_pending());
    c.set_irq(0); assert!(!c.irq_pending());
    c.write_sr(sr::INTSET, u32::MAX).unwrap();
    assert_eq!(c.state().interrupt, 0x80);
    c.write_sr(sr::PS, ps::EXCM).unwrap(); assert!(!c.irq_pending());
    c.write_sr(sr::PS, 0).unwrap();
    assert_eq!(c.step(&mut b).trap(), Some(Trap::Interrupt(7)));
    assert_eq!((c.pc(), c.state().epc[1]), (0x40000030, BASE));
    // RFE removes EXCM and restores EPC1.
    c.set_pc(corpus::RETURN_EXCEPTION);
    c.write_sr(sr::INTCLEAR, 0x80).unwrap();
    assert_eq!(c.step(&mut b).result(), Ok(())); assert_eq!(c.state().ps & ps::EXCM, 0);
}

#[test]
fn jit_requests_cannot_bypass_profile() {
    let (mut c, mut b) = machine(BASE);
    b.mem[..3].copy_from_slice(&[0x30, 0x42, 0xd2]); // quos
    c.set_jit(true);
    assert_eq!(c.run(&mut b, 100), (1, Some(Trap::Exception(exc::ILLEGAL))));
    assert_eq!(c.code_cache_stats(), None);
    assert_eq!(c.run_prepared(&mut b, 100), None);
    assert!(matches!(c.step(&mut b).kind, StepKind::TrapBefore(_)));
}

#[test]
fn software_division_is_an_ordinary_call0_function() {
    for (x, y, expected) in [(27, 4, 6), (3, 7, 0), (45, 5, 9)] {
        let (mut c, mut b) = machine(corpus::SOFTWARE_DIVIDE);
        c.set_ar(1, BASE + 0x8000); c.set_ar(0, 0x12345678);
        c.set_ar(2, x); c.set_ar(3, y);
        for _ in 0..100 {
            if c.pc() == corpus::DIVIDE_DONE { break; }
            assert_eq!(c.step(&mut b).result(), Ok(()));
        }
        assert_eq!((c.pc(), c.get_ar(2), c.get_ar(0), c.get_ar(1)),
            (corpus::DIVIDE_DONE, expected, 0x12345678, BASE + 0x8000));
    }
}

#[test]
fn debug_uses_level_two_and_never_s3_level_six() {
    let (mut c, mut b) = machine(corpus::DEBUG_BREAK);
    assert_eq!(c.step(&mut b).trap(), Some(Trap::Exception(0x108)));
    assert_eq!((c.pc(), c.state().epc[2], c.state().eps[2]),
        (0x40000010, corpus::DEBUG_BREAK, 0));
    assert_eq!(c.state().epc[6], 0);
    assert_eq!(c.state().debugcause, 1 << 3); // BI
}

#[test]
fn break_n_cause_and_break_masked_at_debug_level() {
    // break 1, 2 and break.n 1 (Xtensa ISA RRR/RRRN encodings).
    for (raw, cause) in [(&[0x20u8, 0x41, 0x00][..], 1u32 << 3), (&[0x2d, 0xf1][..], 1 << 4)] {
        for (ps, taken) in [(0, true), (1, true), (ps::EXCM, true), (ps::EXCM | 1, true), (2, false), (3, false), (15, false), (ps::EXCM | 2, false)] {
            let (mut c, mut b) = machine(BASE);
            b.mem[..raw.len()].copy_from_slice(raw);
            c.write_sr(sr::PS, ps).unwrap();
            let out = c.step(&mut b);
            if taken {
                assert_eq!(out.trap(), Some(Trap::Exception(0x108)), "PS {ps:#x}");
                assert_eq!((c.pc(), c.state().debugcause, c.state().epc[2], c.state().eps[2]),
                    (0x40000010, cause, BASE, ps), "PS {ps:#x}");
                assert_eq!(c.state().ps, (ps & !ps::INTLEVEL_MASK) | ps::EXCM | 2);
            } else {
                // A BREAK at or above DEBUGLEVEL retires as a no-op: no debug state changes.
                assert_eq!(out.kind, StepKind::Retired, "PS {ps:#x}");
                assert_eq!((c.pc(), c.state().ps, c.state().debugcause, c.state().epc[2], c.state().eps[2]),
                    (BASE + raw.len() as u32, ps, 0, 0, 0), "PS {ps:#x}");
            }
        }
    }
}

#[test]
fn assembled_special_register_accesses() {
    let (mut c, mut b) = machine(corpus::SPECIAL_REGISTERS);
    c.set_ar(2, BASE + 0x2001); c.set_ar(4, BASE + 0x3000); c.set_ar(5, 100);
    assert_eq!(c.run(&mut b, 5), (5, None));
    assert_eq!(c.get_ar(3), BASE + 0x2001);
    assert_eq!(c.get_ar(4), BASE + 0x2001);
    assert_eq!(c.read_sr(5), Some(BASE + 0x3000));
    assert_eq!(c.get_ar(6), 100);
}

#[test]
fn edge_latches_rearm_and_masked_waiti_takes_nmi() {
    let (mut c, mut b) = machine(corpus::WAIT);
    c.write_sr(sr::INTENABLE, 1 << 8).unwrap();
    c.set_irq(1 << 8); c.set_irq(0); assert!(c.irq_pending());
    c.write_sr(sr::INTCLEAR, 1 << 8).unwrap(); assert!(!c.irq_pending());
    c.set_irq(1 << 8); assert!(c.irq_pending());
    c.write_sr(sr::INTCLEAR, 1 << 8).unwrap();
    c.set_irq(1 << 8); assert!(!c.irq_pending());
    c.write_sr(sr::INTENABLE, 0).unwrap();
    c.step(&mut b); assert!(c.waiting());
    c.write_sr(sr::PS, 0x1f).unwrap();
    c.set_irq(1 << 14); assert!(c.irq_pending());
    assert_eq!(c.step(&mut b).trap(), Some(Trap::Interrupt(14)));
    assert!(!c.waiting()); assert_eq!(c.pc(), 0x40000020);
    c.set_irq(1 << 14); assert!(!c.irq_pending());
    c.set_irq(0); c.set_irq(1 << 14); assert!(c.irq_pending());
}

#[test]
fn sr_direction_matrix_and_read_only_reads() {
    // Independent direction table: every supported SR, not the implementation helpers.
    let all = [3u8, 5, 96, 104, 128, 144, 160, 176, 177, 178, 179, 192,
        194, 195, 208, 209, 210, 211, 226, 227, 228, 230, 231, 232, 233,
        234, 235, 236, 237, 238, 240];
    let readonly = [176, 208, 233, 235];
    for n in 0..=255u8 {
        for (op, code) in [(xtensa_lx7::Op::Rsr, 0x03), (xtensa_lx7::Op::Wsr, 0x13), (xtensa_lx7::Op::Xsr, 0x61)] {
            let i = decode(BASE, [0x20, n, code, 0]);
            assert_eq!(i.op, op);
            let expected = all.contains(&n) && match op {
                xtensa_lx7::Op::Rsr => n != 227,
                xtensa_lx7::Op::Wsr => !readonly.contains(&n),
                _ => !readonly.contains(&n) && n != 226 && n != 227,
            };
            assert_eq!(supports(&i), expected, "SR {n} {op:?}");
        }
        let mut c = Lx106::new();
        assert_eq!(c.read_sr(n as u32).is_some(), all.contains(&n) && n != 227);
        assert_eq!(c.write_sr(n as u32, 0).is_some(), all.contains(&n) && !readonly.contains(&n));
    }
    for (n, value) in [(176u8, 0xc28cdafa), (208, 0x1082b6f6), (233, 0), (235, 0)] {
        let (mut c, mut b) = machine(BASE);
        b.mem[..4].copy_from_slice(&[0x20, n, 0x03, 0]);
        c.set_ar(2, 0xdeadbeef);
        assert_eq!(c.step(&mut b).kind, StepKind::Retired);
        assert_eq!((c.get_ar(2), c.pc()), (value, BASE + 3));
    }
}

#[test]
fn raw_sr_direction_rejections_preserve_registers_before_exception() {
    for &(name, raw) in corpus::SR_FORBIDDEN {
        for mask in [0, ps::UM, ps::EXCM] {
            let (mut c, mut b) = machine(BASE);
            b.mem[..raw.len()].copy_from_slice(raw);
            c.write_sr(sr::PS, mask).unwrap();
            for n in 0..16 { c.set_ar(n, 0xdeadbeef ^ u32::from(n ^ 2)); }
            // Nonzero pending state exposes forbidden XSR INTSET/CLEAR writes.
            c.set_irq(1 << 8); c.set_irq(0);
            c.write_sr(sr::INTSET, 0x80).unwrap();
            let before: Vec<_> = (0..256).map(|n| c.read_sr(n)).collect();
            assert_eq!(c.step(&mut b).trap(), Some(Trap::Exception(exc::ILLEGAL)), "{name}");
            for n in 0..16 { assert_eq!(c.get_ar(n), 0xdeadbeef ^ u32::from(n ^ 2), "{name} a{n}"); }
            for n in 0..256 {
                // Only exception entry and the instruction cycle charge may change.
                if [sr::PS, sr::EPC1, sr::DEPC, sr::EXCCAUSE, sr::CCOUNT].contains(&n) { continue; }
                assert_eq!(c.read_sr(n), before[n as usize], "{name} SR{n}");
            }
            let offset = if mask & ps::EXCM != 0 { 0x70 } else if mask & ps::UM != 0 { 0x50 } else { 0x30 };
            assert_eq!(c.pc(), 0x40000000 + offset, "{name}");
            assert_eq!(c.state().exccause, exc::ILLEGAL);
            assert_eq!(if mask & ps::EXCM != 0 { c.state().depc } else { c.state().epc[1] }, BASE);
        }
    }
}

#[test]
fn unaligned_word_access_has_lx106_alignment_exception() {
    for pc in [corpus::LEAF, corpus::START + 3] { // assembled L32I and S32I
        let (mut c, mut b) = machine(pc);
        c.set_ar(1, BASE + 0x8001);
        assert_eq!(c.step(&mut b).trap(), Some(Trap::Exception(exc::LOAD_STORE_ALIGNMENT)));
        assert_eq!(c.pc(), 0x40000030);
        assert_eq!(c.state().epc[1], pc);
        assert_eq!(c.state().excvaddr, BASE + 0x8001 + if pc == corpus::LEAF { 0 } else { 28 });
    }
}

#[test]
fn rer_wer_trap_unimplemented_without_effect() {
    // rer a2, a3 and wer a2, a3 (Xtensa ISA RRR: op2 4, r 6/7). The SoC must supply
    // these; the generic executor's read-zero/no-op must not leak into the profile.
    for raw in [[0x20u8, 0x63, 0x40], [0x20, 0x73, 0x40]] {
        let (mut c, mut b) = machine(BASE);
        b.mem[..3].copy_from_slice(&raw);
        c.set_ar(2, 0x1234); c.set_ar(3, 0x5678);
        assert_eq!(c.step(&mut b).trap(), Some(Trap::Unimplemented(BASE, decode(BASE, [raw[0], raw[1], raw[2], 0]).raw)));
        assert_eq!((c.pc(), c.get_ar(2), c.get_ar(3)), (BASE, 0x1234, 0x5678));
    }
}
