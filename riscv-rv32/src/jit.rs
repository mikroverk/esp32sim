//! Block JIT: decoded blocks (`block.rs`) compiled to host code, in regions. It runs where blocks
//! run, so on a bus that versions its code: the C3's, C6's, P4's and S31's (EX270). A region is a
//! hot block and the blocks its branches, jumps and fall-throughs reach on the same 4 KiB page;
//! compiled code goes from one to the next itself. Each block is the block loop of `block::run`
//! unrolled, with the same exit rules, so the block interpreter (and below it `exec::step`) stays
//! the oracle and the two must leave bit-identical machine state:
//!
//! - The dispatch loop enters a region only for a whole block, with no hardware loop counting
//!   and no break pending. Inside, going to the next block re-checks what the loop's `lookup`
//!   would: that the block fits the budget whole (else the loop runs it partly) and that its code
//!   versions have not moved. Boundaries (`boundary_bloom`) are not entered: a region only goes
//!   to blocks outside the bloom. The interrupt test is not repeated: its inputs only change
//!   through the instructions that end a region (CSR writes, `mret`, `wfi`, traps), and the SoC
//!   moves `irq` only between runs.
//! - Integer, multiply/divide, bit-manipulation and control-transfer instructions are host code
//!   (`mulhsu`, `cpop` and `orc.b` excepted).
//!   Loads and stores call `h_load`/`h_store`, which make the interpreter's bus access and trap
//!   exactly; everything else calls `h_exec` (`exec::exec_insn`). The region ends
//!   after a helper as the loop would end it: on a trap, on `block_break`, after a hardware-loop
//!   instruction, and (checked in the compiled code) after a store that moved its block's code
//!   versions with instructions left in the block.
//! - Counters (`insn_count`, `cycle_count`, `retired_count`) are added per block, before
//!   `h_exec` (which may read them) and at every exit. `note_pc` is made before every access (in
//!   the helpers) and for the last instruction at exit (`Ctx::last_pc`).
//!
//! Backends: AArch64 on macOS and Linux, WebAssembly with the `wasm-jit-host` feature (the page
//! compiles the modules). Elsewhere [`AVAILABLE`] is false.
// Without a backend (x86-64, wasm without a page to compile for) only the shared front end is
// built, and nothing calls it.
#![cfg_attr(not(any(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux")), all(target_arch = "wasm32", feature = "wasm-jit-host"))), allow(dead_code))]
use crate::block::{decode_block, ends_block, Decoded};
use crate::bus::Bus;
use crate::decode::{Insn, Op};
use crate::state::{exc, Cpu};
use emu_core::core::pc_bit;
use emu_core::Trap;

/// Why `Jit::compile` made no code: no room left (the caller discards all of it and tries again),
/// or this region cannot be compiled, or the page refused its module (its block stays interpreted).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoCode { Full, Refused }

/// How compiled code ended (the high word of its result; the low word is the instructions it
/// used, a trapping one included).
pub const END: u32 = 0;
pub const TRAP: u32 = 1;
/// `block_break` after the last instruction used: the dispatch ends.
pub const STOP: u32 = 2;
/// The rest of the block must not run (stale code, a hardware loop): look up `pc` again.
pub const LEAVE: u32 = 3;

/// Whole runs of a block before its region is compiled (`Blocks::set_jit_hot`); compiling a wasm
/// module costs the page far more than a native region.
pub const HOT: u16 = if cfg!(target_arch = "wasm32") { 256 } else { 32 };
/// Blocks per region.
const MAX_BLOCKS: usize = 24;

/// Compiled code to enter (`Jit::entry`): its address, or in wasm its module and block.
#[derive(Clone, Copy)]
pub struct Code(*const u8);

/// What compiled code and its helpers share for one call.
#[repr(C)]
pub struct Ctx {
    /// the pc of the last instruction used (`note_pc` at exit)
    pub last_pc: u32,
    /// the bus's code versions (`Bus::page_versions`)
    pub pv: *const u32,
    pub trap: Option<Trap>,
}

impl Ctx {
    pub fn new<B: Bus>(bus: &B) -> Ctx { Ctx { last_pc: 0, pv: bus.page_versions().as_ptr(), trap: None } }
}

// ---------------------------------------------------------------- helpers called from compiled code

/// Any instruction, through the interpreter: `block::run`'s body for one instruction after its
/// counting, which the compiled code has done.
extern "C" fn h_exec<B: Bus>(cpu: *mut Cpu, bus: *mut B, ctx: *mut Ctx, i: *const Insn, pc: u32) -> u32 {
    // SAFETY: compiled code passes the exclusive pointers `run` received, and the instruction
    // table the region was compiled with, which lives as long as its code.
    let (cpu, bus, ctx, i) = unsafe { (&mut *cpu, &mut *bus, &mut *ctx, &*i) };
    bus.note_pc(pc);
    if let Err(t) = crate::exec::exec_insn(cpu, bus, i, pc) { ctx.trap = Some(t); return TRAP; }
    cpu.retired_count += 1;
    let next = cpu.pc;
    crate::hwlp::retire(cpu, pc);
    if bus.block_break() { return STOP; }
    if cpu.pc != next || i.op.is_hwlp() || cpu.hwlp_count[0] | cpu.hwlp_count[1] != 0 { return LEAVE; }
    END
}

/// Loads (`kind`: 0 lb, 1 lh, 2 lw, 3 lbu, 4 lhu), as `exec`'s `ld!`: the value in the low word,
/// the status in the high word. Retires the instruction unless it traps.
extern "C" fn h_load<B: Bus>(cpu: *mut Cpu, bus: *mut B, ctx: *mut Ctx, va: u32, pc: u32, kind: u32) -> u64 {
    // SAFETY: as `h_exec`.
    let (cpu, bus, ctx) = unsafe { (&mut *cpu, &mut *bus, &mut *ctx) };
    bus.note_pc(pc);
    let r = match kind {
        0 => bus.read8(va).map(|v| v as i8 as i32 as u32),
        1 => bus.read16(va).map(|v| v as i16 as i32 as u32),
        2 => bus.read32(va),
        3 => bus.read8(va).map(|v| v as u32),
        _ => bus.read16(va).map(|v| v as u32),
    };
    match r {
        Ok(v) => { cpu.retired_count += 1; v as u64 | if bus.block_break() { (STOP as u64) << 32 } else { 0 } }
        Err(_) => {
            let c = exc::LOAD_ACCESS_FAULT;
            cpu.trap(c, va, pc); ctx.trap = Some(Trap::Exception(c)); (TRAP as u64) << 32
        }
    }
}

/// Stores (`kind`: 0 sb, 1 sh, 2 sw), as `exec`'s `st!`; returns the status.
extern "C" fn h_store<B: Bus>(cpu: *mut Cpu, bus: *mut B, ctx: *mut Ctx, va: u32, v: u32, pc: u32, kind: u32) -> u32 {
    // SAFETY: as `h_exec`.
    let (cpu, bus, ctx) = unsafe { (&mut *cpu, &mut *bus, &mut *ctx) };
    bus.note_pc(pc);
    let r = match kind { 0 => bus.write8(va, v as u8), 1 => bus.write16(va, v as u16), _ => bus.write32(va, v) };
    if r.is_err() {
        let c = exc::STORE_ACCESS_FAULT;
        cpu.trap(c, va, pc); ctx.trap = Some(Trap::Exception(c)); return TRAP;
    }
    cpu.retired_count += 1;
    if bus.block_break() { STOP } else { END }
}

// ---------------------------------------------------------------- regions

/// How an instruction is compiled.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Native, Load(u32), Store(u32), Exec }

