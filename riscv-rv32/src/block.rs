//! Decoded basic blocks (EX265): the RV32 counterpart of the Xtensa block cache (EX001). A
//! block is up to `MAX_LEN` instructions decoded once, read ahead with `Bus::fetch_ahead`, and
//! kept while the bus's code versions (`Bus::page_versions`) of the pages its bytes lie on stay
//! put. Running one skips the per-instruction fetch, decode and interrupt test of `exec::step`;
//! each instruction still goes through `exec::exec_insn`'s body, the interpreter's own execution.
//!
//! Exactness against a loop over `step` (the oracle, `--no-jit`):
//! - The interrupt test is made before a block, with the predicate `step_outcome` applies. Only
//!   a CSR access, `mret` or a trap can change its inputs (mstatus.MIE, the CLIC level and
//!   threshold, mtvec's mode), and each of them ends a block; the SoC changes `irq` only between
//!   `run`s. A core that is waiting, or would take an interrupt, takes the interpreter's step.
//! - Every instruction counts, notes its pc on the bus and retires exactly as `step_outcome`
//!   does, hardware-loop back-edge included (`hwlp::retire`, which leaves the block when it
//!   jumps); a trap mid-block returns at once with the iterations used, `mepc` precise.
//! - `block_break` is tested after every instruction, as `Core::run` does; budget cuts end a
//!   block early; a block never runs past a pc in the machine's boundary bloom (`set_boundaries`
//!   flushes when the bloom changes, and building stops in front of one).
//! - A store can change code, its own block's included: after any instruction that may write,
//!   the block's versions are compared again and the rest of it is dropped if they moved.
//! - Reading ahead has no side effects (`fetch_ahead`), and a block is only built where it says
//!   the fetch succeeds; the first instruction of an unreadable pc is the interpreter's step,
//!   faults and all.
use crate::bus::Bus;
use crate::decode::{decode_for, Comp, Insn, Op};
use crate::exec::{exec_insn_inline, step_outcome, Trap};
use crate::jit::{Code, Ctx, Jit, NoCode, STOP, TRAP};
use crate::state::Cpu;
use emu_core::core::pc_bit;

/// Instructions per block. A block spans at most two version pages (pages are >= 256 bytes and
/// a block at most 128), checked while building.
pub const MAX_LEN: usize = 32;
/// Direct-mapped block table, indexed by halfword pc.
const ENTRIES: usize = 1 << 16;
/// Decoded instructions kept before everything is thrown away and rebuilt on demand. A new block
/// reuses the slot of the one it evicts when that is long enough, so the arena stays below
/// `ENTRIES * MAX_LEN` and this is only a backstop.
const ARENA_CAP: usize = ENTRIES * MAX_LEN;
/// No version: this pc is not cacheable.
const NO_PAGE: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Entry {
    pc: u32,
    start: u32,
    n: u8,
    /// arena slots this entry owns from `start` (>= n)
    cap: u8,
    /// bit k: instruction k may write memory (then the versions are compared again)
    writes: u32,
    /// it ends in a branch or jump, or no terminator at all: when it lands on its own start the
    /// interrupt test has nothing new to decide (`run`'s `again`)
    plain_end: bool,
    vidx: [u32; 2],
    ver: [u32; 2],
}
const EMPTY: Entry = Entry { pc: 1, start: 0, n: 0, cap: 0, writes: 0, plain_end: false, vidx: [0; 2], ver: [0; 2] };

/// The core's decoded-block cache. Empty until used: `Cpu::new` stays cheap.
pub struct Blocks {
    /// `Core::set_jit`: false runs the interpreter only
    pub on: bool,
    entries: Vec<Entry>,
    arena: Vec<Insn>,
    /// the ISA word the arena was decoded for
    isa: u32,
    pub builds: u64,
    pub flushes: u64,
    /// the JIT's state, behind a pointer so that blocks without it are laid out as before it:
    /// none where there is no backend, or with `RV32_JIT=0`
    jit: Option<Box<State>>,
}

