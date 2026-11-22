//! Hermetic LX106 corpus, generated with the pinned toolchains in corpus/build-lx106.py.
use emu_core::{Bus, Core, StepKind};
use xtensa_lx7::{decode, lx106::supports, state::{exc, ps, sr}, FlatRam, Lx106, Trap};
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

#[test]
fn forbidden_options_raise_illegal_before_any_effect() {
    for &(name, raw) in corpus::FORBIDDEN {
        for mask in [0, ps::UM, ps::EXCM, ps::WOE | (3 << ps::CALLINC_SHIFT)] {
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