fn kind(op: Op) -> Kind {
    use Op::*;
    match op {
        Lb => Kind::Load(0), Lh => Kind::Load(1), Lw => Kind::Load(2), Lbu => Kind::Load(3), Lhu => Kind::Load(4),
        Sb => Kind::Store(0), Sh => Kind::Store(1), Sw => Kind::Store(2),
        Lui | Auipc | Jal | Jalr | Beq | Bne | Blt | Bge | Bltu | Bgeu
        | Addi | Slti | Sltiu | Xori | Ori | Andi | Slli | Srli | Srai
        | Add | Sub | Sll | Slt | Sltu | Xor | Srl | Sra | Or | And
        | Mul | Mulh | Mulhu | Div | Divu | Rem | Remu | Fence
        | Sh1add | Sh2add | Sh3add | Andn | Orn | Xnor | Clz | Ctz | Max | Maxu | Min | Minu
        | SextB | SextH | ZextH | Rol | Ror | Rori | Rev8
        | Bclr | Bclri | Bext | Bexti | Binv | Binvi | Bset | Bseti => Kind::Native,
        _ => Kind::Exec,
    }
}

/// Where execution goes after a block.
#[derive(Clone, Copy)]
enum Succ {
    /// the last instruction set the pc (`jalr`, the interpreter's instructions): leave
    Gone,
    /// on to `pc` (a block cut short, or `jal`)
    To(u32),
    /// a conditional branch on (rs1, rs2)
    Branch { op: Op, rs1: u8, rs2: u8, taken: u32, not: u32 },
}

/// A block of a region.
struct RBlock { pc: u32, insns: Vec<Insn>, vidx: [u32; 2], ver: [u32; 2], succ: Succ }

impl RBlock {
    fn new(pc: u32, insns: &[Insn], vidx: [u32; 2], ver: [u32; 2]) -> RBlock {
        let last = insns[insns.len() - 1];
        let next = insns.iter().fold(pc, |p, i| p.wrapping_add(i.len as u32));
        let succ = match last.op {
            Op::Jal => Succ::To(last.imm as u32),
            Op::Beq | Op::Bne | Op::Blt | Op::Bge | Op::Bltu | Op::Bgeu => Succ::Branch { op: last.op, rs1: last.rs1, rs2: last.rs2, taken: last.imm as u32, not: next },
            _ if kind(last.op) == Kind::Exec && ends_block(&last) => Succ::Gone,
            Op::Jalr => Succ::Gone,
            _ => Succ::To(next),
        };
        RBlock { pc, insns: insns.to_vec(), vidx, ver, succ }
    }
    fn targets(&self) -> Vec<u32> {
        match self.succ { Succ::Gone => vec![], Succ::To(t) => vec![t], Succ::Branch { taken, not, .. } => vec![not, taken] }
    }
    fn last_pc(&self) -> u32 { self.insns[..self.insns.len() - 1].iter().fold(self.pc, |p, i| p.wrapping_add(i.len as u32)) }
}

/// A hot block and the blocks reachable from it on its 4 KiB page.
pub struct Region { blocks: Vec<RBlock>, bloom: u64 }

impl Region {
    /// Each block's (pc, length, code pages, versions), in entry order.
    pub(crate) fn members(&self) -> impl Iterator<Item = (u32, usize, [u32; 2], [u32; 2])> + '_ {
        self.blocks.iter().map(|b| (b.pc, b.insns.len(), b.vidx, b.ver))
    }
    fn index(&self, pc: u32) -> Option<usize> { self.blocks.iter().position(|b| b.pc == pc) }
    /// The block a transition to `pc` may enter: none at a boundary (the dispatch loop stops
    /// there), the region's own entry included, which a run may start on.
    fn target(&self, pc: u32) -> Option<usize> { if self.bloom & pc_bit(pc) != 0 { None } else { self.index(pc) } }
}

/// Build the region of the block at `pc`: its instructions, code pages and versions, and the
/// blocks it reaches on its 4 KiB page.
pub(crate) fn region<B: Bus>(cpu: &Cpu, bus: &mut B, pc: u32, insns: &[Insn], vidx: [u32; 2], ver: [u32; 2]) -> Region {
    let mut r = Region { blocks: vec![RBlock::new(pc, insns, vidx, ver)], bloom: cpu.boundary_bloom };
    let mut work = r.blocks[0].targets();
    while let Some(t) = work.pop() {
        if r.blocks.len() >= MAX_BLOCKS { break; }
        if t >> 12 != pc >> 12 || cpu.boundary_bloom & pc_bit(t) != 0 || r.index(t).is_some() { continue; }
        let Some(Decoded { insns, n, vidx, ver, .. }) = decode_block(cpu, bus, t) else { continue };
        let b = RBlock::new(t, &insns[..n], vidx, ver);
        work.extend(b.targets());
        r.blocks.push(b);
    }
    r
}

#[cfg(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux")))]
pub use a64::{Jit, AVAILABLE};
#[cfg(all(target_arch = "wasm32", feature = "wasm-jit-host"))]
pub use wasm::{Jit, AVAILABLE};

#[cfg(not(any(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux")), all(target_arch = "wasm32", feature = "wasm-jit-host"))))]
mod none {
    use super::*;
    pub const AVAILABLE: bool = false;
    /// No backend on this host: nothing is ever compiled.
    pub struct Jit;
    impl Jit {
        pub fn new() -> Option<Box<Jit>> { None }
        pub fn compile<B: Bus>(&mut self, _r: &Region) -> Result<Vec<u32>, NoCode> { Err(NoCode::Refused) }
        pub fn reset(&mut self) {}
        pub fn bytes(&self) -> usize { 0 }
        pub fn entry(&self, code: u32) -> Code { Code(code as usize as *const u8) }
        /// # Safety
        /// Never called: no code exists.
        pub unsafe fn call<B: Bus>(_code: Code, _cpu: &mut Cpu, _bus: &mut B, _ctx: &mut Ctx, _budget: u32) -> u64 { unreachable!() }
    }
}
#[cfg(not(any(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux")), all(target_arch = "wasm32", feature = "wasm-jit-host"))))]
pub use none::{Jit, AVAILABLE};

/// Counts not yet added to the Cpu: instructions (`insn_count`, `cycle_count`) and those retired
/// by host code (helpers count their own).
#[derive(Clone, Copy, Default)]
#[allow(dead_code)]
struct Pending { insns: u32, retired: u32 }

#[cfg(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux")))]
mod a64 {
    use super::*;
    use crate::block::may_write;
    use emu_core::jit_a64::{Asm, Cond, Label, Reg, SP, ZR};
    use std::ffi::{c_int, c_void};
    use std::mem::offset_of;

    pub const AVAILABLE: bool = true;

    /// The helper table compiled code calls through (field order = the offsets below).
    #[repr(C)]
    pub struct Helpers { exec: *const (), load: *const (), store: *const () }
    const H_EXEC: u32 = 0;
    const H_LOAD: u32 = 8;
    const H_STORE: u32 = 16;
    impl Helpers {
        fn new<B: Bus>() -> Helpers { Helpers { exec: h_exec::<B> as *const (), load: h_load::<B> as *const (), store: h_store::<B> as *const () } }
    }