/// What the JIT keeps per block table (EX269).
struct State {
    /// whole runs of a block before it is compiled
    hot: u16,
    /// regions compiled
    compiled: u64,
    /// the compiled code, once there is any
    code: Option<Box<Jit>>,
    /// per block entry; empty until the first block is built
    slots: Vec<Slot>,
    /// every compiled block's code by pc: (code, length, code pages, versions), so a block
    /// inside a compiled region is entered there
    regions: std::collections::HashMap<u32, (u32, usize, [u32; 2], [u32; 2])>,
}

#[derive(Clone, Copy)]
struct Slot {
    /// the compiled block's code (`Jit::compile`), `NOT_COMPILED` or `UNCOMPILABLE`
    code: u32,
    /// whole runs of the block not yet compiled, up to `State::hot`
    hits: u16,
}
const NOT_COMPILED: u32 = u32::MAX;
const UNCOMPILABLE: u32 = u32::MAX - 1;
const NO_SLOT: Slot = Slot { code: NOT_COMPILED, hits: 0 };

impl State {
    /// Discard the compiled code and everything that refers to it.
    fn forget(&mut self) {
        if let Some(c) = self.code.as_mut() { c.reset(); }
        self.regions.clear();
        self.slots.fill(NO_SLOT);
    }
}

/// `RV32_JIT` and `RV32_JIT_HOT`, read once (docs/cli.md): the whole runs before a block is
/// compiled, none for no JIT.
fn settings() -> Option<u16> {
    static SETTINGS: std::sync::OnceLock<Option<u16>> = std::sync::OnceLock::new();
    *SETTINGS.get_or_init(|| {
        let on = crate::jit::AVAILABLE && std::env::var_os("RV32_JIT").is_none_or(|v| v != "0");
        on.then(|| std::env::var("RV32_JIT_HOT").ok().and_then(|v| v.parse().ok()).unwrap_or(crate::jit::HOT))
    })
}

impl Default for Blocks {
    fn default() -> Self {
        let jit = settings().map(|hot| Box::new(State { hot, compiled: 0, code: None, slots: Vec::new(), regions: Default::default() }));
        Blocks { on: true, entries: Vec::new(), arena: Vec::new(), isa: 0, builds: 0, flushes: 0, jit }
    }
}

impl Blocks {
    /// Forget every block (a reset, new boundaries, `--no-jit`), and their code.
    pub fn flush(&mut self) {
        if !self.entries.is_empty() { self.entries.iter_mut().for_each(|e| *e = EMPTY); self.flushes += 1; }
        self.arena.clear();
        if let Some(j) = self.jit.as_mut() { j.forget(); }
    }
    /// Compile a block after `runs` whole runs of it (`RV32_JIT_HOT`).
    pub fn set_jit_hot(&mut self, runs: u16) { if let Some(j) = self.jit.as_mut() { j.hot = runs; } }
    /// Regions compiled.
    pub fn compiled(&self) -> u64 { self.jit.as_ref().map_or(0, |j| j.compiled) }
    /// Bytes of compiled code held.
    pub fn code_bytes(&self) -> usize { self.jit.as_ref().and_then(|j| j.code.as_ref()).map_or(0, |c| c.bytes()) }
}

/// Instructions after which the rest of a block may be stale: anything that can store.
#[inline(always)]
pub(crate) fn may_write(op: Op) -> bool {
    use Op::*;
    matches!(op, Sb | Sh | Sw | ScW | AmoSwapW | AmoAddW | AmoXorW | AmoAndW | AmoOrW | AmoMinW | AmoMaxW | AmoMinuW | AmoMaxuW | CmPush | Fsw) || op.is_pie()
}

/// Instructions that end a block: control transfers, and everything that can change what the
/// interrupt test before the next instruction decides (CSR writes, `mret`, `wfi`, traps). A CSR
/// read (`csrrs`/`csrrc` from x0, or with a zero immediate) writes nothing (`exec_insn`) and
/// `read_csr` has no side effects, so a delay loop that polls the cycle counter stays one block.
#[inline(always)]
pub(crate) fn ends_block(i: &Insn) -> bool {
    use Op::*;
    match i.op {
        Csrrs | Csrrc | Csrrsi | Csrrci => i.rs1 != 0,
        Jal | Jalr | Beq | Bne | Blt | Bge | Bltu | Bgeu | Ecall | Ebreak | Mret | Wfi | FenceI | Illegal
        | Csrrw | Csrrwi | CmPopret | CmPopretz => true,
        _ => false,
    }
}

