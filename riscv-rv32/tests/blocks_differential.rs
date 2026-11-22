//! EX265: decoded blocks against the interpreter. Two identical cores on two identical buses
//! run the same sequence of `Core::run` calls, one with blocks (`set_jit(true)`) and one
//! stepping (`set_jit(false)`, the oracle), from random programs that mix every major opcode,
//! compressed forms, loads and stores into their own code (self-modifying code), device reads
//! with side effects, device writes that end a dispatch (`block_break`), traps, interrupts
//! presented between runs, budget cuts and machine boundaries. After every run the iterations,
//! the trap, the pc noted at each memory access and last, and the whole architectural state (and
//! memory) must agree. Where the host has a JIT backend (EX269) every block is also compiled on
//! its first whole run; a second set of programs, mostly valid loops, keeps regions chaining from
//! block to block.
use emu_core::{Bus, Core, Fault, Trap};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use riscv_rv32::Cpu;

const RAM: u32 = 0x4038_0000;
const RAM_SIZE: usize = 0x4000;
const MMIO: u32 = 0x6000_0000;
const HANDLER: u32 = RAM + 0x100;
const CODE: u32 = RAM + 0x200;
const CODE_BYTES: usize = 0x1000;
const DATA: u32 = RAM + 0x2000;

/// RAM with a code version per 256-byte page, read-ahead, and a device page whose reads count
/// and whose writes end the dispatch, as a SoC bus does; so does a read of its last register.
#[derive(Clone)]
struct TestBus { ram: Vec<u8>, ver: Vec<u32>, reads: u32, regs: [u32; 4], brk: bool, versions: bool, last_pc: u32, accesses: u64 }

impl TestBus {
    fn new() -> Self { TestBus { ram: vec![0; RAM_SIZE], ver: vec![0; RAM_SIZE >> 8], reads: 0, regs: [0; 4], brk: false, versions: true, last_pc: 0, accesses: 0 } }
    fn off(&self, a: u32, n: usize) -> Option<usize> {
        let o = a.wrapping_sub(RAM) as usize;
        (o.checked_add(n)? <= RAM_SIZE).then_some(o)
    }
    fn mmio(a: u32, n: usize) -> Option<usize> { (a >= MMIO && a as u64 + n as u64 <= MMIO as u64 + 16).then(|| ((a - MMIO) / 4) as usize) }
    /// A hash of every access, in order: the pc noted when it is made and its address.
    fn access(&mut self, a: u32) { self.accesses = (self.accesses.rotate_left(7) ^ (self.last_pc as u64) << 32 ^ a as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15); }
    fn read(&mut self, a: u32, n: usize) -> Result<u32, Fault> {
        self.access(a);
        if let Some(o) = self.off(a, n) { let mut b = [0; 4]; b[..n].copy_from_slice(&self.ram[o..o + n]); return Ok(u32::from_le_bytes(b)); }
        if let Some(r) = Self::mmio(a, n) { self.reads = self.reads.wrapping_add(1); self.brk |= r == 3; return Ok(self.regs[r] ^ self.reads); }
        Err(Fault::Unmapped)
    }
    fn write(&mut self, a: u32, v: u32, n: usize) -> Result<(), Fault> {
        self.access(a);
        if let Some(o) = self.off(a, n) {
            self.ram[o..o + n].copy_from_slice(&v.to_le_bytes()[..n]);
            let first = o.saturating_sub(3) >> 8;
            for v in &mut self.ver[first..=((o + n - 1) >> 8)] { *v = v.wrapping_add(1); }
            return Ok(());
        }
        if let Some(r) = Self::mmio(a, n) { self.regs[r] = v; self.brk = true; return Ok(()); }
        Err(Fault::Unmapped)
    }
}