    extern "C" {
        fn mmap(addr: *mut c_void, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> c_int;
        #[cfg(target_os = "macos")] fn pthread_jit_write_protect_np(enabled: c_int);
        #[cfg(target_os = "macos")] fn sys_icache_invalidate(start: *mut c_void, len: usize);
        #[cfg(target_os = "linux")] fn __clear_cache(start: *mut c_void, end: *mut c_void);
    }

    /// Executable memory for one core's regions, and the instruction tables `h_exec` reads.
    pub struct Jit { base: *mut u8, size: usize, used: usize, tables: Vec<Box<[Insn]>> }
    // SAFETY: the mapping and the tables are owned by this value and only reached through it.
    unsafe impl Send for Jit {}

    impl Drop for Jit {
        // SAFETY: the mapping was created by `new` with this size and is not used afterwards.
        fn drop(&mut self) { unsafe { munmap(self.base as *mut c_void, self.size); } }
    }

    const CODE_BYTES: usize = 32 << 20;

    impl Jit {
        pub fn new() -> Option<Box<Jit>> {
            #[cfg(target_os = "macos")] let flags = 0x0002 | 0x1000 | 0x0800;   // PRIVATE | ANON | JIT
            #[cfg(target_os = "linux")] let flags = 0x0002 | 0x0020;            // PRIVATE | ANONYMOUS
            // SAFETY: a fresh anonymous mapping; failure is checked before the pointer is kept.
            let p = unsafe { mmap(std::ptr::null_mut(), CODE_BYTES, 7, flags, -1, 0) };
            if p as isize == -1 || p.is_null() { return None; }
            Some(Box::new(Jit { base: p as *mut u8, size: CODE_BYTES, used: 0, tables: Vec::new() }))
        }
        /// Forget all code (the caller forgets every block that refers to it).
        pub fn reset(&mut self) { self.used = 0; self.tables.clear(); }
        pub fn bytes(&self) -> usize { self.used }

        fn write(&mut self, words: &[u32]) -> Option<u32> {
            let bytes = words.len() * 4;
            if self.used + bytes > self.size { return None; }
            let off = self.used;
            // SAFETY: the range is inside the mapping (checked above) and nothing runs code
            // from it while it is written.
            unsafe {
                #[cfg(target_os = "macos")] pthread_jit_write_protect_np(0);
                std::ptr::copy_nonoverlapping(words.as_ptr() as *const u8, self.base.add(off), bytes);
                #[cfg(target_os = "macos")] pthread_jit_write_protect_np(1);
                #[cfg(target_os = "macos")] sys_icache_invalidate(self.base.add(off) as *mut c_void, bytes);
                #[cfg(target_os = "linux")] __clear_cache(self.base.add(off) as *mut c_void, self.base.add(off + bytes) as *mut c_void);
            }
            self.used += bytes;
            Some(off as u32)
        }

        /// The block's code `compile` returned as `code`.
        pub fn entry(&self, code: u32) -> Code { Code(self.base.wrapping_add(code as usize)) }

        /// Run the region at `code` with `budget` instructions left (at least its entry block's).
        /// # Safety
        /// `code` must come from `entry` of a live `Jit` since its last `reset`, for a block that
        /// is still the one in memory (the caller's version check).
        pub unsafe fn call<B: Bus>(code: Code, cpu: &mut Cpu, bus: &mut B, ctx: &mut Ctx, budget: u32) -> u64 {
            let h = Helpers::new::<B>();
            type F<B> = unsafe extern "C" fn(*mut Cpu, *mut B, *const Helpers, *mut Ctx, u32) -> u64;
            let f: F<B> = std::mem::transmute::<*const u8, F<B>>(code.0);
            f(cpu, bus, &h, ctx, budget)
        }

        /// Compile a region: the code of each of its blocks as an entry (`Region::members`
        /// order). Refused when the layout does not fit the encodings used.
        pub fn compile<B: Bus>(&mut self, r: &Region) -> Result<Vec<u32>, NoCode> {
            let tables: Vec<Box<[Insn]>> = r.blocks.iter().map(|b| b.insns.clone().into_boxed_slice()).collect();
            let (words, entries) = emit(r, &tables).ok_or(NoCode::Refused)?;
            let off = self.write(&words).ok_or(NoCode::Full)?;
            self.tables.extend(tables);
            Ok(entries.into_iter().map(|w| off + 4 * w as u32).collect())
        }
    }

    // register plan (callee-saved, kept across helper calls)
    const CPU: Reg = 19;
    const BUS: Reg = 20;
    const HLP: Reg = 21;
    const CTX: Reg = 22;
    /// instructions the region has used before the current block
    const DONE: Reg = 23;
    /// budget left at the current block's start
    const REM: Reg = 24;
    /// the code versions (`Ctx::pv`)
    const PV: Reg = 25;
    /// a helper's status, from its return to the exit
    const STATUS: Reg = 11;

    fn xo(r: u8) -> u32 { offset_of!(Cpu, x) as u32 + 4 * r as u32 }
    fn ld(a: &mut Asm, d: Reg, r: u8) { if r == 0 { a.mov(d, ZR) } else { a.ldr(d, CPU, xo(r)) } }
    fn st(a: &mut Asm, s: Reg, r: u8) { if r != 0 { a.str(s, CPU, xo(r)) } }
    fn raw(a: &mut Asm, w: u32) { a.code.push(w); }
    fn orn(a: &mut Asm, d: Reg, n: Reg, m: Reg) { raw(a, 0x2a20_0000 | m << 16 | n << 5 | d); }
    fn eon(a: &mut Asm, d: Reg, n: Reg, m: Reg) { raw(a, 0x4a20_0000 | m << 16 | n << 5 | d); }
    fn rbit(a: &mut Asm, d: Reg, n: Reg) { raw(a, 0x5ac0_0000 | n << 5 | d); }
    fn rev(a: &mut Asm, d: Reg, n: Reg) { raw(a, 0x5ac0_0800 | n << 5 | d); }
    fn rorv(a: &mut Asm, d: Reg, n: Reg, m: Reg) { raw(a, 0x1ac0_2c00 | m << 16 | n << 5 | d); }
    fn ror_imm(a: &mut Asm, d: Reg, n: Reg, sh: u32) { raw(a, 0x1380_0000 | n << 16 | (sh & 31) << 10 | n << 5 | d); }
    fn set_pc(a: &mut Asm, v: u32) { a.mov32(10, v); a.str(10, CPU, offset_of!(Cpu, pc) as u32); }
    fn set_last(a: &mut Asm, v: u32) { a.mov32(10, v); a.str(10, CTX, offset_of!(Ctx, last_pc) as u32); }

    fn add_counter(a: &mut Asm, off: u32, d: u32) {
        if d == 0 { return; }
        a.ldr_x(9, CPU, off); a.add_imm_x(9, 9, d); a.str_x(9, CPU, off);
    }
    fn flush(a: &mut Asm, p: &mut Pending) {
        add_counter(a, offset_of!(Cpu, insn_count) as u32, p.insns);
        add_counter(a, offset_of!(Cpu, cycle_count) as u32, p.insns);
        add_counter(a, offset_of!(Cpu, retired_count) as u32, p.retired);
        *p = Pending::default();
    }
    fn call_helper(a: &mut Asm, off: u32) { a.ldr_x(9, HLP, off); a.blr(9); }

    /// An early exit after instruction `k` of a block: the status is in STATUS, or `status`.
    struct Exit { at: Label, k: u32, pc: u32, pend: Pending, set_pc: Option<u32>, status: Option<u32> }

    /// Leave the region with END: the pc, the last instruction's pc, DONE.
    fn leave(a: &mut Asm, pc: u32, last: u32, epilogue: Label) {
        set_pc(a, pc); set_last(a, last); a.mov(0, DONE); a.b(epilogue);
    }

    /// Branch to `fail` unless block `b`'s code versions still hold.
    fn check_versions(a: &mut Asm, b: &RBlock, fail: Label) {
        let n = if b.vidx[0] == b.vidx[1] { 1 } else { 2 };
        for v in 0..n {
            a.mov32(10, b.vidx[v] * 4); a.ldr_u(9, PV, 10);
            a.mov32(10, b.ver[v]); a.cmp(9, 10); a.b_cond(Cond::Ne, fail);
        }
    }

    /// The region's code and, per block, the word offset of its entry.
    fn emit(r: &Region, tables: &[Box<[Insn]>]) -> Option<(Vec<u32>, Vec<usize>)> {
        if xo(31) >= 16384 || offset_of!(Cpu, insn_count).max(offset_of!(Cpu, cycle_count)).max(offset_of!(Cpu, retired_count)) >= 32768
            || offset_of!(Cpu, pc) >= 16384 { return None; }
        let mut a = Asm::new();
        let epilogue = a.label();
        let labels: Vec<Label> = r.blocks.iter().map(|_| a.label()).collect();
        // one entry per block: the prologue, then on to the block
        let mut entries = Vec::with_capacity(r.blocks.len());
        for &l in &labels {
            entries.push(a.here());
            a.stp_pre(29, 30, SP, -80);
            a.stp(CPU, BUS, SP, 16);
            a.stp(HLP, CTX, SP, 32);
            a.stp(DONE, REM, SP, 48);
            a.str_x(PV, SP, 64);
            a.mov_x(CPU, 0); a.mov_x(BUS, 1); a.mov_x(HLP, 2); a.mov_x(CTX, 3); a.mov(REM, 4); a.mov(DONE, ZR);
            a.ldr_x(PV, CTX, offset_of!(Ctx, pv) as u32);
            a.b(l);
        }
        let mut exits: Vec<Exit> = Vec::new();
        for (bi, b) in r.blocks.iter().enumerate() {
            a.bind(labels[bi]);
            block(&mut a, r, b, &tables[bi], &labels, &mut exits, epilogue);
        }
        for e in exits {
            a.bind(e.at);
            let mut pend = e.pend;
            flush(&mut a, &mut pend);
            if let Some(n) = e.set_pc { set_pc(&mut a, n); }
            set_last(&mut a, e.pc);
            if let Some(s) = e.status { a.mov32(STATUS, s); }
            a.lsl_imm_x(0, STATUS, 32);
            a.add_imm(10, DONE, e.k + 1);
            a.orr_x(0, 0, 10);
            a.b(epilogue);
        }
        a.bind(epilogue);
        a.ldr_x(PV, SP, 64);
        a.ldp(DONE, REM, SP, 48);
        a.ldp(HLP, CTX, SP, 32);
        a.ldp(CPU, BUS, SP, 16);
        a.ldp_post(29, 30, SP, 80);
        a.ret();
        Some((a.finish(), entries))
    }

    fn block(a: &mut Asm, r: &Region, b: &RBlock, table: &[Insn], labels: &[Label], exits: &mut Vec<Exit>, epilogue: Label) {
        let pc_off = offset_of!(Cpu, pc) as u32;
        let mut p = Pending::default();
        let mut pc = b.pc;
        let n = b.insns.len();
        for (k, i) in b.insns.iter().enumerate() {
            let next = pc.wrapping_add(i.len as u32);
            let k32 = k as u32;
            let more = k + 1 < n;
            match kind(i.op) {
                Kind::Native => { p.insns += 1; p.retired += 1; native(a, i, pc, next, pc_off); }
                Kind::Load(w) => {
                    p.insns += 1;
                    ld(a, 3, i.rs1); a.mov32(9, i.imm as u32); a.add(3, 3, 9);
                    a.mov_x(0, CPU); a.mov_x(1, BUS); a.mov_x(2, CTX);
                    a.mov32(4, pc); a.mov32(5, w);
                    call_helper(a, H_LOAD);
                    a.lsr_imm_x(STATUS, 0, 32);
                    let trap = a.label();
                    a.cmp_imm(STATUS, TRAP); a.b_cond(Cond::Eq, trap);
                    exits.push(Exit { at: trap, k: k32, pc, pend: p, set_pc: None, status: None });
                    st(a, 0, i.rd);
                    let out = a.label();
                    a.cbnz(STATUS, out);
                    exits.push(Exit { at: out, k: k32, pc, pend: p, set_pc: Some(next), status: None });
                }
                Kind::Store(w) => {
                    p.insns += 1;
                    ld(a, 3, i.rs1); a.mov32(9, i.imm as u32); a.add(3, 3, 9);
                    ld(a, 4, i.rs2);
                    a.mov_x(0, CPU); a.mov_x(1, BUS); a.mov_x(2, CTX);
                    a.mov32(5, pc); a.mov32(6, w);
                    call_helper(a, H_STORE);
                    a.mov(STATUS, 0);
                    let trap = a.label();
                    a.cmp_imm(STATUS, TRAP); a.b_cond(Cond::Eq, trap);
                    exits.push(Exit { at: trap, k: k32, pc, pend: p, set_pc: None, status: None });
                    let out = a.label();
                    a.cbnz(STATUS, out);
                    exits.push(Exit { at: out, k: k32, pc, pend: p, set_pc: Some(next), status: None });
                    if more {
                        // the rest of this block may be stale
                        let stale = a.label();
                        check_versions(a, b, stale);
                        exits.push(Exit { at: stale, k: k32, pc, pend: p, set_pc: Some(next), status: Some(LEAVE) });
                    }
                }
                Kind::Exec => {
                    p.insns += 1;
                    flush(a, &mut p);
                    set_pc(a, pc);
                    a.mov_x(0, CPU); a.mov_x(1, BUS); a.mov_x(2, CTX);
                    a.mov64(3, table.as_ptr().wrapping_add(k) as u64);
                    a.mov32(4, pc);
                    call_helper(a, H_EXEC);
                    a.mov(STATUS, 0);
                    let out = a.label();
                    a.cbnz(STATUS, out);
                    // exec set the pc
                    exits.push(Exit { at: out, k: k32, pc, pend: p, set_pc: None, status: None });
                    if more && may_write(i.op) {
                        let stale = a.label();
                        check_versions(a, b, stale);
                        exits.push(Exit { at: stale, k: k32, pc, pend: p, set_pc: None, status: Some(LEAVE) });
                    }
                }
            }
            pc = next;
        }
        // the block ran whole
        flush(a, &mut p);
        a.add_imm(DONE, DONE, n as u32);
        a.sub_imm(REM, REM, n as u32);
        let last = b.last_pc();
        match b.succ {
            Succ::Gone => { set_last(a, last); a.mov(0, DONE); a.b(epilogue); }
            Succ::To(t) => transition(a, r, t, last, labels, epilogue),
            Succ::Branch { op, rs1, rs2, taken, not } => {
                ld(a, 0, rs1); ld(a, 1, rs2); a.cmp(0, 1);
                let c = match op { Op::Beq => Cond::Eq, Op::Bne => Cond::Ne, Op::Blt => Cond::Lt, Op::Bge => Cond::Ge, Op::Bltu => Cond::Lo, _ => Cond::Hs };
                let t_label = a.label();
                a.b_cond(c, t_label);
                transition(a, r, not, last, labels, epilogue);
                a.bind(t_label);
                transition(a, r, taken, last, labels, epilogue);
            }
        }
    }

    /// Go on to the block at `t`: inside the region when the dispatch loop would run it whole
    /// from the same lookup, else leave with the pc at `t`.
    fn transition(a: &mut Asm, r: &Region, t: u32, last: u32, labels: &[Label], epilogue: Label) {
        if let Some(m) = r.target(t) {
            let b = &r.blocks[m];
            let out = a.label();
            a.cmp_imm(REM, b.insns.len() as u32); a.b_cond(Cond::Lo, out);
            check_versions(a, b, out);
            a.b(labels[m]);
            a.bind(out);
        }
        leave(a, t, last, epilogue);
    }

    /// Host code for a native instruction at `pc` (a conditional branch is the block's end, see
    /// `block`; `jal` writes its link here, `jalr` its link and the pc).
    fn native(a: &mut Asm, i: &Insn, pc: u32, next: u32, pc_off: u32) {
        use Op::*;
        let imm = i.imm as u32;
        let rr = |a: &mut Asm| { ld(a, 0, i.rs1); ld(a, 1, i.rs2); };
        let ri = |a: &mut Asm| { ld(a, 0, i.rs1); a.mov32(1, imm); };
        match i.op {
            Fence | Beq | Bne | Blt | Bge | Bltu | Bgeu => return,
            Lui => { a.mov32(2, imm); }
            Auipc => { a.mov32(2, pc.wrapping_add(imm)); }
            Jal => { if i.rd != 0 { a.mov32(2, next); st(a, 2, i.rd); } return; }
            Jalr => {
                ld(a, 0, i.rs1); a.mov32(1, imm); a.add(3, 0, 1);
                a.mov32(4, !1); a.and(3, 3, 4);
                if i.rd != 0 { a.mov32(2, next); st(a, 2, i.rd); }
                a.str(3, CPU, pc_off);
                return;
            }
            Addi => { ri(a); a.add(2, 0, 1); }
            Slti => { ri(a); a.cmp(0, 1); a.cset(2, Cond::Lt); }
            Sltiu => { ri(a); a.cmp(0, 1); a.cset(2, Cond::Lo); }
            Xori => { ri(a); a.eor(2, 0, 1); }
            Ori => { ri(a); a.orr(2, 0, 1); }
            Andi => { ri(a); a.and(2, 0, 1); }
            Slli => { ld(a, 0, i.rs1); a.lsl_imm(2, 0, imm & 31); }
            Srli => { ld(a, 0, i.rs1); a.lsr_imm(2, 0, imm & 31); }
            Srai => { ld(a, 0, i.rs1); a.asr_imm(2, 0, imm & 31); }
            Add => { rr(a); a.add(2, 0, 1); }
            Sub => { rr(a); a.sub(2, 0, 1); }
            Sll => { rr(a); a.lslv(2, 0, 1); }
            Srl => { rr(a); a.lsrv(2, 0, 1); }
            Sra => { rr(a); a.asrv(2, 0, 1); }
            Slt => { rr(a); a.cmp(0, 1); a.cset(2, Cond::Lt); }
            Sltu => { rr(a); a.cmp(0, 1); a.cset(2, Cond::Lo); }
            Xor => { rr(a); a.eor(2, 0, 1); }
            Or => { rr(a); a.orr(2, 0, 1); }
            And => { rr(a); a.and(2, 0, 1); }
            Mul => { rr(a); a.mul(2, 0, 1); }
            Mulh => { rr(a); a.smull(2, 0, 1); a.lsr_imm_x(2, 2, 32); }
            Mulhu => { rr(a); a.umull(2, 0, 1); a.lsr_imm_x(2, 2, 32); }
            // AArch64 divides by zero to 0, RISC-V to all ones; the signed overflow case agrees
            Div | Divu => {
                rr(a);
                if i.op == Div { a.sdiv(2, 0, 1) } else { a.udiv(2, 0, 1) }
                a.movn(3, 0, 0); a.cmp_imm(1, 0); a.csel(2, 3, 2, Cond::Eq);
            }
            // a - (a / b) * b: b = 0 leaves a, the overflow case gives 0, as RISC-V defines
            Rem | Remu => {
                rr(a);
                if i.op == Rem { a.sdiv(2, 0, 1) } else { a.udiv(2, 0, 1) }
                a.msub(2, 2, 1, 0);
            }
            Sh1add | Sh2add | Sh3add => {
                rr(a);
                let sh = match i.op { Sh1add => 1, Sh2add => 2, _ => 3 };
                a.add_lsl(2, 1, 0, sh);
            }
            Andn => { rr(a); a.bic(2, 0, 1); }
            Orn => { rr(a); orn(a, 2, 0, 1); }
            Xnor => { rr(a); eon(a, 2, 0, 1); }
            Clz => { ld(a, 0, i.rs1); a.clz(2, 0); }
            Ctz => { ld(a, 0, i.rs1); rbit(a, 2, 0); a.clz(2, 2); }
            Max | Maxu | Min | Minu => {
                rr(a); a.cmp(0, 1);
                let c = match i.op { Max => Cond::Gt, Maxu => Cond::Hi, Min => Cond::Lt, _ => Cond::Lo };
                a.csel(2, 0, 1, c);
            }
            SextB => { ld(a, 0, i.rs1); a.sbfx(2, 0, 0, 8); }
            SextH => { ld(a, 0, i.rs1); a.sbfx(2, 0, 0, 16); }
            ZextH => { ld(a, 0, i.rs1); a.ubfx(2, 0, 0, 16); }
            Rol => { rr(a); a.neg(1, 1); rorv(a, 2, 0, 1); }
            Ror => { rr(a); rorv(a, 2, 0, 1); }
            Rori => { ld(a, 0, i.rs1); ror_imm(a, 2, 0, imm & 31); }
            Rev8 => { ld(a, 0, i.rs1); rev(a, 2, 0); }
            Bclr | Binv | Bset | Bext => {
                rr(a);
                if i.op == Bext { a.lsrv(2, 0, 1); a.ubfx(2, 2, 0, 1); } else {
                    a.mov32(3, 1); a.lslv(3, 3, 1);
                    match i.op { Bclr => a.bic(2, 0, 3), Binv => a.eor(2, 0, 3), _ => a.orr(2, 0, 3) }
                }
            }
            Bclri | Binvi | Bseti => {
                ld(a, 0, i.rs1); a.mov32(1, 1 << (imm & 31));
                match i.op { Bclri => a.bic(2, 0, 1), Binvi => a.eor(2, 0, 1), _ => a.orr(2, 0, 1) }
            }
            Bexti => { ld(a, 0, i.rs1); a.ubfx(2, 0, imm & 31, 1); }
            _ => unreachable!("not a native instruction: {:?}", i.op),
        }
        st(a, 2, i.rd);
    }
}

#[cfg(all(target_arch = "wasm32", feature = "wasm-jit-host"))]
mod wasm {
    //! Each region is a module with one function `(cpu, bus, ctx, budget, block: i32) -> i64` that
    //! imports the emulator's memory and function table; the page compiles it and installs it in
    //! the table (`host_jit_compile`, as for the Xtensa cores), and Rust calls it through a function
    //! pointer, which in wasm is its table index. Helpers are called the same way. The blocks sit
    //! in a `loop` around a `br_table` on the next block's index.
    use super::*;
    use crate::block::may_write;
    use std::mem::offset_of;

