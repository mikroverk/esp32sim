//! Encodings independently assembled from corpus/operand-regressions.S with
//! Espressif esp-15.2.0_20251204 xtensa-esp32s3-elf-as --no-transform.
use xtensa_lx7::{block::run_block, decode, step, Cpu, FlatRam, Op, Trap};
use xtensa_lx7::state::ps;
const BASE: u32 = 0x4037_0000;

fn machine(word: u32) -> (Cpu, FlatRam) {
    let mut ram = FlatRam::new(BASE, 4096);
    ram.mem[..4].copy_from_slice(&word.to_le_bytes());
    let mut cpu = Cpu::new(0);
    cpu.pc = BASE;
    cpu.ps = ps::WOE;
    cpu.windowbase = 0;
    cpu.windowstart = 3; // a4 and above conflict with a live frame.
    cpu.cpenable = 1;
    cpu.set_ar(2, 0);
    cpu.set_ar(3, BASE + 0x100);
    (cpu, ram)
}

#[test]
fn selectors_immediates_and_fp_registers_do_not_spill_integer_windows() {
    use Op::*;
    let cases = [
        (0x23fb, AddiN), (0x40f320, Nsau), (0x412c30, Srli),
        (0x08f230, Lsx), (0x8b0f20, MoveqzS), (0x40e320, Nsa),
        (0xf42f30, Extui), (0x312f30, Srai), (0x09f320, L32e),
        (0x49f320, S32e), (0x59f320, S32nb), (0x48f230, Ssx), (0x18f230, Lsxp),
        (0x58f230, Ssxp), (0x9b0f20, MovnezS), (0xab0f20, MovltzS),
        (0xbb0f20, MovgezS), (0x406320, Rer), (0x407320, Wer),
        (0x503320, Ritlb0), (0x507320, Ritlb1), (0x50b320, Rdtlb0),
        (0x50f320, Rdtlb1), (0x505320, Pitlb), (0x50d320, Pdtlb),
        (0x506320, Witlb), (0x50e320, Wdtlb),
        (0xc323f0, Movf), (0x244044, Mac16), (0x340244, Mac16), (0x644034, Mac16), (0x800304, Mac16),
    ];
    for (word, op) in cases {
        for block in [false, true] {
            let (mut cpu, mut ram) = machine(word);
            let insn = decode(BASE, word.to_le_bytes());
            assert_eq!(insn.op, op);
            if matches!(op, Lsxp | Ssxp) {
                cpu.set_ar(2, BASE + 0x100);
                cpu.set_ar(3, 0);
            }
            cpu.blocks.jit_enabled = false;
            let trap = if block { run_block(&mut cpu, &mut ram, 1).1 } else { step(&mut cpu, &mut ram).err() };
            assert_eq!(trap, None, "{op:?}, block={block}");
            assert_eq!(cpu.pc, BASE + insn.len as u32, "{op:?}");
            assert_eq!(cpu.insn_count, 1, "{op:?}");
            assert_eq!(cpu.windowbase, 0, "{op:?}");
            assert_eq!(cpu.windowstart, 3, "{op:?}");
        }
    }
}

#[test]
fn real_high_integer_reads_and_destinations_still_spill_before_retiring() {
    for word in [0x8023f0u32, 0xf3fb, 0x482f30, 0x8b01f0, 0x93f230] {
        for block in [false, true] {
            let (mut cpu, mut ram) = machine(word);
            cpu.blocks.jit_enabled = false;
            let before = cpu.ar;
            let trap = if block { run_block(&mut cpu, &mut ram, 1).1 } else { step(&mut cpu, &mut ram).err() };
            assert!(matches!(trap, Some(Trap::Exception(_))), "{word:x}, block={block}");
            assert_eq!(cpu.insn_count, 0);
            assert_eq!(cpu.epc[1], BASE);
            assert_eq!(cpu.ar, before);
        }
    }
}

#[test]
fn corrected_operands_produce_expected_values() {
    // Assertions are instruction semantics, not comparisons to the same engine.
    for (word, input, expected) in [(0x23fbu32, 7, 22), (0x40f320, 0x100, 23), (0x412c30, 0x12345000, 0x12345)] {
        let (mut cpu, mut ram) = machine(word);
        cpu.set_ar(3, input);
        step(&mut cpu, &mut ram).unwrap();
        assert_eq!(cpu.get_ar(2), expected);
    }
    let (mut cpu, mut ram) = machine(0x59f320);
    cpu.set_ar(2, 0x12345678);
    step(&mut cpu, &mut ram).unwrap();
    assert_eq!(&ram.mem[0x13c..0x140], &0x12345678u32.to_le_bytes());
    let (mut cpu, mut ram) = machine(0x08f230);
    ram.mem[0x100..0x104].copy_from_slice(&0x12345678u32.to_le_bytes());
    step(&mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.fr[15], 0x12345678);
    let (mut cpu, mut ram) = machine(0x8b0f20);
    cpu.fr[15] = 0x87654321;
    step(&mut cpu, &mut ram).unwrap();
    assert_eq!(cpu.fr[0], 0x87654321);
}

#[test]
fn operand_directions_preserve_conditional_destinations_and_mac_selectors() {
    let effects = |word: u32| decode(BASE, word.to_le_bytes()).gpr_effects();
    for word in [0x614820, 0x134820, 0x408010, 0x002136, 0x000090] {
        assert!(effects(word).changes_window, "{word:x}");
    }
    assert_eq!(effects(0x614820).unclassified, 1 << 2);
    assert_eq!(effects(0x614820).writes, 0);
    assert_eq!(effects(0x93f230).reads, (1 << 2) | (1 << 3));
    assert_eq!(effects(0x93f230).writes, 0);
    assert_eq!(effects(0x93f230).conditional_writes, 1 << 15);
    assert_eq!(effects(0xc323f0).reads, 1 << 3);
    assert_eq!(effects(0xc323f0).conditional_writes, 1 << 2);
    assert_eq!(effects(0x244044).touched(), 0); // mul.dd.ll m1, m3
    assert_eq!(effects(0x340244).reads, 1 << 2); // mul.ad.ll a2, m3
    assert_eq!(effects(0x644034).reads, 1 << 3); // mul.da.ll m1, a3
    assert_eq!(effects(0x800304).writes, 1 << 3); // ldinc m0, a3
    assert_eq!(effects(0x18f230).reads, (1 << 2) | (1 << 3));
    assert_eq!(effects(0x18f230).writes, 1 << 2);
    assert_eq!(effects(0x8b0f20).reads, 1 << 2);
    assert_eq!(effects(0x8b0f20).writes, 0);
}