impl Bus for TestBus {
    fn note_code_page(&mut self, _vidx: u32) {}
    fn note_pc(&mut self, pc: u32) { self.last_pc = pc; }
    fn read8(&mut self, a: u32) -> Result<u8, Fault> { self.read(a, 1).map(|v| v as u8) }
    fn read16(&mut self, a: u32) -> Result<u16, Fault> { self.read(a, 2).map(|v| v as u16) }
    fn read32(&mut self, a: u32) -> Result<u32, Fault> { self.read(a, 4) }
    fn write8(&mut self, a: u32, v: u8) -> Result<(), Fault> { self.write(a, v as u32, 1) }
    fn write16(&mut self, a: u32, v: u16) -> Result<(), Fault> { self.write(a, v as u32, 2) }
    fn write32(&mut self, a: u32, v: u32) -> Result<(), Fault> { self.write(a, v, 4) }
    fn fetch(&mut self, pc: u32) -> Result<[u8; 4], Fault> {
        let o = self.off(pc, 2).ok_or(Fault::Unmapped)?;
        let mut b = [0; 4];
        for (i, x) in b.iter_mut().enumerate() { if o + i < RAM_SIZE { *x = self.ram[o + i]; } }
        Ok(b)
    }
    fn fetch_ahead(&mut self, pc: u32) -> Option<[u8; 4]> { self.fetch(pc).ok() }
    fn page_versions(&self) -> &[u32] { if self.versions { &self.ver } else { &[] } }
    fn code_page(&mut self, pc: u32) -> u32 { match self.off(pc, 1) { Some(o) => (o >> 8) as u32, None => u32::MAX } }
    fn block_break(&self) -> bool { self.brk }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; (self.0 >> 16) as u32 }
    fn below(&mut self, n: u32) -> u32 { self.next() % n }
}

fn b_type(op: u32, f3: u32, rs1: u32, rs2: u32, off: i32) -> u32 {
    let o = off as u32;
    op | ((o >> 11) & 1) << 7 | ((o >> 1) & 0xf) << 8 | f3 << 12 | rs1 << 15 | rs2 << 20 | ((o >> 5) & 0x3f) << 25 | ((o >> 12) & 1) << 31
}
fn j_type(rd: u32, off: i32) -> u32 {
    let o = off as u32;
    0x6f | rd << 7 | ((o >> 12) & 0xff) << 12 | ((o >> 11) & 1) << 20 | ((o >> 1) & 0x3ff) << 21 | ((o >> 20) & 1) << 31
}
fn i_type(op: u32, f3: u32, rd: u32, rs1: u32, imm: i32) -> u32 { op | rd << 7 | f3 << 12 | rs1 << 15 | ((imm as u32) & 0xfff) << 20 }
fn r_type(f7: u32, f3: u32, rd: u32, rs1: u32, rs2: u32) -> u32 { 0x33 | rd << 7 | f3 << 12 | rs1 << 15 | rs2 << 20 | f7 << 25 }
fn s_type(f3: u32, rs1: u32, rs2: u32, imm: i32) -> u32 { let i = imm as u32; 0x23 | (i & 0x1f) << 7 | f3 << 12 | rs1 << 15 | rs2 << 20 | ((i >> 5) & 0x7f) << 25 }

/// A Zba, Zbb or Zbs instruction, or `mul`/`div`/`rem`.
fn zb_or_m(r: &mut Rng, rd: u32, rs1: u32, rs2: u32) -> u32 {
    // (funct7, funct3): sh1add-sh3add, andn, orn, xnor, min-maxu, rol, ror, bclr, bext, binv, bset
    const REG: [(u32, u32); 16] = [(0x10, 2), (0x10, 4), (0x10, 6), (0x20, 7), (0x20, 6), (0x20, 4), (5, 4), (5, 5), (5, 6), (5, 7), (0x30, 1), (0x30, 5), (0x24, 1), (0x24, 5), (0x34, 1), (0x14, 1)];
    // (imm, funct3): clz, ctz, cpop, sext.b, sext.h, rev8, orc.b
    const UNARY: [(i32, u32); 7] = [(0x600, 1), (0x601, 1), (0x602, 1), (0x604, 1), (0x605, 1), (0x698, 5), (0x287, 5)];
    // rori, bclri, bexti, binvi, bseti (the shift amount added)
    const IMM: [(i32, u32); 5] = [(0x600, 5), (0x480, 1), (0x480, 5), (0x680, 1), (0x280, 1)];
    match r.below(8) {
        0..=2 => { let (f7, f3) = REG[r.below(16) as usize]; r_type(f7, f3, rd, rs1, rs2) }
        3 => { let (imm, f3) = UNARY[r.below(7) as usize]; i_type(0x13, f3, rd, rs1, imm) }
        4 | 5 => { let (imm, f3) = IMM[r.below(5) as usize]; i_type(0x13, f3, rd, rs1, imm | r.below(32) as i32) }
        6 => r_type(4, 4, rd, rs1, 0),     // zext.h
        _ => r_type(1, r.below(8), rd, rs1, rs2),
    }
}

