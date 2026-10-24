//! Hermetic tests of what the core does, on a flat RAM: the register-window, loop, timer and
//! interrupt mechanics the firmware leans on, and the oracle property — one instruction at a
//! time, the block interpreter and the JIT must leave identical state.
//!
//! Programs were assembled with `xtensa-esp32s3-elf-as` (esp-14.2.0); the listing's hex groups
//! are pasted as-is and turned into memory order here. The decoder's disassembly is checked
//! against the listing's first instruction so a paste error cannot pass silently.
use emu_core::Core;
use xtensa_lx7::block::run_block;
use xtensa_lx7::state::{exc, ps, vec};
use xtensa_lx7::{decode, disasm, step, Cpu, FlatRam, Trap};

const BASE: u32 = 0x4037_0000;
const VECBASE: u32 = 0x4037_8000;

/// objdump prints each Xtensa instruction as one big-endian hex group; memory is little-endian.
fn asm(listing: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for g in listing.split_whitespace() { let mut b: Vec<u8> = (0..g.len() / 2).map(|i| u8::from_str_radix(&g[2 * i..2 * i + 2], 16).unwrap()).collect(); b.reverse(); out.extend(b); }
    out
}

fn machine(prog: &[u8], first: &str) -> (Cpu, FlatRam) {
    let mut ram = FlatRam::new(BASE, 64 * 1024);
    ram.mem[..prog.len()].copy_from_slice(prog);
    let mut cpu = Cpu::new(0);
    cpu.pc = BASE; cpu.ps = 0; cpu.vecbase = VECBASE;
    let mut b = [0u8; 4]; b.copy_from_slice(&ram.mem[..4]);
    assert_eq!(disasm::format(&decode(BASE, b)), first, "the pasted program does not start with the expected instruction");
    (cpu, ram)
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode { Step, Blocks, Jit }

/// Run up to `n` scheduling steps (instructions, or one for a trap taken first); with `stop`,
/// return at the first trap so its state can be inspected. Returns the traps seen.
fn run(cpu: &mut Cpu, bus: &mut FlatRam, mode: Mode, n: u32, stop: bool) -> Vec<(u64, Trap)> {
    let mut traps = Vec::new();
    match mode {
        Mode::Step => for _ in 0..n { if let Err(t) = step(cpu, bus) { traps.push((cpu.insn_count, t)); if stop { break; } } },
        Mode::Blocks | Mode::Jit => {
            cpu.blocks.jit_enabled = mode == Mode::Jit;
            let mut left = n;
            while left > 0 { let (used, t) = run_block(cpu, bus, left); left -= used.min(left); if let Some(t) = t { traps.push((cpu.insn_count, t)); if stop { break; } } }
        }
    }
    traps
}

fn state(cpu: &Cpu) -> (Vec<u32>, u32, u32, u32, u32, u32, u64, u32, u32, u32) {
    (cpu.ar.to_vec(), cpu.windowbase, cpu.windowstart, cpu.pc, cpu.ps, cpu.sar, cpu.insn_count, cpu.ccount, cpu.lcount, cpu.lend)
}

#[test]
fn floating_postincrement_accesses_the_original_address() {
    for mode in [Mode::Step, Mode::Blocks, Mode::Jit] {
        let (mut cpu, mut ram) = machine(&asm("018213 01c313 182450 582650 ffff06"), "lsip f1, a2, 4");
        cpu.cpenable = 1;
        for (reg, offset) in [(2, 0x100), (3, 0x200), (4, 0x300), (6, 0x400)] { cpu.set_ar(reg, BASE + offset); }
        cpu.set_ar(5, 8);
        ram.mem[0x100..0x108].copy_from_slice(&[0x11; 8]);
        ram.mem[0x104..0x108].copy_from_slice(&[0x22; 4]);
        ram.mem[0x300..0x30c].copy_from_slice(&[0x33; 12]);
        ram.mem[0x308..0x30c].copy_from_slice(&[0x44; 4]);
        run(&mut cpu, &mut ram, mode, 4, false);
        assert_eq!(cpu.fr[1], 0x11111111, "{mode:?}: LSIP reads before updating");
        assert_eq!(cpu.fr[2], 0x33333333, "{mode:?}: LSXP reads before updating");
        assert_eq!(&ram.mem[0x200..0x208], &[0x11, 0x11, 0x11, 0x11, 0, 0, 0, 0]);
        assert_eq!(&ram.mem[0x400..0x40c], &[0x33, 0x33, 0x33, 0x33, 0, 0, 0, 0, 0, 0, 0, 0]);
        for (reg, offset) in [(2, 0x104), (3, 0x204), (4, 0x308), (6, 0x408)] { assert_eq!(cpu.get_ar(reg), BASE + offset); }
    }
}

#[test]
fn standalone_wakenet_float_primitives_write_their_results() {
    for mode in [Mode::Step, Mode::Blocks, Mode::Jit] {
        let (mut cpu, mut ram) = machine(&asm("fa1080 fa20a0 6a3010 ffff06"), "recip0.s f1, f0");
        cpu.cpenable = 1;
        cpu.fr[0] = 4f32.to_bits();
        cpu.fr[3] = 1f32.to_bits();
        run(&mut cpu, &mut ram, mode, 3, false);
        assert_eq!(cpu.fr[1], 0.25f32.to_bits(), "{mode:?}: reciprocal");
        assert_eq!(cpu.fr[2], 0.5f32.to_bits(), "{mode:?}: reciprocal square root");
        assert_eq!(cpu.fr[3], 2f32.to_bits(), "{mode:?}: nearest fused multiply-add");
        for (input, reciprocal, root) in [
            (-0.0f32, f32::NEG_INFINITY, Some(f32::NEG_INFINITY)),
            (f32::INFINITY, 0.0, Some(0.0)),
            (-4.0, -0.25, None),
        ] {
            let (mut cpu, mut ram) = machine(&asm("fa1080 fa20a0 6a3010 ffff06"), "recip0.s f1, f0");
            cpu.cpenable = 1;
            cpu.fr[0] = input.to_bits();
            run(&mut cpu, &mut ram, mode, 2, false);
            assert_eq!(cpu.fr[1], reciprocal.to_bits());
            if let Some(root) = root { assert_eq!(cpu.fr[2], root.to_bits()); }
            else { assert!(f32::from_bits(cpu.fr[2]).is_nan()); }
        }
    }
}

#[test]
fn zero_overhead_loop_runs_its_count() {
    let (mut cpu, mut ram) = machine(&asm("030c 05a022 018276 331b ffff06"), "movi.n a3, 0");
    run(&mut cpu, &mut ram, Mode::Step, 40, false);
    assert_eq!(cpu.get_ar(3), 5);
    assert_eq!(cpu.pc, BASE + 10, "parked on the final j");
}

const MIX: &str = "030c e8a342 0239 0258 333b 1155f0 416350 307560 1279 021282 084282 080292 aa9a e09347 000001 0000c0 0000c6 bb7b f00d ffff06";

/// The interpreter, the block interpreter and the JIT are three implementations of one
/// semantics: after the same number of instructions every register, the cycle counter and
/// memory must agree.
#[test]
fn step_blocks_and_jit_agree() {
    // mix.S starts with `movi a2, 0x40370100`, which the assembler turns into an l32r; the data
    // address is handed to a2 directly instead and the program starts at the next instruction
    let prog = asm(MIX);
    let mut results = Vec::new();
    for mode in [Mode::Step, Mode::Blocks, Mode::Jit] {
        let mut ram = FlatRam::new(BASE, 64 * 1024);
        ram.mem[..prog.len()].copy_from_slice(&prog);
        let mut cpu = Cpu::new(0);
        cpu.pc = BASE; cpu.ps = 0; cpu.vecbase = VECBASE; cpu.set_ar(2, BASE + 0x100);
        let traps = run(&mut cpu, &mut ram, mode, 3000, false);
        assert!(traps.is_empty(), "{:?}: unexpected traps {:?}", mode, traps);
        assert_eq!(cpu.insn_count, 3000, "{:?}", mode);
        results.push((mode, state(&cpu), ram.mem[0x100..0x110].to_vec()));
    }
    let (_, s0, m0) = &results[0];
    for (mode, s, m) in &results[1..] { assert_eq!(s, s0, "{:?} state differs from Step", mode); assert_eq!(m, m0, "{:?} memory differs from Step", mode); }
    assert_ne!(results[0].1 .0[3], 0, "the loop ran (a3 advanced)");
    assert_ne!(u32::from_le_bytes(m0[0..4].try_into().unwrap()), 0, "the loop stored to memory");
}

/// A call8 chain deeper than the 16-window file: the overflow exception is raised at the
/// `call8` that first touches the wrapped window (not at the `entry`), with the frame-size-2 vector.
#[test]
fn window_overflow_is_raised_at_the_call_that_touches_it() {
    let (mut cpu, mut ram) = machine(&asm("020c 000065 000246 004136 221b ffffa5 f01d ffff06"), "movi.n a2, 0");
    cpu.ps = ps::WOE;
    let mut first = None;
    for _ in 0..200 { if let Err(t) = step(&mut cpu, &mut ram) { first = Some(t); break; } }
    assert_eq!(first, Some(Trap::Exception(0x202)), "window overflow 8 expected");
    assert_eq!(cpu.pc, VECBASE + vec::WINDOW_OF8);
    assert_eq!(cpu.epc[1], BASE + 0xd, "EPC1 points at the call8 that first touches the wrapped window (verified against silicon)");
    assert!(cpu.ps & ps::EXCM != 0);
    // each frame is entry + addi + call8; the 16 windows hold seven frames of two before wrapping
    assert!((15..=40).contains(&cpu.insn_count), "instructions before the overflow: {}", cpu.insn_count);
    assert_eq!(cpu.windowstart.count_ones(), 8, "seven frames plus the wrapped one are live");
}

const TIMER: &str = "020c 13ea20 421c 13f020 024c 13e420 002010 030c 331b fffe86";

/// CCOMPARE0 = 20 with the timer line enabled: the level-1 interrupt is taken at the instruction
/// boundary where CCOUNT reaches 20 — in all three execution modes, at the same instruction.
#[test]
fn ccompare_interrupt_lands_on_the_same_instruction_in_every_mode() {
    let mut at = Vec::new();
    for mode in [Mode::Step, Mode::Blocks, Mode::Jit] {
        let (mut cpu, mut ram) = machine(&asm(TIMER), "movi.n a2, 0");
        let traps = run(&mut cpu, &mut ram, mode, 200, true);
        let (i, t) = traps.first().copied().unwrap_or_else(|| panic!("{:?}: no trap", mode));
        assert_eq!(t, Trap::Interrupt(6), "{:?}", mode);
        assert_eq!(cpu.exccause, exc::LEVEL1_INTERRUPT);
        at.push((mode, i));
    }
    let (mut cpu, mut ram) = machine(&asm(TIMER), "movi.n a2, 0");
    let mut taken = None;
    for k in 0..200u64 { match step(&mut cpu, &mut ram) { Err(Trap::Interrupt(6)) => { taken = Some(k); break; } Err(t) => panic!("{:?}", t), Ok(()) => {} } }
    let k = taken.unwrap();
    assert!((20..=21).contains(&cpu.ccount), "ccount at delivery: {}", cpu.ccount);
    assert_eq!(cpu.pc, VECBASE + vec::KERNEL, "level-1 interrupts go through the kernel vector (UM=0)");
    assert!(cpu.epc[1] == BASE + 0x14 || cpu.epc[1] == BASE + 0x16, "EPC1 is an instruction of the loop: {:#x}", cpu.epc[1]);
    assert!(k > 8, "delivered after the setup");
    assert!(at.iter().all(|(_, i)| *i == at[0].1), "delivery instruction differs between modes: {:?}", at);
}

/// With PS.INTLEVEL raised to 3 the pending level-1 line waits; `rsil 0` releases it.
#[test]
fn intlevel_masks_and_rsil_releases() {
    let (mut cpu, mut ram) = machine(&asm("020c 13ea20 a20c 13f020 024c 13e420 006340 030c 331b fac366 006040 331b fffe86"), "movi.n a2, 0");
    let mut taken_at_a3 = None;
    for _ in 0..400 {
        match step(&mut cpu, &mut ram) {
            Err(Trap::Interrupt(6)) => { taken_at_a3 = Some(cpu.get_ar(3)); break; }
            Err(t) => panic!("{:?}", t),
            Ok(()) => { if cpu.ccount > 12 && cpu.get_ar(3) < 32 { assert_ne!(cpu.interrupt & (1 << 6), 0, "the line is pending while masked"); assert_eq!(cpu.intlevel(), 3); } }
        }
    }
    assert_eq!(taken_at_a3, Some(32), "delivered right after rsil 0, once the masked loop finished");
}

/// `waiti` parks the core until a line the machine presents is unmasked.
#[test]
fn waiti_sleeps_until_a_line_arrives() {
    let (mut cpu, mut ram) = machine(&asm("024c 13e420 007000 ffff06"), "movi.n a2, 64");   // intenable=0x40 (the timer line); waiti 0; j .
    for _ in 0..3 { step(&mut cpu, &mut ram).unwrap(); }
    assert!(cpu.waiting());
    let before = cpu.insn_count;
    step(&mut cpu, &mut ram).unwrap();
    assert!(cpu.waiting(), "still asleep with nothing pending"); assert_eq!(cpu.insn_count, before);
    cpu.set_irq(1 << 6);
    assert!(!cpu.irq_pending(), "the timer bit is the core's own: the SoC cannot set it through set_irq");
    cpu.interrupt |= 1 << 6;   // what advance_ccount does on a CCOMPARE match
    assert!(cpu.irq_pending());
    match step(&mut cpu, &mut ram) { Err(Trap::Interrupt(6)) => {} r => panic!("{:?}", r) }
    assert!(!cpu.waiting());
}

/// RSR PS remains inside the block, but enabling an already pending interrupt ends it.
#[test]
fn rsr_ps_does_not_split_block_before_interrupt_enable() {
    // add.n a3,a3,a3; rsr a4,PS; add.n a3,a3,a3; wsr a2,INTENABLE; nop
    let program = asm("333a 03e640 333a 13e420 0020f0");
    for jit in [false, true] {
        let (mut c, mut bus) = machine(&program, "add.n a3, a3, a3");
        let (mut oracle, mut reference) = machine(&program, "add.n a3, a3, a3");
        for cpu in [&mut c, &mut oracle] {
            cpu.set_ar(3, 1);
            cpu.set_ar(2, 1 << 6);
            cpu.interrupt = 1 << 6;
        }
        c.blocks.jit_enabled = jit;
        assert_eq!(run_block(&mut c, &mut bus, 20), (4, None));
        for _ in 0..4 { step(&mut oracle, &mut reference).unwrap(); }
        assert_eq!((c.pc, c.ar, c.ps, c.intenable, c.insn_count),
            (oracle.pc, oracle.ar, oracle.ps, oracle.intenable, oracle.insn_count));
        assert_eq!(c.get_ar(3), 4);
        assert_eq!(run_block(&mut c, &mut bus, 20), (1, Some(Trap::Interrupt(6))));
        assert_eq!(step(&mut oracle, &mut reference), Err(Trap::Interrupt(6)));
        assert_eq!((c.pc, c.epc), (oracle.pc, oracle.epc));
    }
}