    pub const AVAILABLE: bool = true;

    #[link(wasm_import_module = "env")]
    extern "C" {
        fn host_jit_compile(ptr: *const u8, len: usize) -> u32;
        fn host_jit_release(slot: u32);
    }

    /// Modules, and bytes of them, before this core's are all discarded (the Xtensa JIT's
    /// `RETAIN_BLOCKS` and `RETAIN_BYTES`).
    const MAX_MODULES: usize = 16384;
    const MAX_BYTES: usize = 64 << 20;

    /// This core's modules, their size, and the instruction tables `h_exec` reads.
    pub struct Jit { tables: Vec<Box<[Insn]>>, slots: Vec<u32>, bytes: usize }

    impl Drop for Jit { fn drop(&mut self) { self.reset(); } }

    impl Jit {
        pub fn new() -> Option<Box<Jit>> { Some(Box::new(Jit { tables: Vec::new(), slots: Vec::new(), bytes: 0 })) }
        /// Forget all code (the caller forgets every block that refers to it).
        pub fn reset(&mut self) {
            // SAFETY: each slot came from `host_jit_compile` and is released once
            for s in self.slots.drain(..) { unsafe { host_jit_release(s) } }
            self.tables.clear();
            self.bytes = 0;
        }
        pub fn bytes(&self) -> usize { self.bytes }