/// One instruction: mostly well-formed common forms with small offsets (so that loops, loads and
/// stores stay in RAM, the code region included), some device accesses, and a share of random
/// words of every major opcode and random compressed halfwords.
fn insn(r: &mut Rng, out: &mut Vec<u8>) {
    let reg = |r: &mut Rng| 8 + r.below(8);                     // s0-a5 hold RAM or device addresses
    let any = |r: &mut Rng| r.below(32);
    let w = match r.below(24) {
        22 | 23 => { let (rd, a, b) = (any(r), any(r), any(r)); zb_or_m(r, rd, a, b) }
        20 | 21 => {
            // a hardware-loop instruction (Xesploop), with a short body and a small count
            let (m, v, _) = riscv_rv32::decode::HWLP_FORMS[r.below(6) as usize];
            let w = v | (r.next() & !m);
            let w = if m >> 20 == 0 { (w & 0x000f_ffff) | (1 + r.below(24)) << 20 } else { w };
            // esp.lp.count/setup take the count from a register: use one that holds a small value
            if (w >> 12) & 7 == 2 || (w >> 12) & 7 == 4 { (w & !(0x1f << 15)) | (16 + r.below(2)) << 15 } else { w }
        }
        0..=3 => i_type(0x13, r.below(8), any(r), any(r), (r.next() as i32) >> 20 & 0x3ff),
        4 | 5 => 0x33 | any(r) << 7 | r.below(8) << 12 | any(r) << 15 | any(r) << 20 | [0, 0x20, 1][r.below(3) as usize] << 25,
        6 | 7 => i_type(0x03, [0, 1, 2, 4, 5][r.below(5) as usize], 1 + r.below(31), reg(r), (r.below(64) as i32 - 32) * 4),
        8 | 9 => s_type(r.below(3), reg(r), any(r), (r.below(64) as i32 - 32) * 4),
        10 | 11 => b_type(0x63, [0, 1, 4, 5, 6, 7][r.below(6) as usize], any(r), any(r), (r.below(48) as i32 - 24) * 2),
        12 => j_type(r.below(2), (r.below(128) as i32 - 64) * 2),
        13 => {
            // CSR: reads of the counters and scratch, writes of mscratch/mstatus/mtvec/mie, a
            // random CSR number now and then
            let csr = [0x340, 0xb00, 0xb02, 0xc00, 0x300, 0x7e2, 0x342, r.next() & 0xfff][r.below(8) as usize];
            let f3 = [1, 2, 3, 5, 6, 7][r.below(6) as usize];
            let rs1 = if r.below(2) == 0 { 0 } else { any(r) };
            // a write that sets mstatus bits may turn MIE on mid-program, as an ISR's exit does;
            // keep it rarer than the rest so the programs do not spend all their time in traps
            let csr = if csr == 0x300 && rs1 != 0 && f3 & 3 == 1 && r.below(3) != 0 { 0x340 } else { csr };
            0x73 | any(r) << 7 | f3 << 12 | rs1 << 15 | csr << 20
        }
        14 => [0x0000_0073, 0x0010_0073, 0x3020_0073, 0x1050_0073, 0x0000_100f, 0x0ff0_000f][r.below(6) as usize],
        15 => { let c = r.next() as u16; out.extend_from_slice(&c.to_le_bytes()); return; }
        16 => i_type(0x67, 0, r.below(2), reg(r), (r.below(16) as i32) * 4),
        17 => 0x2f | any(r) << 7 | 2 << 12 | reg(r) << 15 | any(r) << 20 | [0x02, 0x03, 0x01, 0x00, 0x04, 0x0c, 0x08, 0x10, 0x18][r.below(9) as usize] << 27,
        _ => {
            let op = [0x03, 0x07, 0x0f, 0x13, 0x17, 0x1b, 0x1f, 0x23, 0x27, 0x2f, 0x33, 0x37, 0x43, 0x53, 0x5f, 0x63, 0x67, 0x6f, 0x73][r.below(19) as usize];
            op | (r.next() & !0x7f)
        }
    };
    out.extend_from_slice(&w.to_le_bytes());
}