impl Cpu {
    /// Whether `step_outcome` would do something other than execute the instruction at pc:
    /// sleep, or take the interrupt the SoC presents.
    #[inline(always)]
    fn irq_acts(&self) -> bool {
        match self.irq {
            None => false,
            Some(line) => self.mie_enabled() && (!self.clic || (line >> crate::exec::CLIC_IRQ_LEVEL_SHIFT) & 0xff > self.mil.max(self.mintthresh)),
        }
    }
}

/// Look up, or build, the block at `pc`; `None` when there is none to run (the caller steps).
#[inline(always)]
fn lookup<B: Bus>(cpu: &mut Cpu, bus: &mut B, pc: u32) -> Option<Entry> {
    if cpu.blocks.entries.is_empty() { cpu.blocks.entries = vec![EMPTY; ENTRIES]; }
    let ei = (pc >> 1) as usize & (ENTRIES - 1);
    let e = cpu.blocks.entries[ei];
    if e.pc == pc {
        let pv = bus.page_versions();
        if pv.get(e.vidx[0] as usize) == Some(&e.ver[0]) && pv.get(e.vidx[1] as usize) == Some(&e.ver[1]) { return Some(e); }
    }
    build(cpu, bus, pc, ei)
}

/// A decoded block before it is stored: what `build` keeps, and what the JIT's regions use for
/// the blocks around a hot one.
pub(crate) struct Decoded {
    pub insns: [Insn; MAX_LEN],
    pub n: usize,
    pub writes: u32,
    pub plain_end: bool,
    pub vidx: [u32; 2],
    pub ver: [u32; 2],
}

/// Decode the block at `pc`, as `build` stores it: up to `MAX_LEN` instructions, at most two
/// version pages, in front of a boundary, through its terminator. The pages are watched before
/// their versions are read (EX110). `None` where there is no block to run.
pub(crate) fn decode_block<B: Bus>(cpu: &Cpu, bus: &mut B, pc: u32) -> Option<Decoded> {
    let isa = cpu.isa();
    let v0 = bus.code_page(pc);
    if v0 == NO_PAGE { return None; }
    let mut v1 = v0;
    const NOP: Insn = Insn { op: Op::Illegal, rd: 0, rs1: 0, rs2: 0, imm: 0, len: 4, comp: Comp::None, raw: 0 };
    let mut d = Decoded { insns: [NOP; MAX_LEN], n: 0, writes: 0, plain_end: true, vidx: [v0, v0], ver: [0; 2] };
    let mut at = pc;
    for k in 0..MAX_LEN {
        if k > 0 && cpu.boundary_bloom & pc_bit(at) != 0 { break; }
        let Some(bytes) = bus.fetch_ahead(at) else { break; };
        let i = decode_for(isa, at, bytes);
        let last = at.wrapping_add(i.len as u32 - 1);
        // both ends of the instruction must lie on the block's (at most two) pages
        let (pa, pb) = (bus.code_page(at), bus.code_page(last));
        if pa == NO_PAGE || pb == NO_PAGE { break; }
        let mut fits = true;
        for p in [pa, pb] {
            if p != v0 && p != v1 { if v1 == v0 { v1 = p; } else { fits = false; } }
        }
        if !fits { break; }
        if may_write(i.op) { d.writes |= 1 << k; }
        d.insns[k] = i;
        d.n = k + 1;
        if ends_block(&i) {
            d.plain_end = matches!(i.op, Op::Jal | Op::Jalr | Op::Beq | Op::Bne | Op::Blt | Op::Bge | Op::Bltu | Op::Bgeu | Op::CmPopret | Op::CmPopretz);
            break;
        }
        at = at.wrapping_add(i.len as u32);
    }
    if d.n == 0 { return None; }
    // EX110's order: watch the pages, then read the versions the block will compare.
    bus.note_code_page(v0);
    if v1 != v0 { bus.note_code_page(v1); }
    let pv = bus.page_versions();
    let (Some(&ver0), Some(&ver1)) = (pv.get(v0 as usize), pv.get(v1 as usize)) else { return None; };
    d.vidx = [v0, v1];
    d.ver = [ver0, ver1];
    Some(d)
}