        /// The block's code `compile` returned as `code`: its module's table index times 32 plus
        /// the block.
        pub fn entry(&self, code: u32) -> Code { Code(code as usize as *const u8) }

        /// # Safety
        /// `code` must come from `entry` of a live `Jit` since its last `reset`, for a block that
        /// is still the one in memory.
        pub unsafe fn call<B: Bus>(code: Code, cpu: &mut Cpu, bus: &mut B, ctx: &mut Ctx, budget: u32) -> u64 {
            type F<B> = unsafe extern "C" fn(*mut Cpu, *mut B, *mut Ctx, u32, u32) -> u64;
            let f: F<B> = std::mem::transmute::<usize, F<B>>(code.0 as usize >> 5);
            f(cpu, bus, ctx, budget, code.0 as u32 & 31)
        }

        /// Compile a region: the code of each of its blocks as an entry (`Region::members`).
        pub fn compile<B: Bus>(&mut self, r: &Region) -> Result<Vec<u32>, NoCode> {
            if r.blocks.len() > 32 { return Err(NoCode::Refused); }
            if self.slots.len() >= MAX_MODULES { return Err(NoCode::Full); }
            let tables: Vec<Box<[Insn]>> = r.blocks.iter().map(|b| b.insns.clone().into_boxed_slice()).collect();
            let module = emit::<B>(r, &tables);
            if self.bytes + module.len() > MAX_BYTES { return Err(NoCode::Full); }
            // SAFETY: the import reads `len` bytes of the module; 0 is its failure
            let idx = unsafe { host_jit_compile(module.as_ptr(), module.len()) };
            if idx == 0 { return Err(NoCode::Refused); }
            self.slots.push(idx);
            self.bytes += module.len();
            self.tables.extend(tables);
            Ok((0..r.blocks.len() as u32).map(|k| idx << 5 | k).collect())
        }
    }