/// One instruction of a program that mostly runs: integer code on registers other than s0-a5,
/// which hold data (now and then device or code) addresses, short branches, loads and stores, a
/// few jumps, CSR accesses and environment calls.
fn loop_insn(r: &mut Rng, out: &mut Vec<u8>) {
    let any = |r: &mut Rng| r.below(32);
    let dst = |r: &mut Rng| { let d = r.below(24); if d < 8 { d } else { d + 8 } };
    let base = |r: &mut Rng| 8 + r.below(8);
    let w = match r.below(40) {
        0..=8 => i_type(0x13, [0, 2, 3, 4, 6, 7][r.below(6) as usize], dst(r), any(r), (r.next() as i32) >> 20),
        9..=13 => r_type([0, 0, 0x20][r.below(3) as usize], [0, 5, 1, 2][r.below(4) as usize], dst(r), any(r), any(r)),
        14..=19 => { let (rd, a, b) = (dst(r), any(r), any(r)); zb_or_m(r, rd, a, b) }
        20..=24 => i_type(0x03, [0, 1, 2, 4, 5][r.below(5) as usize], dst(r), base(r), (r.below(64) as i32 - 32) * 4 + r.below(3) as i32),
        25..=28 => s_type(r.below(3), base(r), any(r), (r.below(64) as i32 - 32) * 4 + r.below(3) as i32),
        29..=34 => b_type(0x63, [0, 1, 4, 5, 6, 7][r.below(6) as usize], any(r), any(r), if r.below(3) == 0 { (r.below(64) as i32 + 2) * 4 } else { -(r.below(48) as i32 + 2) * 4 }),
        35 => j_type(r.below(2), (r.below(128) as i32 - 64) * 4),
        36 => [0x37, 0x17][r.below(2) as usize] | dst(r) << 7 | (r.next() & 0xffff_f000),
        37 => 0x73 | dst(r) << 7 | 2 << 12 | [0xc00, 0xb00, 0xb02, 0x340][r.below(4) as usize] << 20,
        38 => [0x0000_000f, 0x0000_0073, 0x0000_100f][r.below(3) as usize],
        _ => i_type(0x67, 0, r.below(2), base(r), (r.below(16) as i32) * 4),
    };
    out.extend_from_slice(&w.to_le_bytes());
}

fn state(c: &Cpu) -> String {
    format!("hwlp=({:x?},{:x?},{:x?},{:x}) pc={:08x} x={:?} f={:?} q={:?} mstatus={:08x} mepc={:08x} mcause={:08x} mtval={:08x} mtvec={:08x} mscratch={:08x} mie={:08x} insns={} retired={} cycles={} waiting={} fcsr={:x} mil={} thresh={} res={:?} pie=({:x},{:x},{:x},{:x}) ext_ill={:x} other={:?}",
        c.hwlp_start, c.hwlp_end, c.hwlp_count, c.hwlp_state, c.pc, c.x, c.f, c.q, c.mstatus, c.mepc, c.mcause, c.mtval, c.mtvec, c.mscratch, c.mie, c.insn_count, c.retired_count, c.cycle_count, c.waiting, c.fcsr, c.mil, c.mintthresh, c.reservation, c.pie_sar, c.pie_sar_byte, c.pie_cfg, c.pie_state, c.ext_ill, { let mut v: Vec<_> = c.csr_other.iter().collect(); v.sort(); v })
}