#[cold]
#[inline(never)]
fn build<B: Bus>(cpu: &mut Cpu, bus: &mut B, pc: u32, ei: usize) -> Option<Entry> {
    let isa = cpu.isa();
    let b = &mut cpu.blocks;
    if b.isa != isa || b.arena.len() + MAX_LEN > ARENA_CAP { b.flush(); b.isa = isa; }
    let d = decode_block(cpu, bus, pc)?;
    let b = &mut cpu.blocks;
    let n = d.n;
    // the evicted block's slots, if they are enough; nothing else refers to them
    let old = b.entries[ei];
    let (start, cap) = if old.pc != EMPTY.pc && old.cap as usize >= n { (old.start as usize, old.cap) } else {
        let start = b.arena.len();
        b.arena.extend_from_slice(&d.insns[..n]);
        (start, n as u8)
    };
    b.arena[start..start + n].copy_from_slice(&d.insns[..n]);
    let e = Entry { pc, start: start as u32, n: n as u8, cap, writes: d.writes, plain_end: d.plain_end, vidx: d.vidx, ver: d.ver };
    b.entries[ei] = e;
    if let Some(j) = b.jit.as_mut() {
        if j.slots.is_empty() { j.slots = vec![NO_SLOT; ENTRIES]; }
        j.slots[ei] = NO_SLOT;
    }
    b.builds += 1;
    Some(e)
}

/// The compiled code of the block in entry `ei`, compiling its region once the block is hot.
#[inline(always)]
fn jit_code<B: Bus>(cpu: &mut Cpu, bus: &mut B, ei: usize) -> Option<Code> {
    let b = &mut cpu.blocks;
    let j = b.jit.as_deref_mut()?;
    let s = j.slots.get_mut(ei)?;
    if s.code >= UNCOMPILABLE {
        if s.code == UNCOMPILABLE { return None; }
        s.hits = s.hits.saturating_add(1);
        if s.hits < j.hot { return None; }
        // inside a region compiled already, from the same bytes (versions) and as long
        let e = &b.entries[ei];
        match j.regions.get(&e.pc) {
            Some(&(code, n, vidx, ver)) if n == e.n as usize && vidx == e.vidx && ver == e.ver => s.code = code,
            _ => return jit_compile(cpu, bus, ei),
        }
    }
    let code = s.code;
    j.code.as_ref().map(|c| c.entry(code))
}

#[cold]
#[inline(never)]
fn jit_compile<B: Bus>(cpu: &mut Cpu, bus: &mut B, ei: usize) -> Option<Code> {
    let e = cpu.blocks.entries[ei];
    let insns = cpu.blocks.arena[e.start as usize..e.start as usize + e.n as usize].to_vec();
    let region = crate::jit::region(cpu, bus, e.pc, &insns, e.vidx, e.ver);
    let j = cpu.blocks.jit.as_deref_mut()?;
    if j.code.is_none() {
        j.code = Jit::new();
        // the host gives no executable memory: blocks stay interpreted
        if j.code.is_none() { cpu.blocks.jit = None; return None; }
    }
    let mut codes = j.code.as_mut()?.compile::<B>(&region);
    if codes == Err(NoCode::Full) {
        // start again, every block's code with it
        j.forget();
        codes = j.code.as_mut()?.compile::<B>(&region);
    }
    let Ok(codes) = codes else { j.slots[ei].code = UNCOMPILABLE; return None; };
    for ((pc, n, vidx, ver), &code) in region.members().zip(&codes) { j.regions.insert(pc, (code, n, vidx, ver)); }
    j.slots[ei].code = codes[0];
    j.compiled += 1;
    j.code.as_ref().map(|c| c.entry(codes[0]))
}