    fn uleb(out: &mut Vec<u8>, mut v: u64) {
        loop { let b = (v & 0x7f) as u8; v >>= 7; if v == 0 { out.push(b); break; } out.push(b | 0x80); }
    }
    fn sleb(out: &mut Vec<u8>, mut v: i64) {
        loop {
            let b = (v & 0x7f) as u8; v >>= 7;
            let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0);
            out.push(if done { b } else { b | 0x80 });
            if done { break; }
        }
    }

    // params cpu, bus, ctx, budget left (REM), the block to start with (IDX); locals: status,
    // DONE, the code versions, a load's result
    const CPU: u32 = 0;
    const BUS: u32 = 1;
    const CTX: u32 = 2;
    const REM: u32 = 3;
    const IDX: u32 = 4;
    const ST: u32 = 5;
    const DONE: u32 = 6;
    const PV: u32 = 7;
    const V64: u32 = 8;
    // types: 0 the region, 1 h_exec, 2 h_load, 3 h_store
    const T_EXEC: u32 = 1;
    const T_LOAD: u32 = 2;
    const T_STORE: u32 = 3;

    struct W { c: Vec<u8>, h_exec: usize, h_load: usize, h_store: usize, nest: u32 }
    impl W {
        fn op(&mut self, b: u8) { self.c.push(b); }
        fn i32c(&mut self, v: u32) { self.op(0x41); sleb(&mut self.c, v as i32 as i64); }
        fn i64c(&mut self, v: u64) { self.op(0x42); sleb(&mut self.c, v as i64); }
        fn lget(&mut self, l: u32) { self.op(0x20); uleb(&mut self.c, l as u64); }
        fn lset(&mut self, l: u32) { self.op(0x21); uleb(&mut self.c, l as u64); }
        fn mem(&mut self, op: u8, align: u32, off: u32) { self.op(op); uleb(&mut self.c, align as u64); uleb(&mut self.c, off as u64); }
        fn get(&mut self, r: u8) { if r == 0 { self.i32c(0) } else { self.lget(CPU); self.mem(0x28, 2, xo(r)); } }
        fn set(&mut self, r: u8, f: impl FnOnce(&mut W)) { if r == 0 { return; } self.lget(CPU); f(self); self.mem(0x36, 2, xo(r)); }
        fn set_pc(&mut self, v: u32) { self.lget(CPU); self.i32c(v); self.mem(0x36, 2, offset_of!(Cpu, pc) as u32); }
        fn counter(&mut self, off: u32, d: u32) {
            if d == 0 { return; }
            self.lget(CPU); self.lget(CPU); self.mem(0x29, 3, off); self.i64c(d as u64); self.op(0x7c); self.mem(0x37, 3, off);
        }
        fn flush(&mut self, p: &mut Pending) {
            self.counter(offset_of!(Cpu, insn_count) as u32, p.insns);
            self.counter(offset_of!(Cpu, cycle_count) as u32, p.insns);
            self.counter(offset_of!(Cpu, retired_count) as u32, p.retired);
            *p = Pending::default();
        }
        fn last_pc(&mut self, v: u32) { self.lget(CTX); self.i32c(v); self.mem(0x36, 2, offset_of!(Ctx, last_pc) as u32); }
        fn call(&mut self, f: usize, ty: u32) { self.i32c(f as u32); self.op(0x11); uleb(&mut self.c, ty as u64); self.op(0x00); }
        fn if_(&mut self) { self.op(0x04); self.op(0x40); self.nest += 1; }
        fn end(&mut self) { self.op(0x0b); self.nest -= 1; }
        /// return (status << 32) | (DONE + done), the status in ST
        fn ret_status(&mut self, done: u32) {
            self.lget(ST); self.op(0xad); self.i64c(32); self.op(0x86);
            self.lget(DONE); self.i32c(done); self.op(0x6a); self.op(0xad); self.op(0x84); self.op(0x0f);
        }
        /// an early exit after instruction `k`: counters, pc, last pc, status
        fn exit(&mut self, mut p: Pending, k: u32, pc: u32, set_pc: Option<u32>, status: Option<u32>) {
            self.flush(&mut p);
            if let Some(n) = set_pc { self.set_pc(n); }
            self.last_pc(pc);
            if let Some(s) = status { self.i32c(s); self.lset(ST); }
            self.ret_status(k + 1);
        }
        /// push whether block `b`'s code versions still hold
        fn versions_ok(&mut self, b: &RBlock) {
            let n = if b.vidx[0] == b.vidx[1] { 1 } else { 2 };
            for v in 0..n {
                self.lget(PV); self.i32c(b.vidx[v] * 4); self.op(0x6a); self.mem(0x28, 2, 0);
                self.i32c(b.ver[v]); self.op(0x46);
                if v == 1 { self.op(0x71); }
            }
        }
    }

    fn xo(r: u8) -> u32 { offset_of!(Cpu, x) as u32 + 4 * r as u32 }

