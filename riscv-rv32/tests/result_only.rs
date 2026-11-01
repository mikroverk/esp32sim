//! The result-only run path and observing step path must stay architecturally identical.
use emu_core::Core;
use riscv_rv32::{Cpu, FlatRam, step};

const BASE: u32 = 0x4038_0000;

fn setup(word: u32, mode: u32) -> (Cpu, FlatRam) {
    let mut cpu = Cpu::new_rv32imac();
    cpu.pc = if mode == 6 { BASE + 64 } else { BASE };
    for (n, r) in cpu.x.iter_mut().enumerate().skip(1) {
        *r = match n % 3 { 0 => BASE + 32, 1 => u32::MAX, _ => n as u32 };
    }
    cpu.mtvec = BASE + 48;
    cpu.mepc = BASE + 16;
    cpu.mstatus = if mode & 1 != 0 { 0x1808 } else { 0x1800 };
    cpu.waiting = mode == 2 || mode == 3;
    cpu.irq = (mode == 3 || mode == 4 || mode == 5).then_some(3);
    cpu.reservation = Some(BASE + 32);
    cpu.insn_count = 100;
    cpu.retired_count = 80;
    cpu.cycle_count = 120;
    cpu.cycle_base = 10;
    cpu.instret_base = 5;
    cpu.csr_other.insert(0x777, 0x1234);
    let mut ram = FlatRam::new(BASE, 64);
    ram.mem[..4].copy_from_slice(&word.to_le_bytes());
    ram.mem[32..36].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    (cpu, ram)
}

fn assert_cpu(a: &Cpu, b: &Cpu) {
    assert_eq!(a.pc, b.pc);
    assert_eq!(a.x, b.x);
    assert_eq!((a.mstatus, a.mtvec, a.mepc, a.mcause, a.mtval, a.mie, a.mscratch),
               (b.mstatus, b.mtvec, b.mepc, b.mcause, b.mtval, b.mie, b.mscratch));
    assert_eq!((a.insn_count, a.retired_count, a.cycle_count, a.cycle_base, a.instret_base),
               (b.insn_count, b.retired_count, b.cycle_count, b.cycle_base, b.instret_base));
    assert_eq!((a.waiting, a.irq, a.misa, a.reservation, a.boundary_bloom),
               (b.waiting, b.irq, b.misa, b.reservation, b.boundary_bloom));
    assert_eq!((a.has_ext_ill, &a.f, a.fcsr, a.ext_ill, a.ext_ill_all_fp, a.user_counters),
               (b.has_ext_ill, &b.f, b.fcsr, b.ext_ill, b.ext_ill_all_fp, b.user_counters));
    assert_eq!((a.clic, a.clic_std, a.mintthresh, a.mtvt, a.hartid, a.mil),
               (b.clic, b.clic_std, b.mintthresh, b.mtvt, b.hartid, b.mil));
    assert_eq!(a.csr_other, b.csr_other);
}

fn compare(word: u32, mode: u32) {
    let (mut observed, mut observed_ram) = setup(word, mode);
    let outcome = observed.step(&mut observed_ram);
    assert_eq!((observed.insn_count, observed.cycle_count), (101, 121));
    assert_eq!(observed.retired_count, 80 + u64::from(outcome.kind == emu_core::StepKind::Retired));
    let (mut result_only, mut result_ram) = setup(word, mode);
    assert_eq!(step(&mut result_only, &mut result_ram), outcome.result(), "word={word:08x}, mode={mode}");
    assert_cpu(&observed, &result_only);
    assert_eq!(observed_ram.mem, result_ram.mem);
    let (mut run, mut run_ram) = setup(word, mode);
    assert_eq!(run.run(&mut run_ram, 1), (1, outcome.result().err()));
    assert_cpu(&observed, &run);
    assert_eq!(observed_ram.mem, run_ram.mem);
}

#[test]
fn result_only_matches_facts_for_compressed_and_full_instructions() {
    // Every compressed encoding, including illegal ones and a nonzero upper fetch half.
    for half in 0..=u16::MAX {
        if half & 3 != 3 { compare(0xa5a5_0000 | u32::from(half), 0); }
    }
    let mut word = 0x1234_5678u32;
    for _ in 0..16_384 {
        word = word.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        compare(word | 3, 0);
    }
    // Retire, illegal, traps, WFI, MRET, fence.i, LR/SC, store, CSR; plus idle,
    // interrupt gating/wakeup and failed fetch. These do not depend on random coverage.
    for word in [0x0050_0093, 0xffff_ffff, 0x0000_0073, 0x0010_0073,
                 0x1050_0073, 0x3020_0073, 0x0000_100f, 0x1001_a2af,
                 0x1851_a2af, 0x0051_a023, 0xb020_22f3] {
        for mode in 0..7 { compare(word, mode); }
    }
}