/// One instruction through the interpreter, with `Core::run`'s stop tests.
#[inline(always)]
fn step_one<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> Result<bool, Trap> {
    step_outcome(cpu, bus).result()?;
    Ok(bus.block_break() || (cpu.boundary_bloom != 0 && cpu.boundary_bloom & pc_bit(cpu.pc) != 0))
}

/// `Core::run` with blocks: the same iterations, state and stops as a loop over `step`.
pub fn run<B: Bus>(cpu: &mut Cpu, bus: &mut B, budget: u32) -> (u32, Option<Trap>) {
    let mut used = 0u32;
    // A block that ran whole, stored nothing and branched back to its own start is still valid
    // and the interrupt test still passes: nothing in it could change either.
    let mut again: Option<Entry> = None;
    while used < budget {
        let e = if again.is_some() { again.take() } else if cpu.waiting || cpu.irq_acts() { None } else { let pc = cpu.pc; lookup(cpu, bus, pc) };
        let Some(e) = e else {
            used += 1;
            match step_one(cpu, bus) {
                Err(t) => return (used, Some(t)),
                Ok(true) => return (used, None),
                Ok(false) => continue,
            }
        };
        let n = (e.n as u32).min(budget - used);
        // a whole block, no hardware loop counting, no break pending: its compiled code
        if cpu.blocks.jit.is_some() && n == e.n as u32 && cpu.hwlp_count[0] | cpu.hwlp_count[1] == 0 && !bus.block_break() {
            if let Some(code) = jit_code(cpu, bus, (e.pc >> 1) as usize & (ENTRIES - 1)) {
                let mut ctx = Ctx::new(bus);
                // SAFETY: the code is this core's (slots forget theirs whenever it is reset) and
                // the entry block's versions were just checked by `lookup`. Nothing compiles or
                // discards code while it runs: the helpers never reach `build` or `flush`. Code is
                // made for one bus: a core always runs on the same one, whose versions never shrink.
                let r = unsafe { Jit::call(code, cpu, bus, &mut ctx, budget - used) };
                let (done, status) = (r as u32, (r >> 32) as u32);
                bus.note_pc(ctx.last_pc);
                match status {
                    TRAP => return (used + done, ctx.trap),
                    STOP => return (used + done, None),
                    _ => {}
                }
                used += done;
                if cpu.boundary_bloom != 0 && cpu.boundary_bloom & pc_bit(cpu.pc) != 0 { return (used, None); }
                continue;
            }
        }
        let mut done = 0;
        while done < n {
            let i = cpu.blocks.arena[(e.start + done) as usize];
            let pc = cpu.pc;
            bus.note_pc(pc);
            cpu.insn_count += 1; cpu.cycle_count += 1;
            done += 1;
            if let Err(t) = exec_insn_inline(cpu, bus, &i, pc) { return (used + done, Some(t)); }
            cpu.retired_count += 1;
            // a hardware loop's back-edge after its last instruction, as `step_outcome` takes it;
            // the rest of the block is then not where execution goes
            let next = cpu.pc;
            crate::hwlp::retire(cpu, pc);
            let looped = cpu.pc != next;
            if bus.block_break() { return (used + done, None); }
            if looped { break; }
            if e.writes & (1 << (done - 1)) != 0 && done < n {
                let pv = bus.page_versions();
                // the rest of this block may be stale: carry on from a fresh lookup
                if pv.get(e.vidx[0] as usize) != Some(&e.ver[0]) || pv.get(e.vidx[1] as usize) != Some(&e.ver[1]) { break; }
            }
        }
        used += done;
        if cpu.boundary_bloom != 0 && cpu.boundary_bloom & pc_bit(cpu.pc) != 0 { return (used, None); }
        if done == e.n as u32 && e.writes == 0 && e.plain_end && cpu.pc == e.pc { again = Some(e); }
    }
    (budget, None)
}