    fn emit<B: Bus>(r: &Region, tables: &[Box<[Insn]>]) -> Vec<u8> {
        let mut w = W { c: Vec::with_capacity(4096), h_exec: h_exec::<B> as *const () as usize, h_load: h_load::<B> as *const () as usize, h_store: h_store::<B> as *const () as usize, nest: 0 };
        w.i32c(0); w.lset(DONE);
        w.lget(CTX); w.mem(0x28, 2, offset_of!(Ctx, pv) as u32); w.lset(PV);
        let nb = r.blocks.len() as u32;
        // loop $top; block $b(nb-1) .. block $b0; br_table on IDX
        w.op(0x03); w.op(0x40);
        for _ in 0..nb { w.op(0x02); w.op(0x40); }
        w.lget(IDX); w.op(0x0e); uleb(&mut w.c, nb as u64 - 1);
        for k in 0..nb - 1 { uleb(&mut w.c, k as u64); }
        uleb(&mut w.c, nb as u64 - 1);
        for (bi, b) in r.blocks.iter().enumerate() {
            w.op(0x0b); // end of $b(bi): its code follows, inside $b(bi+1).. and the loop
            // the loop is at depth (blocks still open) = nb - 1 - bi
            block(&mut w, r, b, &tables[bi], nb - 1 - bi as u32);
        }
        w.op(0x0b); // end of the loop
        w.op(0x00); // unreachable: every block ends in a return or a branch to the loop
        w.op(0x0b);
        assemble(&w.c)
    }

    fn block(w: &mut W, r: &Region, b: &RBlock, table: &[Insn], depth: u32) {
        let mut p = Pending::default();
        let mut pc = b.pc;
        let n = b.insns.len();
        for (k, i) in b.insns.iter().enumerate() {
            let next = pc.wrapping_add(i.len as u32);
            let k32 = k as u32;
            let more = k + 1 < n;
            match kind(i.op) {
                Kind::Native => { p.insns += 1; p.retired += 1; native(w, i, pc, next); }
                Kind::Load(kd) => {
                    p.insns += 1;
                    w.lget(CPU); w.lget(BUS); w.lget(CTX);
                    w.get(i.rs1); w.i32c(i.imm as u32); w.op(0x6a);
                    w.i32c(pc); w.i32c(kd);
                    w.call(w.h_load, T_LOAD);
                    w.lset(V64);
                    w.lget(V64); w.i64c(32); w.op(0x88); w.op(0xa7); w.lset(ST);
                    w.lget(ST); w.i32c(TRAP); w.op(0x46); w.if_(); w.exit(p, k32, pc, None, None); w.end();
                    if i.rd != 0 { w.lget(CPU); w.lget(V64); w.op(0xa7); w.mem(0x36, 2, xo(i.rd)); }
                    w.lget(ST); w.if_(); w.exit(p, k32, pc, Some(next), None); w.end();
                }
                Kind::Store(kd) => {
                    p.insns += 1;
                    w.lget(CPU); w.lget(BUS); w.lget(CTX);
                    w.get(i.rs1); w.i32c(i.imm as u32); w.op(0x6a);
                    w.get(i.rs2); w.i32c(pc); w.i32c(kd);
                    w.call(w.h_store, T_STORE);
                    w.lset(ST);
                    w.lget(ST); w.i32c(TRAP); w.op(0x46); w.if_(); w.exit(p, k32, pc, None, None); w.end();
                    w.lget(ST); w.if_(); w.exit(p, k32, pc, Some(next), None); w.end();
                    if more { w.versions_ok(b); w.op(0x45); w.if_(); w.exit(p, k32, pc, Some(next), Some(LEAVE)); w.end(); }
                }
                Kind::Exec => {
                    p.insns += 1;
                    w.flush(&mut p);
                    w.set_pc(pc);
                    w.lget(CPU); w.lget(BUS); w.lget(CTX);
                    w.i32c(table.as_ptr().wrapping_add(k) as usize as u32); w.i32c(pc);
                    w.call(w.h_exec, T_EXEC);
                    w.lset(ST);
                    w.lget(ST); w.if_(); w.exit(p, k32, pc, None, None); w.end();
                    if more && may_write(i.op) { w.versions_ok(b); w.op(0x45); w.if_(); w.exit(p, k32, pc, None, Some(LEAVE)); w.end(); }
                }
            }
            pc = next;
        }
        w.flush(&mut p);
        w.lget(DONE); w.i32c(n as u32); w.op(0x6a); w.lset(DONE);
        w.lget(REM); w.i32c(n as u32); w.op(0x6b); w.lset(REM);
        let last = b.last_pc();
        match b.succ {
            Succ::Gone => { w.last_pc(last); w.lget(DONE); w.op(0xad); w.op(0x0f); }
            Succ::To(t) => transition(w, r, t, last, depth),
            Succ::Branch { op, rs1, rs2, taken, not } => {
                let cmp = match op { Op::Beq => 0x46, Op::Bne => 0x47, Op::Blt => 0x48, Op::Bge => 0x4e, Op::Bltu => 0x49, _ => 0x4f };
                w.get(rs1); w.get(rs2); w.op(cmp);
                w.if_(); transition(w, r, taken, last, depth); w.end();
                transition(w, r, not, last, depth);
            }
        }
    }

    /// Go on to the block at `t` inside the region when the dispatch loop would run it whole from
    /// the same lookup, else leave with the pc at `t`. `depth`: the loop's label depth here,
    /// before any `if` this emits (`W::nest` counts those around it).
    fn transition(w: &mut W, r: &Region, t: u32, last: u32, depth: u32) {
        if let Some(m) = r.target(t) {
            let b = &r.blocks[m];
            // REM >= n && versions
            w.lget(REM); w.i32c(b.insns.len() as u32); w.op(0x4f);
            w.versions_ok(b); w.op(0x71);
            w.if_();
            w.i32c(m as u32); w.lset(IDX);
            w.op(0x0c); uleb(&mut w.c, (depth + w.nest - 1 + 1) as u64);
            w.end();
        }
        w.set_pc(t); w.last_pc(last); w.lget(DONE); w.op(0xad); w.op(0x0f);
    }