fn core(kind: u32) -> Cpu {
    match kind { 0 => Cpu::new(), 1 => Cpu::new_rv32imac(), 2 => Cpu::new_esp32p4_rev3(), _ => Cpu::new_esp32s31() }
}

/// Returns the number of `run` calls compared, blocks built and instructions run; `loops` takes
/// the programs of `loop_insn`.
fn one(seed: u64, kind: u32, loops: bool) -> (u64, u64, u64) {
    let mut r = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut bus = TestBus::new();
    // trap handler: skip the trapping instruction (mepc += 4) and return
    let handler = [0x3410_2ff3u32, 0x004f_8f93, 0x341f_9073, 0x3020_0073];
    for (k, w) in handler.iter().enumerate() { bus.ram[0x100 + 4 * k..][..4].copy_from_slice(&w.to_le_bytes()); }
    let mut code = Vec::new();
    while code.len() < CODE_BYTES { if loops { loop_insn(&mut r, &mut code); } else { insn(&mut r, &mut code); } }
    bus.ram[0x200..0x200 + CODE_BYTES].copy_from_slice(&code[..CODE_BYTES]);
    for b in &mut bus.ram[0x200 + CODE_BYTES..] { *b = r.next() as u8; }
    let mut a = core(kind);
    let mut b = core(kind);
    for c in [&mut a, &mut b] {
        c.pc = CODE;
        c.mtvec = HANDLER;
        c.mstatus |= riscv_rv32::state::mstatus::MIE;
        if kind >= 2 { c.mstatus |= riscv_rv32::state::mstatus::FS_INITIAL; c.set_hwlp_state(1); }
    }
    let mut regs = [0u32; 32];
    for (i, x) in regs.iter_mut().enumerate().skip(1) {
        *x = match i { 8..=15 if loops && r.below(8) != 0 => DATA + 4 * r.below(0x200), 16 | 17 => r.below(6), 8..=15 => if r.below(6) == 0 { MMIO + 4 * r.below(4) } else { CODE + 4 * r.below((CODE_BYTES / 4) as u32 + 256) }, 2 => RAM + RAM_SIZE as u32 - 64, _ => r.next() };
    }
    a.x = regs; b.x = regs;
    b.set_jit(false);
    // compile every block on its first whole run (the JIT, where the host has a backend)
    a.blocks.set_jit_hot(1);
    let mut bus_b = bus.clone();
    let mut calls = 0;
    for _ in 0..300 {
        // what the machine does between dispatches: refresh (clear the break), present an
        // interrupt now and then, sometimes move the boundaries
        // (a machine may also dispatch with the break still pending: one instruction runs)
        let brk = bus.brk && r.below(16) == 0;
        bus.brk = brk; bus_b.brk = brk;
        let irq = match r.below(8) { 0 => Some(r.below(32) | (1 + r.below(7)) << 8), 1 => Some(r.below(32) | 7 << 8 | 1 << 16), _ => None };
        a.set_irq(irq); b.set_irq(irq);
        if r.below(40) == 0 { let bloom = if r.below(2) == 0 { 0 } else { emu_core::core::pc_bit(CODE + 2 * r.below(0x800)) }; a.set_boundaries(bloom); b.set_boundaries(bloom); }
        if r.below(60) == 0 && a.pc.wrapping_sub(RAM) as usize >= RAM_SIZE { a.pc = CODE; b.pc = CODE; }
        if a.mtvec != HANDLER && r.below(4) == 0 { a.mtvec = HANDLER; b.mtvec = HANDLER; }
        let budget = if loops { [5, 31, 64, 300, 1000, 5000, 20000][r.below(7) as usize] + r.below(3) } else { [1, 2, 3, 7, 64, 256, 1000][r.below(7) as usize] };
        let ra = a.run(&mut bus, budget);
        let rb = b.run(&mut bus_b, budget);
        calls += 1;
        RUNS.fetch_add(1, Relaxed);
        let (sa, sb) = (state(&a), state(&b));
        if ra != rb || sa != sb || bus.ram != bus_b.ram || bus.regs != bus_b.regs || bus.reads != bus_b.reads || bus.brk != bus_b.brk || bus.last_pc != bus_b.last_pc || bus.accesses != bus_b.accesses {
            panic!("seed {seed} kind {kind} call {calls}: blocks {ra:?} vs interpreter {rb:?}\nblocks      {sa}\ninterpreter {sb}\nram same {} last pc {:08x} vs {:08x} accesses same {}", bus.ram == bus_b.ram, bus.last_pc, bus_b.last_pc, bus.accesses == bus_b.accesses);
        }
        // the interpreter oracle must not have been using blocks
        assert_eq!(b.blocks.builds, 0);
    }
    COMPILED.fetch_add(a.blocks.compiled(), Relaxed);
    (calls, a.blocks.builds, a.insn_count)
}