    /// Wasm for a native instruction (a conditional branch is the block's end; `jal` writes its
    /// link here, `jalr` its link and the pc).
    fn native(w: &mut W, i: &Insn, pc: u32, next: u32) {
        use Op::*;
        let imm = i.imm as u32;
        let (rs1, rs2) = (i.rs1, i.rs2);
        let rr = |w: &mut W, ops: &[u8]| { let ops = ops.to_vec(); w.set(i.rd, move |w| { w.get(rs1); w.get(rs2); for o in ops { w.op(o); } }); };
        let ri = |w: &mut W, v: u32, ops: &[u8]| { let ops = ops.to_vec(); w.set(i.rd, move |w| { w.get(rs1); w.i32c(v); for o in ops { w.op(o); } }); };
        match i.op {
            Fence | Beq | Bne | Blt | Bge | Bltu | Bgeu => {}
            Lui => w.set(i.rd, |w| w.i32c(imm)),
            Auipc => w.set(i.rd, |w| w.i32c(pc.wrapping_add(imm))),
            Jal => w.set(i.rd, |w| w.i32c(next)),
            Jalr => {
                // the target first: rd may be rs1
                w.get(rs1); w.i32c(imm); w.op(0x6a); w.i32c(!1); w.op(0x71); w.lset(ST);
                w.set(i.rd, |w| w.i32c(next));
                w.lget(CPU); w.lget(ST); w.mem(0x36, 2, offset_of!(Cpu, pc) as u32);
            }
            Addi => ri(w, imm, &[0x6a]),
            Slti => ri(w, imm, &[0x48]),
            Sltiu => ri(w, imm, &[0x49]),
            Xori => ri(w, imm, &[0x73]),
            Ori => ri(w, imm, &[0x72]),
            Andi => ri(w, imm, &[0x71]),
            Slli => ri(w, imm & 31, &[0x74]),
            Srli => ri(w, imm & 31, &[0x76]),
            Srai => ri(w, imm & 31, &[0x75]),
            Add => rr(w, &[0x6a]),
            Sub => rr(w, &[0x6b]),
            Sll => rr(w, &[0x74]),
            Srl => rr(w, &[0x76]),
            Sra => rr(w, &[0x75]),
            Slt => rr(w, &[0x48]),
            Sltu => rr(w, &[0x49]),
            Xor => rr(w, &[0x73]),
            Or => rr(w, &[0x72]),
            And => rr(w, &[0x71]),
            Mul => rr(w, &[0x6c]),
            Mulh | Mulhu => {
                let ext = if i.op == Mulh { 0xac } else { 0xad };
                w.set(i.rd, |w| { w.get(rs1); w.op(ext); w.get(rs2); w.op(ext); w.op(0x7e); w.i64c(32); w.op(0x88); w.op(0xa7); });
            }
            // wasm traps where RISC-V defines a result: divisor 0, and INT_MIN / -1 (signed)
            Div | Divu | Rem | Remu => {
                let signed = matches!(i.op, Div | Rem);
                let div = matches!(i.op, Div | Divu);
                w.set(i.rd, |w| {
                    w.get(rs2); w.op(0x45);
                    w.op(0x04); w.op(0x7f);
                    if div { w.i32c(u32::MAX) } else { w.get(rs1) }
                    w.op(0x05);
                    if signed {
                        w.get(rs1); w.i32c(0x8000_0000); w.op(0x46); w.get(rs2); w.i32c(u32::MAX); w.op(0x46); w.op(0x71);
                        w.op(0x04); w.op(0x7f);
                        if div { w.get(rs1) } else { w.i32c(0) }
                        w.op(0x05);
                        w.get(rs1); w.get(rs2); w.op(if div { 0x6d } else { 0x6f });
                        w.op(0x0b);
                    } else {
                        w.get(rs1); w.get(rs2); w.op(if div { 0x6e } else { 0x70 });
                    }
                    w.op(0x0b);
                });
            }
            Sh1add | Sh2add | Sh3add => {
                let sh = match i.op { Sh1add => 1, Sh2add => 2, _ => 3 };
                w.set(i.rd, |w| { w.get(rs1); w.i32c(sh); w.op(0x74); w.get(rs2); w.op(0x6a); });
            }
            Andn => w.set(i.rd, |w| { w.get(rs1); w.get(rs2); w.i32c(u32::MAX); w.op(0x73); w.op(0x71); }),
            Orn => w.set(i.rd, |w| { w.get(rs1); w.get(rs2); w.i32c(u32::MAX); w.op(0x73); w.op(0x72); }),
            Xnor => w.set(i.rd, |w| { w.get(rs1); w.get(rs2); w.op(0x73); w.i32c(u32::MAX); w.op(0x73); }),
            Clz => w.set(i.rd, |w| { w.get(rs1); w.op(0x67); }),
            Ctz => w.set(i.rd, |w| { w.get(rs1); w.op(0x68); }),
            Max | Maxu | Min | Minu => {
                let c = match i.op { Max => 0x4a, Maxu => 0x4b, Min => 0x48, _ => 0x49 };
                w.set(i.rd, |w| { w.get(rs1); w.get(rs2); w.get(rs1); w.get(rs2); w.op(c); w.op(0x1b); });
            }
            SextB => w.set(i.rd, |w| { w.get(rs1); w.i32c(24); w.op(0x74); w.i32c(24); w.op(0x75); }),
            SextH => w.set(i.rd, |w| { w.get(rs1); w.i32c(16); w.op(0x74); w.i32c(16); w.op(0x75); }),
            ZextH => w.set(i.rd, |w| { w.get(rs1); w.i32c(0xffff); w.op(0x71); }),
            Rol => rr(w, &[0x77]),
            Ror => rr(w, &[0x78]),
            Rori => ri(w, imm & 31, &[0x78]),
            Rev8 => w.set(i.rd, |w| {
                w.get(rs1); w.i32c(24); w.op(0x74);
                w.get(rs1); w.i32c(8); w.op(0x74); w.i32c(0x00ff_0000); w.op(0x71); w.op(0x72);
                w.get(rs1); w.i32c(8); w.op(0x76); w.i32c(0xff00); w.op(0x71); w.op(0x72);
                w.get(rs1); w.i32c(24); w.op(0x76); w.op(0x72);
            }),
            Bclr | Binv | Bset => {
                let o = match i.op { Bclr => 0x71, Binv => 0x73, _ => 0x72 };
                w.set(i.rd, |w| {
                    w.get(rs1); w.i32c(1); w.get(rs2); w.op(0x74);
                    if o == 0x71 { w.i32c(u32::MAX); w.op(0x73); }
                    w.op(o);
                });
            }
            Bext => w.set(i.rd, |w| { w.get(rs1); w.get(rs2); w.op(0x76); w.i32c(1); w.op(0x71); }),
            Bclri => ri(w, !(1u32 << (imm & 31)), &[0x71]),
            Binvi => ri(w, 1 << (imm & 31), &[0x73]),
            Bseti => ri(w, 1 << (imm & 31), &[0x72]),
            Bexti => w.set(i.rd, |w| { w.get(rs1); w.i32c(imm & 31); w.op(0x76); w.i32c(1); w.op(0x71); }),
            _ => unreachable!("not a native instruction: {:?}", i.op),
        }
    }

    /// The module: types, imports (env.memory, env.table), one function exported as "run".
    fn assemble(body: &[u8]) -> Vec<u8> {
        let mut m = b"\0asm\x01\0\0\0".to_vec();
        let section = |m: &mut Vec<u8>, id: u8, payload: &[u8]| { m.push(id); uleb(m, payload.len() as u64); m.extend_from_slice(payload); };
        const I: u8 = 0x7f; const L: u8 = 0x7e;
        section(&mut m, 1, &[4,
            0x60, 5, I, I, I, I, I, 1, L,          // the region (cpu, bus, ctx, budget, start) -> i64
            0x60, 5, I, I, I, I, I, 1, I,          // h_exec (cpu, bus, ctx, insn, pc) -> i32
            0x60, 6, I, I, I, I, I, I, 1, L,       // h_load (cpu, bus, ctx, va, pc, kind) -> i64
            0x60, 7, I, I, I, I, I, I, I, 1, I,    // h_store (cpu, bus, ctx, va, v, pc, kind) -> i32
        ]);
        let mut imp = vec![2];
        for (name, desc) in [("memory", &[0x02u8, 0x00, 0x00][..]), ("table", &[0x01u8, 0x70, 0x00, 0x00][..])] {
            imp.push(3); imp.extend_from_slice(b"env"); imp.push(name.len() as u8); imp.extend_from_slice(name.as_bytes()); imp.extend_from_slice(desc);
        }
        section(&mut m, 2, &imp);
        section(&mut m, 3, &[1, 0]);
        section(&mut m, 7, b"\x01\x03run\x00\x00");
        // locals after the 5 params: ST, DONE, PV (i32), V64 (i64)
        let mut f = vec![2, 3, I, 1, L];
        f.extend_from_slice(body);
        let mut code = vec![1];
        uleb(&mut code, f.len() as u64);
        code.extend_from_slice(&f);
        section(&mut m, 10, &code);
        m
    }
}