static COMPILED: AtomicU64 = AtomicU64::new(0);
static RUNS: AtomicU64 = AtomicU64::new(0);

/// Compiled code that overruns its budget may never return: fail the test run instead of hanging.
fn watch_for_hangs() {
    std::thread::spawn(|| {
        use std::io::Write;
        let (mut seen, mut idle) = (0, 0);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let now = RUNS.load(Relaxed);
            (seen, idle) = if now == seen { (now, idle + 1) } else { (now, 0) };
            // past the test harness's output capture
            if idle == 60 { let _ = std::io::stderr().write_all(b"a run call did not return within 60 s\n"); std::process::exit(101); }
        }
    });
}

#[test]
fn blocks_match_the_interpreter_on_random_programs() {
    watch_for_hangs();
    let seeds: u64 = std::env::var("RV_BLOCK_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(400);
    let (mut programs, mut calls, mut blocks, mut insns, mut looped) = (0, 0, 0, 0, 0);
    let only: Option<u64> = std::env::var("RV_BLOCK_SEED").ok().and_then(|s| s.parse().ok());
    for kind in 0..4 {
        for seed in 0..seeds {
            if let Some(o) = only { if seed * 4 + kind as u64 != o { continue; } }
            let (c, b, i) = one(seed * 4 + kind as u64, kind, false);
            programs += 1; calls += c; blocks += b; insns += i;
            let (c, b, i) = one(seed * 4 + kind as u64, kind, true);
            programs += 1; calls += c; blocks += b; looped += i;
        }
    }
    let compiled = COMPILED.load(Relaxed);
    eprintln!("blocks differential: {programs} programs, {calls} run calls compared, {blocks} blocks built, {compiled} compiled, {insns} + {looped} (loops) instructions");
    if riscv_rv32::jit::AVAILABLE && std::env::var_os("RV32_JIT").is_none() { assert!(compiled > programs, "the programs must exercise the JIT"); }
    assert!(blocks > programs, "the programs must exercise blocks");
}

/// A store in one block rewrites the next block of its region, on another version page: going
/// there must run what was stored, not what was compiled.
#[test]
fn a_store_into_the_next_block_of_a_region_takes_effect() {
    for kind in 0..4 {
        let mut bus = TestBus::new();
        let put = |bus: &mut TestBus, at: u32, w: u32| bus.ram[(at - RAM) as usize..][..4].copy_from_slice(&w.to_le_bytes());
        let next = CODE + 0x200;
        // sw t1, 0(s0) ; j next   next: addi ra, ra, 1 (becomes addi ra, ra, 100) ; ebreak
        for (at, w) in [(CODE, s_type(2, 8, 6, 0)), (CODE + 4, j_type(0, 0x1fc)), (next, i_type(0x13, 0, 1, 1, 1)), (next + 4, 0x0010_0073)] { put(&mut bus, at, w); }
        let mut c = core(kind);
        c.pc = CODE; c.mtvec = HANDLER; c.x[8] = next; c.x[6] = i_type(0x13, 0, 1, 1, 100);
        c.blocks.set_jit_hot(1);
        let (used, trap) = c.run(&mut bus, 100);
        assert!(trap.is_some() && used == 4, "{used} {trap:?}");
        assert_eq!(c.x[1], 100, "kind {kind}: the stored instruction ran");
    }
}

/// A block across two version pages, and a store into its second page: from the block itself, and
/// from the block before it in the region. What was stored must run.
#[test]
fn a_store_into_a_blocks_second_page_takes_effect() {
    for kind in 0..4 {
        for (start, target) in [(CODE + 0xf0, CODE + 0x108), (CODE, CODE + 0x308)] {
            let mut bus = TestBus::new();
            let put = |bus: &mut TestBus, at: u32, w: u32| bus.ram[(at - RAM) as usize..][..4].copy_from_slice(&w.to_le_bytes());
            for k in 0..0x100 { put(&mut bus, CODE + 4 * k, 0x13); }                  // nops
            put(&mut bus, start, s_type(2, 8, 6, 0));                                 // sw t1, 0(s0)
            if start == CODE { put(&mut bus, CODE + 4, j_type(0, 0x2ec)); }           // to a block across CODE + 0x300
            put(&mut bus, target, i_type(0x13, 0, 1, 1, 1));                          // addi ra, ra, 1 (becomes addi ra, ra, 100)
            put(&mut bus, target + 8, 0x0010_0073);
            let mut bus_b = bus.clone();
            let (mut a, mut b) = (core(kind), core(kind));
            for c in [&mut a, &mut b] { c.pc = start; c.mtvec = HANDLER; c.x[8] = target; c.x[6] = i_type(0x13, 0, 1, 1, 100); }
            a.blocks.set_jit_hot(1); b.set_jit(false);
            let (ra, rb) = (a.run(&mut bus, 100), b.run(&mut bus_b, 100));
            assert_eq!(b.x[1], 100);
            assert_eq!((ra, state(&a)), (rb, state(&b)), "kind {kind} start {start:08x}");
        }
    }
}

/// A loop inside one region, cut by every budget up to past its end: the run uses its budget and
/// not an instruction more, and leaves what the interpreter leaves.
#[test]
fn a_region_never_runs_past_its_budget() {
    // addi a0, a0, 1 ; addi a1, a1, 2 ; bne a0, a2, -8 ; addi a3, a3, 1 ; ebreak: 10 times round
    let prog = [i_type(0x13, 0, 10, 10, 1), i_type(0x13, 0, 11, 11, 2), b_type(0x63, 1, 10, 12, -8), i_type(0x13, 0, 13, 13, 1), 0x0010_0073];
    for budget in 1..40 {
        let (mut a, mut b) = (core(0), core(0));
        let mut bus = TestBus::new();
        for (k, w) in prog.iter().enumerate() { bus.ram[0x200 + 4 * k..][..4].copy_from_slice(&w.to_le_bytes()); }
        let mut bus_b = bus.clone();
        for c in [&mut a, &mut b] { c.pc = CODE; c.mtvec = HANDLER; c.x[12] = 10; }
        a.blocks.set_jit_hot(1); b.set_jit(false);
        let (ra, rb) = (a.run(&mut bus, budget), b.run(&mut bus_b, budget));
        assert_eq!(ra.0, budget.min(32));
        assert_eq!((ra, state(&a), bus.last_pc), (rb, state(&b), bus_b.last_pc), "budget {budget}");
    }
}

/// A store into the next instruction of its own block: the interpreter fetches the new word,
/// and so must the block (the rest of it is dropped when its page version moves).
#[test]
fn a_store_into_its_own_block_takes_effect_at_once() {
    let mut bus = TestBus::new();
    // lui a0, 0x00500 ; addi a0,a0,0x513 (a0 = 0x00500513 = addi a0, zero, 5) ; sw a0, 4(s0) ; addi a0, zero, 1 ; j .
    let prog = [0x0050_0537u32, 0x5135_0513, 0x00a4_2223, 0x0010_0513, 0x0000_006f];
    for (k, w) in prog.iter().enumerate() { bus.ram[0x200 + 4 * k..][..4].copy_from_slice(&w.to_le_bytes()); }
    let mut c = Cpu::new();
    c.pc = CODE; c.x[8] = CODE + 8;          // s0 + 4 = the `addi a0, zero, 1` after the store
    let (used, trap) = c.run(&mut bus, 4);
    assert_eq!((used, trap), (4, None));
    assert_eq!(c.x[10], 5, "the stored instruction ran, not the stale one");
    assert!(c.blocks.builds >= 2, "the block was rebuilt after the store");
}

/// A bus without code versions keeps the interpreter: no block is ever built.
#[test]
fn a_bus_without_versions_never_builds() {
    let mut bus = TestBus::new();
    bus.versions = false;
    bus.ram[0x200..0x208].copy_from_slice(&[0x93, 0x00, 0x50, 0x00, 0x6f, 0x00, 0x00, 0x00]);
    let mut c = Cpu::new(); c.pc = CODE;
    assert_eq!(c.run(&mut bus, 100), (100, None));
    assert_eq!(c.blocks.builds, 0);
}

/// A trap in the middle of a block: mepc is the faulting instruction and the iterations count
/// the instructions before it plus the faulting one.
#[test]
fn a_fault_mid_block_is_precise() {
    let mut bus = TestBus::new();
    // addi a0,zero,1 ; addi a1,zero,2 ; lw a2, 0(zero) (faults) ; addi a3,zero,3
    let prog = [0x0010_0513u32, 0x0020_0593, 0x0000_2603, 0x0030_0693];
    for (k, w) in prog.iter().enumerate() { bus.ram[0x200 + 4 * k..][..4].copy_from_slice(&w.to_le_bytes()); }
    let mut c = Cpu::new(); c.pc = CODE; c.mtvec = HANDLER;
    let (used, trap) = c.run(&mut bus, 10);
    assert_eq!((used, trap), (3, Some(Trap::Exception(5))));
    assert_eq!((c.mepc, c.pc, c.x[11], c.x[13]), (CODE + 8, HANDLER, 2, 0));
    assert_eq!((c.insn_count, c.retired_count), (3, 2));
}

/// A hardware loop whose body spans a block: the back-edge after the end instruction must be
/// taken from blocks exactly as the interpreter takes it, count and state included.
#[test]
fn a_hardware_loop_runs_its_body_count_times_in_blocks() {
    // esp.lp.setupi 0, count 5, end = pc + 12 (the third body instruction):
    //   addi a0, a0, 1 ; addi a1, a1, 2 ; addi a2, a2, 3 ; then addi a3, zero, 7 ; j .
    let setupi = 0x0000_502bu32 | 5 << 20 | (12 >> 1) << 15;
    let prog = [setupi, 0x0015_0513, 0x0025_8593, 0x0036_0613, 0x0070_0693, 0x0000_006f];
    for jit in [true, false] {
        let mut bus = TestBus::new();
        for (k, w) in prog.iter().enumerate() { bus.ram[0x200 + 4 * k..][..4].copy_from_slice(&w.to_le_bytes()); }
        let mut c = Cpu::new_esp32s31(); c.pc = CODE; c.set_hwlp_state(1); c.set_jit(jit);
        let (used, trap) = c.run(&mut bus, 1 + 3 * 5 + 1);
        assert_eq!((used, trap), (17, None));
        assert_eq!((c.x[10], c.x[11], c.x[12], c.x[13]), (5, 10, 15, 7), "jit {jit}");
        assert_eq!((c.hwlp_count[0], c.pc, c.hwlp_state & 3), (0, CODE + 20, 3), "jit {jit}");
        assert_eq!(c.blocks.builds > 0, jit);
    }
}
