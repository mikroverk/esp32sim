//! WASM block backend for the ordinary scheduler. Hot blocks are installed once in the
//! exported function table; execution then uses WASM call_indirect, with no JS dispatch.
//! This preserves the interpreter's instruction-count timing, not the receipt cost model.
use crate::block::BlockInsn;
use crate::bus::{Bus, FastMem, TlbEntry};
use crate::exec::exec_insn;
use crate::state::{ps, Cpu};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::mem::{offset_of, size_of};

pub const AVAILABLE: bool = true;
pub const NONE: u32 = u32::MAX;
pub const CODE_END: u32 = 0;
pub const CODE_LEFT: u32 = 1;
pub const CODE_TRAP: u32 = 2;
pub const CODE_CUT: u32 = 3;
pub const CODE_TRAP_PRE: u32 = 4;
/// A region declined to run (resume, short credit, window or coprocessor state).
pub const CODE_REJECT: u32 = 5;
/// tails-s1: private region exits the dispatcher reports as CODE_LEFT. SHORT: a fresh region
/// left at a chunk head short of credit, counted. TAIL: a guarded copy cut mid-chunk; the site
/// after its own names the chunk head.
const CODE_SHORT: u32 = 6;
const CODE_TAIL: u32 = 7;
/// tails-s1: credit-short exits a region counts before it chooses its guarded copies
/// (few under the differential suite, so its region programs run through the copies).
const SHORT_SAMPLE: u32 = if cfg!(feature = "wasm-jit-tests") { 2 } else { 1024 };
/// edge-s1: instructions a region may copy, whatever the number of chunks (tails-s1's 4 x 32 bound).
const COPY_ROOM: u32 = 128;
/// edge-s1r: copy selections per formation; a later one only adds chunks, from its own window.
const TUNES: u8 = 3;
/// Formation attempts per block, including re-formation after a code page changed.
const REGION_TRIES: u8 = 8;
#[cfg(feature = "wasm-jit-tests")]
pub(crate) static REGION_STATS: [std::sync::atomic::AtomicU32; 18] = [const { std::sync::atomic::AtomicU32::new(0) }; 18];

/// Last retired PC and (lane-s1) the region parameter that resumes right at the exit PC when the
/// exit ended the quantum there (credit short at a chunk head, a guarded copy's cut), else NONE.
#[cfg(not(feature = "wasm-jit-profile"))]
type ExitSite = (u32, u32);
#[cfg(feature = "wasm-jit-profile")]
type ExitSite = (u32, ExitKind, u32);

#[cfg(feature = "wasm-jit-profile")]
#[derive(Clone, Copy, Default)]
enum ExitKind {
    Call, Callx, Retw, Ret, Jx, Sr, Memory, Edge, Budget, Dirty,
    #[default]
    Other,
}
#[cfg(feature = "wasm-jit-profile")]
impl ExitKind {
    fn for_op(op: crate::Op) -> Self {
        use crate::Op::*;
        match op {
            Call0 | Call4 | Call8 | Call12 => Self::Call,
            Callx0 | Callx4 | Callx8 | Callx12 => Self::Callx,
            Retw | RetwN => Self::Retw,
            Ret | RetN => Self::Ret,
            Jx => Self::Jx,
            Wsr | Xsr | Rsil => Self::Sr,
            L8ui | L16ui | L16si | L32i | L32iN | L32r | S8i | S16i | S32i | S32iN | Lsi | Ssi | Pie => Self::Memory,
            _ => Self::Other,
        }
    }
}
#[inline(always)]
fn site_pc(site: ExitSite) -> u32 { site.0 }
#[inline(always)]
fn site_resume(site: ExitSite) -> u32 {
    #[cfg(feature = "wasm-jit-profile")]
    { site.2 }
    #[cfg(not(feature = "wasm-jit-profile"))]
    { site.1 }
}

/// Region counters for the opt-in profile build; absent from production.
#[cfg(feature = "wasm-jit-profile")]
#[derive(Default)]
pub struct RegionStats {
    pub formed: Cell<u64>,
    pub failed: Cell<u64>,
    pub covered: Cell<u64>,
    pub dropped: Cell<u64>,
    pub tuned: Cell<u64>,
    pub calls: Cell<u64>,
    pub rejected: Cell<u64>,
    pub retired: Cell<u64>,
    pub exits: [Cell<u64>; 8],
    pub left_kinds: [Cell<u64>; 11],
    pub chunks: Cell<u64>,
    pub instructions: Cell<u64>,
    pub bytes: Cell<u64>,
    /// EX153 census: [run calls, calls with budget>=64, whole calls, whole retired, tail-cut calls, tail-cut retired,
    /// resumed calls, resumed retired, resumed-and-cut-again calls, zero-retired calls, sum of budgets, chained calls]
    /// lane-s1: [12] memo hits, [13] memo declined at a matching PC
    pub ex153: [Cell<u64>; 14],
}
#[cfg(feature = "wasm-jit-profile")]
impl RegionStats {
    pub fn report(&self) -> String {
        format!("[ex153] run_calls={} budget64_calls={} whole_calls={} whole_retired={} tailcut_calls={} tailcut_retired={} resumed_calls={} resumed_retired={} resumed_cut_again={} zero_retired_calls={} budget_sum={} chained={} memo_hits={} memo_declined={}\n[wasm-region] formed={} failed={} covered={} dropped={} chunks={} instructions={} bytes={} calls={} rejected={} retired={} exits[end,left,trap,cut,pre]={:?} left_kinds[call,callx,retw,ret,jx,sr,memory,edge,budget,dirty,other]={:?} tuned={}",
            self.ex153[0].get(), self.ex153[1].get(), self.ex153[2].get(), self.ex153[3].get(), self.ex153[4].get(), self.ex153[5].get(),
            self.ex153[6].get(), self.ex153[7].get(), self.ex153[8].get(), self.ex153[9].get(), self.ex153[10].get(), self.ex153[11].get(), self.ex153[12].get(), self.ex153[13].get(),
            self.formed.get(), self.failed.get(), self.covered.get(), self.dropped.get(), self.chunks.get(),
            self.instructions.get(), self.bytes.get(), self.calls.get(), self.rejected.get(), self.retired.get(),
            self.exits[..5].iter().map(|c| c.get()).collect::<Vec<_>>(),
            self.left_kinds.iter().map(|c| c.get()).collect::<Vec<_>>(), self.tuned.get())
    }
}
pub static CENSUS: [std::sync::atomic::AtomicU64; 8] = [const { std::sync::atomic::AtomicU64::new(0) }; 8];
#[inline(always)]
fn census(i: usize, n: u64) { if cfg!(feature = "wasm-cpu-profile") { CENSUS[i].fetch_add(n, std::sync::atomic::Ordering::Relaxed); } }
const HOT: u32 = 32;
/// EX138: emit control-flow prices into code generated from now on.
pub static PRICED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Emit the inline data-cache probe into code generated from now on (a `cache-inline` build that
/// runs without the timing model must not pay for it).
/// EX147: generated code records reached instruction byte ranges in `Cpu::fetch_ring`.
pub static FETCH_RING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Set index mask of the inline data-cache probe: 63 for the 32 KB cache, 127 for 64 KB (64-byte lines, 8 ways).
pub static CACHE_SET_MASK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(63);
pub static CACHE_PROBES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const RETAIN_BYTES: usize = 64 << 20;
const RETAIN_BLOCKS: usize = 16_384;

#[link(wasm_import_module = "env")]
extern "C" {
    fn host_jit_compile(bytes: *const u8, len: usize) -> u32;
    fn host_jit_release(slot: u32);
}

struct Block {
    instructions: Vec<BlockInsn>,
    pc: u32,
    pcs: Vec<u32>,
    fast: bool,
    loop_prefix: usize,
    /// EX156: the LEND seen inside this block when its module was generated, or zero.
    lend_hint: Cell<u32>,
    generation: u64,
    hits: Cell<u32>,
    bytes: Cell<usize>,
    /// A region headed by this block, once it is hot and one could be formed.
    region: RefCell<Option<Region>>,
    region_tries: Cell<u8>,
    /// Region (owning block, chunk) this head was last found in; rechecked when stale.
    covered_by: Cell<(u32, u32)>,
    /// Last coverage epoch where this PC was absent from the map.
    uncovered_epoch: Cell<u64>,
}
/// shell-s1: what a dispatch of a compiled block reads, flat and borrow-free, indexed like
/// `CodeCache::blocks`; `run_block_body` still names its last instruction through `Block::pcs`.
struct Rec {
    slot: Cell<u32>,
    pc: u32,
    n: u32,
    /// Retained-loop answer for LBEG == pc: (LEND, prefix length or 0, code pages of the first and
    /// last byte). `LOOP_UNKNOWN` until computed, and again whenever a rebuild reuses this block.
    looped: Cell<(u32, u32, [u32; 2])>,
    /// EX136: everything a dispatch at this head needs to enter its region, copied out of the
    /// owning block so the common path follows no pointers; valid while `epoch` is current.
    hot: Cell<Hot>,
    /// hop-s2: the coverage epoch at which this block had no region, no cover and no formation
    /// tries left, or `u64::MAX`. Only a coverage insertion (which moves that epoch) can change it.
    none: Cell<u64>,
}
const LOOP_UNKNOWN: (u32, u32, [u32; 2]) = (0, u32::MAX, [0; 2]);
/// Entry facts of one region chunk. `sites` points into the owning region's vector, which
/// lives until that region is dropped, and every drop moves `CodeCache::region_epoch` on.
#[derive(Clone, Copy)]
/// `copy`: tails-s2, dispatch index of this chunk's guarded copy, or 0. `counting`: edge-s1, the
/// region still counts where quanta end and this chunk has no copy but could get one.
struct Hot { epoch: u64, bloom: u64, slot: u32, k: u32, len: u32, lo: u32, span: u32, pages: [(u32, u32); emitter::region::MAX_PAGES], npages: u32, nsites: u32, sites: *const ExitSite, copy: u32,
    /// shell-s2: `Bus::stable_pages` epoch at the refill; `pages` leaves out the pages it vouches for.
    fepoch: u64, counting: bool,
    /// lane-s1l: the owning region's (LEND, LBEG) loops, live while `epoch` is current.
    loops: *const (u32, u32), nloops: u32,
    /// loop-s1: the region loop (LEND, LBEG) whose body holds this chunk; with it active the region continues it.
    lp: Option<(u32, u32)> }
impl Hot { const NONE: Hot = Hot { epoch: 0, bloom: 0, slot: 0, k: 0, len: 0, lo: 0, span: 0, pages: [(0, 0); emitter::region::MAX_PAGES], npages: 0, nsites: 0, sites: std::ptr::null(), copy: 0, fepoch: 0, counting: false, loops: std::ptr::null(), nloops: 0, lp: None }; }
#[cfg(any(debug_assertions, feature = "wasm-jit-tests"))]
impl Hot {
    fn assert_matches(&self, r: &Region, k: u32, (lo, hi, _): (u32, u32, u64)) {
        assert_eq!((self.slot, self.k, self.len, self.copy), (r.slot, k, r.lens[k as usize], r.copies[k as usize]));
        assert_eq!((self.bloom, self.lo, self.span), (r.bloom, r.lo, r.hi.wrapping_sub(r.lo)));
        assert!(self.pages[..self.npages as usize].iter().eq(r.pages.iter().filter(|&&(i, _)| i < lo || i >= hi)));
        assert_eq!((self.sites, self.nsites), (r.sites.as_ptr(), r.sites.len() as u32));
        assert_eq!((self.loops, self.nloops), (r.loops.as_ptr(), r.loops.len() as u32));
        assert_eq!(self.counting, !r.short.is_empty() && r.lens[k as usize] > 1 && r.copies[k as usize] == 0);
        assert_eq!(self.lp, r.chunk_loop(k));
    }
}
/// Several chunks compiled as one function; see wasm_region.rs.
struct Region {
    /// The generated code holds pointers to these instructions for its helper calls,
    /// so they live exactly as long as the module does.
    #[allow(dead_code)]
    chunks: Vec<emitter::region::Chunk>,
    leaves: Vec<emitter::region::Leaf>,
    slot: u32,
    bytes: usize,
    bloom: u64,
    lo: u32,
    hi: u32,
    loops: Vec<(u32, u32)>,
    pages: Vec<(u32, u32)>,
    sites: Vec<ExitSite>,
    /// instructions per chunk, for the credit check at an entry
    lens: Vec<u32>,
    /// tails-s1: credit-short exits per target chunk while counting (edge-s1: and own-module
    /// resumes); empty after the last of TUNES selections, so regeneration stays bounded.
    short: Vec<u32>,
    tunes: u8,
    /// tails-s2: dispatch index of each chunk's guarded copy, or 0.
    copies: Vec<u32>,
}
impl Region {
    fn chunk_loop(&self, k: u32) -> Option<(u32, u32)> {
        let pc = self.chunks[k as usize].pc;
        self.loops.iter().copied().find(|&(lend, lbeg)| pc.wrapping_sub(lbeg) < lend.wrapping_sub(lbeg))
    }
}
// Compiled instructions own their backing storage, independently of the decoder arena.
// A decoder flush invalidates every handle before reset may compact this cache.
pub struct CodeCache {
    blocks: Vec<Block>,
    recs: Vec<Rec>,
    by_pc: HashMap<(u32, usize, bool), u32>,
    generation: u64,
    /// Chunk heads of live regions: PC -> (owning block, chunk index). A head inside
    /// some region does not get a region of its own; a dispatch there enters the
    /// covering region at that chunk. Overlapping copies of one loop would only cost
    /// code and compile time.
    covered: RefCell<HashMap<u32, (u32, u32)>>,
    /// Advances whenever a previously absent PC might acquire a region.
    coverage_epoch: Cell<u64>,
    /// EX136: never zero; advance on drops, index changes or any change to a live block's
    /// owning region/chunk. Live region facts are immutable and insertion must not replace
    /// an existing owner: both rejection caching and skipped refills rely on this.
    region_epoch: Cell<u64>,
    #[cfg(feature = "wasm-jit-profile")]
    pub region_stats: RegionStats,
}
impl Block {
    fn release(&self, slot: u32, id: u32, covered: &RefCell<HashMap<u32, (u32, u32)>>) {
        if slot != NONE && slot != 0 {
            // SAFETY: reset/drop happen only when no compiled block is executing.
            unsafe {
                host_jit_release(slot);
            }
        }
        self.drop_region(id, covered);
    }
    fn drop_region(&self, id: u32, covered: &RefCell<HashMap<u32, (u32, u32)>>) {
        if let Some(r) = self.region.borrow_mut().take() {
            let mut covered = covered.borrow_mut();
            for c in &r.chunks {
                if covered.get(&c.pc).is_some_and(|&(owner, _)| owner == id) { covered.remove(&c.pc); }
            }
            // SAFETY: as above; a region is dropped from Rust between compiled calls.
            unsafe {
                host_jit_release(r.slot);
            }
        }
    }
    fn size(&self) -> usize {
        self.bytes.get() + self.region.borrow().as_ref().map_or(0, |r| r.bytes)
    }
}
impl CodeCache {
    pub fn new(_: usize) -> Option<Self> {
        Some(Self {
            blocks: Vec::new(),
            recs: Vec::new(),
            by_pc: HashMap::new(),
            generation: 0,
            covered: RefCell::new(HashMap::new()),
            coverage_epoch: Cell::new(0),
            region_epoch: Cell::new(1),
            #[cfg(feature = "wasm-jit-profile")]
            region_stats: RegionStats::default(),
        })
    }
    pub fn used(&self) -> usize {
        self.blocks.iter().map(|b| b.size()).sum()
    }
    pub fn reset(&mut self) {
        self.generation += 1;
        self.coverage_epoch.set(self.coverage_epoch.get().wrapping_add(1));
        self.region_epoch.set(self.region_epoch.get() + 1);
        // Keep recently decoded blocks across arena turnover. Prefer recent code under
        // pressure; enforce these retention limits only after all decoder handles die.
        let mut all: Vec<(Block, Rec)> = self.blocks.drain(..).zip(self.recs.drain(..)).collect();
        all.sort_by_key(|(b, _)| std::cmp::Reverse(b.generation));
        let (mut bytes, mut count) = (0, 0);
        let (generation, covered) = (self.generation, &self.covered);
        let mut id = 0u32;
        all.retain(|(b, r)| {
            let keep = generation - b.generation <= 2
                && count < RETAIN_BLOCKS
                && bytes + b.size() <= RETAIN_BYTES;
            if keep {
                bytes += b.size();
                count += 1;
            } else {
                b.release(r.slot.get(), id, covered);
            }
            id += 1;
            keep
        });
        (self.blocks, self.recs) = all.into_iter().unzip();
        // Retained blocks have new indices: rebuild every map that holds them.
        self.by_pc.clear();
        let mut covered = self.covered.borrow_mut();
        covered.clear();
        for (id, b) in self.blocks.iter().enumerate() {
            // Compaction changed block indices; a surviving hint can name a different
            // overlapping owner and still pass the live-chunk check. Refill from the map.
            b.covered_by.set((NONE, 0));
            self.by_pc
                .insert((b.pc, b.instructions.len(), b.fast), id as u32);
            if let Some(r) = b.region.borrow().as_ref() {
                for (k, c) in r.chunks.iter().enumerate() { covered.entry(c.pc).or_insert((id as u32, k as u32)); }
            }
        }
    }
}
impl Drop for CodeCache {
    fn drop(&mut self) {
        for (id, (b, r)) in self.blocks.iter().zip(&self.recs).enumerate() {
            b.release(r.slot.get(), id as u32, &self.covered);
        }
    }
}

pub fn compile(
    cc: &mut CodeCache,
    instructions: &mut [BlockInsn],
    pc: u32,
    fast: bool,
) -> Option<u32> {
    if instructions.len() < 2 || !emitter::admitted(instructions, fast) {
        return None;
    }
    Some(queue(cc, instructions, pc, fast))
}

fn queue(cc: &mut CodeCache, instructions: &mut [BlockInsn], pc: u32, fast: bool) -> u32 {
    for (i, instruction) in instructions.iter_mut().enumerate() {
        instruction.off = i as u32;
    }
    let key = (pc, instructions.len(), fast);
    if let Some(&id) = cc.by_pc.get(&key) {
        let b = &mut cc.blocks[id as usize];
        // PC alone is not identity: self-modifying code and new observer boundaries
        // must never resurrect stale code. Compare every decoded field, including raw.
        if b.instructions
            .iter()
            .zip(instructions.iter())
            .all(|(a, b)| a.insn == b.insn && a.max_ar == b.max_ar)
        {
            b.generation = cc.generation;
            // shell-s1: the rebuild may map this PC to other code pages.
            cc.recs[id as usize].looped.set(LOOP_UNKNOWN);
            return id;
        }
    }
    let id = cc.blocks.len() as u32;
    let mut at = pc;
    let pcs: Vec<u32> = instructions
        .iter()
        .map(|i| {
            let old = at;
            at = at.wrapping_add(i.insn.len as u32);
            old
        })
        .collect();
    cc.recs.push(Rec { slot: Cell::new(NONE), pc, n: pcs.len() as u32, looped: Cell::new(LOOP_UNKNOWN), hot: Cell::new(Hot::NONE), none: Cell::new(u64::MAX) });
    cc.blocks.push(Block {
        pcs,
        // Explicit loop-state writes break the LCOUNT-delta accounting used for retained
        // prefixes. A terminal WSR/XSR LEND can also create a new helper-side backedge.
        loop_prefix: if instructions.iter().any(|i| matches!(i.insn.op, crate::Op::Loop | crate::Op::Loopnez | crate::Op::Loopgtz)
            || (matches!(i.insn.op, crate::Op::Wsr | crate::Op::Xsr)
                && matches!(i.insn.imm as u32, crate::state::sr::LBEG | crate::state::sr::LEND | crate::state::sr::LCOUNT))) { 0 }
            else { instructions.iter().take_while(|i| emitter::loop_safe(i.insn.op, fast)).count() },
        instructions: instructions.to_vec(),
        pc,
        fast,
        lend_hint: Cell::new(0),
        generation: cc.generation,
        hits: Cell::new(0),
        bytes: Cell::new(0),
        region: RefCell::new(None),
        region_tries: Cell::new(0),
        covered_by: Cell::new((NONE, 0)),
        uncovered_epoch: Cell::new(u64::MAX),
    });
    cc.by_pc.insert(key, id);
    id
}
#[inline]
pub fn ready(cc: &CodeCache, code: u32, lend: u32) -> bool {
    let slot = cc.recs[code as usize].slot.get();
    if slot == NONE { prepare(&cc.blocks[code as usize], &cc.recs[code as usize].slot, lend) } else { slot != 0 }
}

#[cold]
#[inline(never)]
fn prepare(b: &Block, slot_out: &Cell<u32>, lend: u32) -> bool {
    let hits = b.hits.get() + 1;
    b.hits.set(hits);
    if hits < HOT { return false; }
    // LEND persists after its loop: a loop-body block keeps seeing the same value.
    if b.instructions.iter().zip(&b.pcs).any(|(i, pc)| pc.wrapping_add(i.insn.len as u32) == lend) { b.lend_hint.set(lend); }
    let bytes = generate(b);
    // SAFETY: The host synchronously copies these bytes, installs a module using the
    // shared memory/table, and returns a correctly typed function slot or zero.
    let slot = unsafe { host_jit_compile(bytes.as_ptr(), bytes.len()) };
    slot_out.set(slot);
    b.bytes.set(if slot == 0 { 0 } else { bytes.len() });
    slot != 0
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Helpers {
    exec: *const (),
    overflow: *const (),
    fused: *const (),
    loop_end: u32,
    version_ptrs: [*const u32; 2],
    versions: [u32; 2],
    #[cfg(feature = "wasm-cache-inline")]
    cache: *const emu_core::bus::FastCache,
}
impl Helpers {
    pub const fn new<B: Bus>() -> Self {
        Self {
            exec: h_exec::<B> as *const (),
            overflow: h_overflow as *const (),
            fused: h_fused as *const (),
            loop_end: 0,
            version_ptrs: [std::ptr::null(); 2],
            versions: [0; 2],
            #[cfg(feature = "wasm-cache-inline")]
            cache: std::ptr::null(),
        }
    }
    pub fn shared<B: Bus>() -> &'static Self {
        &const { Helpers::new::<B>() }
    }
}
const _: () = {
    assert!(size_of::<Helpers>() == if cfg!(feature = "wasm-cache-inline") { 36 } else { 32 });
    assert!(offset_of!(Helpers, overflow) == 4);
    assert!(offset_of!(Helpers, fused) == 8);
};
// Baseline WASM has no fused multiply-add opcode. Preserve Rust's single rounding
// without spilling integer register locals or invoking the instruction dispatcher.
extern "C" fn h_fused(s: u32, t: u32, r: u32, subtract: u32) -> u32 {
    let s = f32::from_bits(s ^ if subtract != 0 { 0x8000_0000 } else { 0 });
    s.mul_add(f32::from_bits(t), f32::from_bits(r)).to_bits()
}
extern "C" fn h_exec<B: Bus>(
    cpu: *mut Cpu,
    bus: *mut B,
    instruction: *const BlockInsn,
    pc: u32,
) -> u32 {
    // SAFETY: The compiled caller passes the exclusive live CPU/bus and an instruction
    // owned by its live CodeCache. No Rust execution overlaps generated access.
    let (cpu, bus, instruction) = unsafe { (&mut *cpu, &mut *bus, &*instruction) };
    // A return that does not trap changes only the window and the PC: nothing the dispatcher
    // would re-derive (interrupt inputs, waiting, device state) before the next block.
    if !matches!(instruction.insn.op, crate::Op::Retw | crate::Op::RetwN | crate::Op::Ret | crate::Op::RetN) {
        cpu.jit_helped = true;
    }
    cpu.pc = pc;
    #[cfg(feature = "wasm-jit-profile")]
    {
        let mut c = crate::census::get();
        let core = crate::census::core(cpu);
        let i = &instruction.insn;
        use crate::Op::*;
        let addr = match i.op {
            L8ui | L16ui | L16si | L32i | L32iN | S8i | S16i | S32i | S32iN | Lsi | Ssi => Some(cpu.get_ar(i.s).wrapping_add(i.imm as u32)),
            L32r => Some(i.imm as u32),
            Pie => { let p = &crate::pie::OPS[i.imm as usize]; let o = crate::pie::extract(i.raw, p); o.has(crate::pie::Role::As).then(|| cpu.get_ar(o.get(crate::pie::Role::As) as u8)) }
            _ => None,
        };
        let nm = crate::census::name(i);
        *c.helpers.entry((core, nm.clone(), addr.map_or(0, |a| a >> 16))).or_default() += 1;
        if let Some(a) = addr { *c.slowmem.entry((core, nm, crate::census::region(a))).or_default() += 1; }
    }
    if crate::exec::defer_instruction(cpu, bus, &instruction.insn) {
        cpu.jit_trap = None;
        return 1;
    }
    bus.note_pc(pc);
    match exec_insn(cpu, bus, &instruction.insn) {
        Ok(()) => {
            if cpu.price_control {
                let taken = crate::exec::control_taken(cpu, &instruction.insn);
                cpu.timing_extra += crate::exec::control_price(instruction.insn.op, taken)
                    + u32::from(taken && crate::exec::transfers(instruction.insn.op) && crate::exec::straddles(bus, cpu.pc));
            }
            (bus.block_break() as u32) << 1
        }
        Err(t) => {
            cpu.jit_trap = Some(t);
            1
        }
    }
}
extern "C" fn h_overflow(cpu: *mut Cpu, max_ar: u32, pc: u32) -> u32 {
    // SAFETY: The generated caller has exclusive access to this CPU.
    let cpu = unsafe { &mut *cpu };
    cpu.pc = pc;
    match cpu.check_overflow(max_ar as u8) {
        Some(t) => {
            cpu.jit_trap = Some(t);
            1
        }
        None => 0,
    }
}

/// A loop may repeat only across an ordinary instruction boundary with no observer.
/// The decoder already cuts at interior observers; the loop head needs its own check.
/// Returns the retained prefix length and the code pages of the block's first and last byte.
#[inline(always)]
pub fn loop_len<B: Bus>(cc: &CodeCache, code: u32, cpu: &Cpu, bus: &mut B) -> Option<(usize, [u32; 2])> {
    let r = &cc.recs[code as usize];
    if cpu.lcount == 0 || cpu.lbeg != r.pc || cpu.blocks.observed
        || cpu.boundary_bloom & emu_core::core::pc_bit(r.pc) != 0 {
        return None;
    }
    // shell-s1: the answer depends only on LEND and this block's immutable instructions.
    let (lend, n, pages) = r.looped.get();
    let (n, pages) = if lend == cpu.lend && n != u32::MAX { (n, pages) } else { loop_learn(cc, code, cpu.lend, bus) };
    (n != 0).then_some((n as usize, pages))
}
#[cold]
#[inline(never)]
fn loop_learn<B: Bus>(cc: &CodeCache, code: u32, lend: u32, bus: &mut B) -> (u32, [u32; 2]) {
    let b = &cc.blocks[code as usize];
    let n = b.instructions.iter().zip(&b.pcs).take(b.loop_prefix)
        .position(|(i, pc)| pc.wrapping_add(i.insn.len as u32) == lend)
        .map_or(0, |n| n as u32 + 1);
    let last = b.pcs.last().unwrap().wrapping_add(b.instructions.last().unwrap().insn.len as u32 - 1);
    let pages = if n == 0 { [0; 2] } else { [bus.code_page(b.pc), bus.code_page(last)] };
    cc.recs[code as usize].looped.set((lend, n, pages));
    (n, pages)
}

/// EX168 t4 (EX030 retry be64c0e7): shadow `$h` with the cache view for one wrapper call (`run`,
/// lane-s1 `resume`); copy the 36-byte table only when a view exists, since constructors leave
/// `cache` null, which is what the copy would have stored.
macro_rules! decorate {
    ($bus:ident, $h:ident) => {
        #[cfg(feature = "wasm-cache-inline")]
        let cache_view = if CACHE_PROBES.load(std::sync::atomic::Ordering::Relaxed) { $bus.fast_cache() } else { None };
        #[cfg(feature = "wasm-cache-inline")]
        let hinted;
        #[cfg(feature = "wasm-cache-inline")]
        debug_assert!($h.cache.is_null(), "a wrapper call requires an undecorated helper table");
        #[cfg(feature = "wasm-cache-inline")]
        let $h = if let Some(cache) = cache_view.as_ref() { hinted = Helpers { cache, ..*$h }; &hinted } else { $h };
    };
}

/// Execute a published block against the exclusively borrowed machine state.
/// Returns retired count in bits 0..16 and exit code in bits 16..19. For CODE_CUT,
/// bits 19..32 carry the next instruction index in the decoded block.
///
/// # Safety
/// `code` must be ready in this cache; `entry` must be its recorded instruction index.
/// `h` must have been created for B and be undecorated (its cache pointer is null).
/// FastMem must describe this bus and remain valid.
#[cfg_attr(feature = "wasm-cpu-profile", inline(never))]
pub unsafe fn run<B: Bus>(
    cc: &CodeCache,
    code: u32,
    cpu: &mut Cpu,
    bus: &mut B,
    h: &Helpers,
    budget: u32,
    entry: u32,
    fm: Option<FastMem>,
) -> u32 {
    let priced_fetch = cpu.price_control && cpu.icache_fill != 0;
    cpu.fetch_n = 0;
    let budget = if cpu.icache_fill != 0 { budget.min(64) } else { budget };
    // SAFETY: preserve the caller's live code, helper and memory guarantees.
    cpu.jit_helped = false;
    cpu.blocks.chain_ei = NONE;
    cpu.blocks.bridged = 0;
    // inner-s1: the helper table and the fast-memory pointers hold for the whole chain (it stops
    // once a helper ran), so each hop's run_inner no longer re-derives them.
    decorate!(bus, h);
    let (tlb, versions) = tables(fm);
    // A dispatch at a probed PC stays one block long, as the differential suite requires.
    let chain = cpu.boundary_bloom & emu_core::core::pc_bit(cpu.pc) == 0;
    let mut result = unsafe { run_inner(cc, code, cpu, bus, h, budget, entry, tlb, versions) };
    if chain { result = unsafe { chain_on(cc, cpu, bus, h, budget, result, tlb, versions) }; }
    #[cfg(feature = "wasm-jit-profile")]
    {
        let st = &cc.region_stats.ex153;
        let add = |i: usize, n: u64| st[i].set(st[i].get() + n);
        add(0, 1);
        if budget >= 64 { add(1, 1); }
        add(10, budget as u64);
        if result & 0xffff == 0 { add(9, 1); }
    }
    if priced_fetch {
        let n = cpu.fetch_n.saturating_sub(u32::from(bus.deferred()));
        assert!(n <= 64, "compiled fetch trace exceeded its instruction budget");
        for index in 0..n as usize {
            let [lo, hi] = cpu.fetch_ring[index];
            cpu.touch_fetch_lines(lo, hi);
        }
    }
    result
}

/// EX153: keep going inside this wrapper while nothing the dispatcher would look at can have
/// changed: a plain END/LEFT exit, no interpreter helper ran, credit remains, and the next PC
/// has a valid decoded entry with ready code that may start without a boundary check.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
unsafe fn chain_on<B: Bus>(cc: &CodeCache, cpu: &mut Cpu, bus: &mut B, h: &Helpers, budget: u32, mut result: u32, tlb: *const TlbEntry, versions: *mut u32) -> u32 {
    let mut total = 0u32;
    loop {
        let exit = (result >> 16) & 7;
        let sofar = total + (result & 0xffff);
        if (exit != CODE_END && exit != CODE_LEFT) || sofar >= budget || cpu.blocks.observed
            || cpu.jit_helped { break; }
        let pc = cpu.pc;
        if cpu.boundary_bloom & emu_core::core::pc_bit(pc) != 0 { break; }
        let Some((ei, next)) = cpu.blocks.chain_target(pc, bus.page_versions()) else {
            // EX171: a tiny block the emitter does not admit is interpreted right here when it is
            // pure register/branch work, instead of ending the chain and costing two dispatches.
            if crate::block::BRIDGE_CLASS != 0 && !cpu.price_control {
                if let Some((start, n)) = cpu.blocks.bridge_target(pc, bus.page_versions(), budget - sofar) {
                    total = sofar;
                    result = crate::block::bridge(cpu, bus, start, n);
                    continue;
                }
            }
            break
        };
        let slot = cc.recs[next as usize].slot.get();
        if slot == NONE || slot == 0 { break; }
        total = sofar;
        cpu.blocks.chain_ei = ei;
        #[cfg(feature = "wasm-jit-profile")]
        { let st = &cc.region_stats.ex153; st[11].set(st[11].get() + 1); }
        // SAFETY: the entry is valid for the current code pages and its code is ready in this cache.
        result = unsafe { run_inner(cc, next, cpu, bus, h, budget - total, 0, tlb, versions) };
    }
    result + total
}

/// lane-s1: the dispatch a quantum-ending region exit left for this core, straight into that region
/// at its recorded parameter, with the checks the Hot path makes for an entry there (live epochs,
/// pages, probes, hardware loop, a whole chunk of credit), then the EX153 chain. The caller has
/// checked interrupts, waiting and the PC. `None` (the memo is dropped): take the ordinary path.
/// # Safety
/// As for `run`.
pub unsafe fn resume<B: Bus>(cc: &CodeCache, cpu: &mut Cpu, bus: &mut B, h: &Helpers, budget: u32, fm: Option<FastMem>) -> Option<u32> {
    type Run<B> =
        extern "C" fn(*mut Cpu, *mut B, *const Helpers, u32, u32, *const TlbEntry, *mut u32) -> u32;
    let (_, code, param, epoch, _) = cpu.blocks.memo;
    cpu.blocks.memo.0 = 1;
    // SAFETY: the epoch proves `code` indexes the same record; no refill runs while this is read.
    let live = epoch == cc.region_epoch.get();
    let hot = if live { unsafe { &*cc.recs[code as usize].hot.as_ptr() } } else { &Hot::NONE };
    let pv = bus.page_versions();
    // Hot facts stamped in this epoch describe the region the memo came from (memo-s2: a memo is only
    // recorded with the epoch its Hot facts carry, and they change only with it, so the region epoch
    // alone proves them); a chunk-head entry needs credit for the whole chunk (at most MAX_LEN), a
    // guarded copy cuts where credit ends.
    // lane-s1l: a chunk head admits an active loop of the region's own, as the slow path does.
    // loop-s1: a copy admits its chunk's own loop, as the Hot path does; these facts are that
    // chunk's exactly when the copy is theirs.
    if !live || hot.fepoch != bus.stable_pages().2 || cpu.boundary_bloom & hot.bloom != 0
        || (cpu.lcount != 0 && cpu.lend.wrapping_sub(hot.lo) <= hot.span && !if param >> 16 == 0 {
            // SAFETY: the epoch proves the owning region, and so this vector, is live.
            unsafe { std::slice::from_raw_parts(hot.loops, hot.nloops as usize) }.contains(&(cpu.lend, cpu.lbeg))
        } else { param & 0xffff == hot.copy && hot.lp == Some((cpu.lend, cpu.lbeg)) })
        || (param >> 16 == 0 && budget < crate::block::MAX_LEN as u32)
        || !hot.pages[..hot.npages as usize].iter().all(|&(i, v)| pv.get(i as usize).copied().unwrap_or(0) == v) {
        #[cfg(feature = "wasm-jit-profile")]
        { let st = &cc.region_stats.ex153; st[13].set(st[13].get() + 1); }
        #[cfg(feature = "wasm-jit-tests")]
        REGION_STATS[15].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return None;
    }
    cpu.jit_helped = false;
    cpu.blocks.chain_ei = NONE;
    cpu.blocks.bridged = 0;
    decorate!(bus, h);
    let (tlb, versions) = tables(fm);
    let (slot, sites, nsites) = (hot.slot, hot.sites, hot.nsites);
    // SAFETY: the epoch proves the region, its slot and its sites are live.
    let f: Run<B> = unsafe { std::mem::transmute(slot as usize) };
    let mut result = f(cpu, bus, h, budget.min(0xffff), param, tlb, versions);
    // window or coprocessor state refused the region before it ran anything
    if (result >> 16) & 7 == CODE_REJECT { return None; }
    // As after find_block: this dispatch consumes the lookup hints (nothing read them since) and may
    // leave new ones.
    (cpu.blocks.resume.2, cpu.blocks.alias_pc) = (1, 1);
    let sites = unsafe { std::slice::from_raw_parts(sites, nsites as usize) };
    let (short, cut) = private_exit(&mut result, budget);
    let t = (result >> 19) as usize;
    region_stats(cc, result, budget, Some(sites[t]));
    // before note_short, which may regenerate the region and free these sites
    exit_site(cpu, bus, sites, t, cut, code, epoch);
    if short { note_short(cc, code, slot, cpu.pc); }
    #[cfg(feature = "wasm-jit-profile")]
    { let st = &cc.region_stats.ex153; st[12].set(st[12].get() + 1); }
    #[cfg(feature = "wasm-jit-tests")]
    { REGION_STATS[14].fetch_add(1, std::sync::atomic::Ordering::Relaxed); if param >> 16 != 0 { REGION_STATS[13].fetch_add(1, std::sync::atomic::Ordering::Relaxed); } }
    Some(unsafe { chain_on(cc, cpu, bus, h, budget, result & 0x7ffff, tlb, versions) })
}

/// lane-s1: keep a site's resume parameter only for a chunk this region owns in the coverage map,
/// the region an ordinary dispatch there enters; elsewhere the memo would move execution into
/// another region's code (measured: +12% generated bytes on fluidbox).
fn own_resumes(r: &mut Region, owner: u32, covered: &HashMap<u32, (u32, u32)>) {
    for site in &mut r.sites {
        #[cfg(not(feature = "wasm-jit-profile"))]
        let p = &mut site.1;
        #[cfg(feature = "wasm-jit-profile")]
        let p = &mut site.2;
        if *p == NONE { continue; }
        let k = if *p >> 16 == 0 { *p } else { r.copies.iter().position(|&c| c == *p & 0xffff).unwrap() as u32 };
        if covered.get(&r.chunks[k as usize].pc) != Some(&(owner, k)) { *p = NONE; }
    }
}

/// EX173 s1: a bus without fast memory gets a table where every entry is EMPTY instead of a
/// null pointer, so generated accesses reject it with the range test they already run and no
/// per-access null test is emitted. No probe can succeed, so `versions` is never dereferenced.
#[inline(always)]
fn tables(fm: Option<FastMem>) -> (*const TlbEntry, *mut u32) {
    static NO_FAST_MEM: [TlbEntry; crate::bus::TLB_ENTRIES] = [TlbEntry::EMPTY; crate::bus::TLB_ENTRIES];
    fm.map(|m| (m.tlb, m.page_ver)).unwrap_or((NO_FAST_MEM.as_ptr(), std::ptr::null_mut()))
}

#[allow(clippy::too_many_arguments)]
unsafe fn run_inner<B: Bus>(
    cc: &CodeCache,
    code: u32,
    cpu: &mut Cpu,
    bus: &mut B,
    h: &Helpers,
    budget: u32,
    entry: u32,
    tlb: *const TlbEntry,
    versions: *mut u32,
) -> u32 {
    type Run<B> =
        extern "C" fn(*mut Cpu, *mut B, *const Helpers, u32, u32, *const TlbEntry, *mut u32) -> u32;
    let rec = &cc.recs[code as usize];
    if !cpu.blocks.observed {
        // EX136: the facts the checks below would fetch through the owning block, its region and
        // three of its vectors are cached in this block while no region has been dropped.
        // Read cached admission facts in place instead of copying the entire descriptor.
        // End the borrow before entering generated code or updating the cached descriptor.
        let mut rejected;
        'facts: {
            // SAFETY: the only writer, the refill below, runs after this reference's scope.
            let hot = unsafe { &*rec.hot.as_ptr() };
            let stable = bus.stable_pages();
            #[cfg(any(debug_assertions, feature = "wasm-jit-tests"))]
            if hot.epoch == cc.region_epoch.get() && hot.fepoch == stable.2 {
                // The slow lookup keeps a live hint even if the coverage map now
                // prefers another overlapping owner. Validate the selected region.
                let (owner, k) = if cc.blocks[code as usize].region.borrow().is_some() { (code, 0) }
                    else { cc.blocks[code as usize].covered_by.get() };
                let region = cc.blocks[owner as usize].region.borrow();
                let r = region.as_ref().expect("live hot region");
                assert_eq!(r.chunks[k as usize].pc, rec.pc);
                hot.assert_matches(r, k, stable);
            }
            let live = hot.epoch == cc.region_epoch.get() && hot.fepoch == stable.2;
            // hop-s2: a block no region can run skips the admission facts and the slow lookup.
            if !live && rec.none.get() == cc.coverage_epoch.get() {
                #[cfg(any(debug_assertions, feature = "wasm-jit-tests"))]
                {
                    let b = &cc.blocks[code as usize];
                    assert!(b.region.borrow().is_none() && b.region_tries.get() >= REGION_TRIES && !cc.covered.borrow().contains_key(&b.pc));
                }
                #[cfg(feature = "wasm-jit-tests")]
                REGION_STATS[17].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                rejected = true;
                break 'facts;
            }
            // EX168 s1: `fits` false with current pages is a proven rejection: while the epoch holds,
            // the slow lookup below selects this same live chunk and fails the same budget/bloom test.
            // inner-s1: one bloom test for both the credit and the resume admission.
            let clear = cpu.boundary_bloom & hot.bloom == 0;
            let fits = budget >= hot.len && clear;
            // loop-s1: the chunk's own region loop active is as good as none: its LEND ends a chunk,
            // where the region takes the backedge itself.
            let unlooped = cpu.lcount == 0 || cpu.lend.wrapping_sub(hot.lo) > hot.span || hot.lp == Some((cpu.lend, cpu.lbeg));
            // tails-s2 (EX182 s2): a resume inside a chunk with a guarded copy enters the copy at
            // `entry`, whose STOP cuts wherever the credit ends. A resume never forms or enters a
            // region on the slow path below; that only re-stamps stale facts of a covered head.
            let resumable = entry < hot.len && clear && unlooped;
            let enter = if entry == 0 { !fits || unlooped } else { hot.copy != 0 && resumable };
            rejected = entry != 0 && (live || {
                let b = &cc.blocks[code as usize];
                b.covered_by.get().0 == NONE && b.region.borrow().is_none()
            });
            // edge-s1: a resume the own module must take (no copy yet) also marks where quanta end.
            if live && entry != 0 && hot.counting && resumable { note_short(cc, code, hot.slot, rec.pc); }
            if live && enter {
                let pv = bus.page_versions();
                if hot.pages[..hot.npages as usize].iter().all(|&(i, v)| pv.get(i as usize).copied().unwrap_or(0) == v) {
                  if entry == 0 && !fits { rejected = true; } else {
                    // SAFETY: as for the region call below; the epoch proves slot and sites are live.
                    let (slot, sites, nsites) = (hot.slot, hot.sites, hot.nsites);
                    let param = if entry == 0 { hot.k } else { hot.copy | entry << 16 };
                    let epoch = hot.epoch;
                    #[cfg(feature = "wasm-jit-tests")]
                    if entry != 0 { REGION_STATS[13].fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
                    #[cfg(feature = "wasm-jit-tests")]
                    if cpu.lcount != 0 && hot.lp == Some((cpu.lend, cpu.lbeg)) { REGION_STATS[16].fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
                    let f: Run<B> = unsafe { std::mem::transmute(slot as usize) };
                    let mut result = f(cpu, bus, h, budget.min(0xffff), param, tlb, versions);
                    // SAFETY: the epoch proves the owning region, and so this vector, is live.
                    let (short, cut) = private_exit(&mut result, budget);
                    let site = if (result >> 16) & 7 != CODE_REJECT {
                        assert!((result >> 19) < nsites);
                        // SAFETY: index checked against the live vector's length.
                        Some(unsafe { *sites.add((result >> 19) as usize) })
                    } else { None };
                    region_stats(cc, result, budget, site);
                    if site.is_some() { census(0, 1); census(1, (result & 0xffff) as u64); }
                    // before note_short, which may regenerate the region and free these sites
                    if site.is_some() { exit_site(cpu, bus, unsafe { std::slice::from_raw_parts(sites, nsites as usize) }, (result >> 19) as usize, cut, code, epoch); }
                    if short { note_short(cc, code, slot, cpu.pc); }
                    if site.is_some() { return result & 0x7ffff; }
                    return run_block_body(cc, code, cpu, bus, h, budget, entry, tlb, versions);
                  }
                }
            }
        }
        if !rejected {
        let b = &cc.blocks[code as usize];
        // The region to run: this block's own, or the one covering this PC.
        let (owner, k) = if b.region.borrow().is_some() {
            (code, 0)
        } else {
            let cached = b.covered_by.get();
            let live = cached.0 != NONE
                && cc.blocks.get(cached.0 as usize).and_then(|o| o.region.borrow().as_ref()
                    .map(|r| r.chunks.get(cached.1 as usize).is_some_and(|c| c.pc == b.pc))).unwrap_or(false);
            // A temporary borrow in an `if let` would outlive the whole chain.
            let epoch = cc.coverage_epoch.get();
            let found = if live || b.uncovered_epoch.get() == epoch {
                None
            } else {
                let found = cc.covered.borrow().get(&b.pc).copied();
                if found.is_none() { b.uncovered_epoch.set(epoch); }
                found
            };
            if live {
                cached
            } else if let Some(found) = found {
                b.covered_by.set(found);
                #[cfg(feature = "wasm-jit-profile")]
                cc.region_stats.covered.set(cc.region_stats.covered.get() + 1);
                #[cfg(feature = "wasm-jit-tests")]
                REGION_STATS[10].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                found
            } else if entry == 0 && b.region_tries.get() < REGION_TRIES {
                b.region_tries.set(b.region_tries.get() + 1);
                let mut formed = emitter::region::form(cpu, bus, b.pc, &b.instructions, b.fast).and_then(|f| {
                    let (bytes, sites) = emitter::region::generate(&f.chunks, &f.pages, &f.loops, &f.leaves, b.fast, None);
                    // SAFETY: as for ready(): the host copies and installs the module.
                    let slot = unsafe { host_jit_compile(bytes.as_ptr(), bytes.len()) };
                    (slot != 0).then(|| Region {
                        lens: f.chunks.iter().map(|c| c.instructions.len() as u32).collect(),
                        short: vec![0; f.chunks.len()], tunes: 0,
                        copies: vec![0; f.chunks.len()],
                        chunks: f.chunks, leaves: f.leaves, slot, bytes: bytes.len(), bloom: f.bloom, lo: f.lo, hi: f.hi, loops: f.loops, pages: f.pages, sites,
                    })
                });
                if let Some(r) = &mut formed {
                    // Removal cannot invalidate a negative lookup; insertion can.
                    cc.coverage_epoch.set(cc.coverage_epoch.get().wrapping_add(1));
                    let mut covered = cc.covered.borrow_mut();
                    for (k, c) in r.chunks.iter().enumerate() { covered.entry(c.pc).or_insert((code, k as u32)); }
                    own_resumes(r, code, &covered);
                    #[cfg(feature = "wasm-jit-profile")]
                    {
                        let st = &cc.region_stats;
                        st.formed.set(st.formed.get() + 1);
                        st.chunks.set(st.chunks.get() + r.chunks.len() as u64);
                        st.instructions.set(st.instructions.get() + r.chunks.iter().map(|c| c.instructions.len() as u64).sum::<u64>());
                        st.bytes.set(st.bytes.get() + r.bytes as u64);
                    }
                }
                #[cfg(feature = "wasm-jit-profile")]
                if formed.is_none() { cc.region_stats.failed.set(cc.region_stats.failed.get() + 1); }
                #[cfg(feature = "wasm-jit-tests")]
                REGION_STATS[if formed.is_some() { 0 } else { 1 }].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                *b.region.borrow_mut() = formed;
                (code, 0)
            } else {
                // hop-s2: nothing here can change before a coverage insertion (EX168 t1's verdict).
                if b.region_tries.get() >= REGION_TRIES { rec.none.set(epoch); }
                (NONE, 0)
            }
        };
        if owner != NONE {
            let rb = &cc.blocks[owner as usize];
            let region = rb.region.borrow();
            if let Some(r) = region.as_ref() {
                let pv = bus.page_versions();
                let current = r.pages.iter().all(|&(i, v)| pv.get(i as usize).copied().unwrap_or(0) == v);
                if !current {
                    // Some chunk's code changed: rebuild the region from the new code later.
                    drop(region);
                    rb.drop_region(owner, &cc.covered);
                    cc.region_epoch.set(cc.region_epoch.get() + 1);
                    #[cfg(feature = "wasm-jit-profile")]
                    cc.region_stats.dropped.set(cc.region_stats.dropped.get() + 1);
                    #[cfg(feature = "wasm-jit-tests")]
                    REGION_STATS[9].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else {
                // EX168 t3: facts stamped with the current epoch were copied from this same live
                // (owner, chunk) and a region's facts never change: nothing to rewrite. tails-s2:
                // stamped before the entry test, so a later resume finds them.
                let (lo, hi, fepoch) = bus.stable_pages();
                // SAFETY: a copied field; no reference into `hot` is live here.
                let old = unsafe { ((*rec.hot.as_ptr()).epoch, (*rec.hot.as_ptr()).fepoch) };
                if r.pages.len() <= emitter::region::MAX_PAGES && old != (cc.region_epoch.get(), fepoch) {
                    // shell-s2: every page was compared just above; the pages the bus vouches for stay
                    // current while its epoch holds, so only the others are compared from now on.
                    let mut pages = [(0, 0); emitter::region::MAX_PAGES];
                    let mut npages = 0;
                    for &(i, v) in &r.pages { if i < lo || i >= hi { pages[npages] = (i, v); npages += 1; } }
                    rec.hot.set(Hot { epoch: cc.region_epoch.get(), bloom: r.bloom, slot: r.slot, k, len: r.lens[k as usize], lo: r.lo,
                        span: r.hi.wrapping_sub(r.lo), pages, npages: npages as u32, nsites: r.sites.len() as u32, sites: r.sites.as_ptr(), loops: r.loops.as_ptr(), nloops: r.loops.len() as u32,
                        copy: r.copies[k as usize], fepoch, counting: !r.short.is_empty() && r.lens[k as usize] > 1 && r.copies[k as usize] == 0,
                        lp: r.chunk_loop(k) });
                }
                if entry == 0
                    && budget >= r.lens[k as usize]
                    && cpu.boundary_bloom & r.bloom == 0
                    && (cpu.lcount == 0
                        || cpu.lend.wrapping_sub(r.lo) > r.hi.wrapping_sub(r.lo)
                        || r.loops.contains(&(cpu.lend, cpu.lbeg)))
                {
                    // SAFETY: the region was installed with the block signature; its
                    // entry parameter is the chunk index.
                    let f: Run<B> = unsafe { std::mem::transmute(r.slot as usize) };
                    #[cfg(any(debug_assertions, feature = "wasm-jit-tests"))]
                    unsafe { &*rec.hot.as_ptr() }.assert_matches(r, k, (lo, hi, fepoch));
                    let mut result = f(cpu, bus, h, budget.min(0xffff), k, tlb, versions);
                    // memo-s2: the facts were stamped above unless the region has too many pages;
                    // epoch 0 never matches, so such a memo only replays its hints.
                    let epoch = if r.pages.len() <= emitter::region::MAX_PAGES { cc.region_epoch.get() } else { 0 };
                    let (short, cut) = private_exit(&mut result, budget);
                    let site = if (result >> 16) & 7 != CODE_REJECT {
                        assert!(((result >> 19) as usize) < r.sites.len(), "region {:x}: result {result:#x} sites {}", rb.pc, r.sites.len());
                        Some(r.sites[(result >> 19) as usize])
                    } else { None };
                    region_stats(cc, result, budget, site);
                    if site.is_some() { census(0, 1); census(1, (result & 0xffff) as u64); }
                    if site.is_some() { exit_site(cpu, bus, &r.sites, (result >> 19) as usize, cut, code, epoch); }
                    if short {
                        let slot = r.slot;
                        drop(region);
                        note_short(cc, code, slot, cpu.pc);
                    }
                    if site.is_some() { return result & 0x7ffff; }
                }
                }
            }
        }
        }
    }
    run_block_body(cc, code, cpu, bus, h, budget, entry, tlb, versions)
}

/// tails-s1: report SHORT and TAIL as CODE_LEFT. True for a SHORT exit that left credit
/// unspent, which the wrapper would spend in the target's own module. A TAIL exit names the
/// chunk head, so the next dispatch resumes inside that block rather than decoding a new
/// head mid-chunk (EX172's alias finds it, or decodes it after a flush).
#[inline(always)]
fn private_exit(result: &mut u32, budget: u32) -> (bool, bool) {
    let code = (*result >> 16) & 7;
    if code < CODE_SHORT { return (false, false); }
    *result ^= (code ^ CODE_LEFT) << 16;
    if code == CODE_TAIL { return (false, true); }
    (*result & 0xffff < budget, false)
}

/// The exit side of a region call that returned through site `t` (`cut`: a guarded copy's TAIL):
/// the last retired PC for the bus, then either the lane-s1 memo for the next dispatch or the lookup
/// hints. memo-s2: a memo'd exit leaves its hints to a dispatch that declines the memo (`memo_hints`);
/// only a dispatch at this PC reads them, and a memo hit overwrites them unread.
#[inline(always)]
fn exit_site<B: Bus>(cpu: &mut Cpu, bus: &mut B, sites: &[ExitSite], t: usize, cut: bool, code: u32, epoch: u64) {
    let site = sites[t];
    bus.note_pc(site_pc(site));
    let param = site_resume(site);
    if param != NONE {
        cpu.blocks.memo = (cpu.pc, code, param, epoch, if cut { site_pc(sites[t + 1]) } else { site_pc(site) });
        return;
    }
    if cut { crate::block::tail_hints(cpu, site_pc(sites[t + 1]), site_pc(sites[t + 2])); }
    crate::block::note_sequential(cpu, site_pc(site));
}

/// tails-s1: count a credit-short exit to chunk head `target` of the region in `slot`, reached
/// through block `code` (edge-s1: or an own-module resume there). After SHORT_SAMPLE of them,
/// regenerate the region with guarded copies of the chunks that caught at least 1/32, largest
/// first, up to COPY_ROOM instructions, so code grows only where quanta actually end. edge-s1r:
/// up to TUNES windows, each keeping the copies so far. The drop of the old module moves the
/// region epoch on like any drop.
#[cold]
#[inline(never)]
fn note_short(cc: &CodeCache, code: u32, slot: u32, target: u32) {
    let b = &cc.blocks[code as usize];
    let owner = if b.region.borrow().is_some() { code } else { b.covered_by.get().0 };
    let Some(rb) = cc.blocks.get(owner as usize) else { return };
    let mut region = rb.region.borrow_mut();
    let Some(r) = region.as_mut().filter(|r| r.slot == slot && !r.short.is_empty()) else { return };
    let Some(k) = r.chunks.iter().position(|c| c.pc == target) else { return };
    r.short[k] += 1;
    let total: u32 = r.short.iter().sum();
    if total < SHORT_SAMPLE { return; }
    let copied = |k: usize| r.copies[k] != 0;
    let mut room = COPY_ROOM - (0..r.lens.len()).filter(|&k| copied(k)).map(|k| r.lens[k]).sum::<u32>();
    let mut new: Vec<usize> = (0..r.short.len()).filter(|&k| !copied(k) && r.short[k] * 32 >= total && r.lens[k] > 1).collect();
    new.sort_by_key(|&k| std::cmp::Reverse(r.short[k]));
    new.retain(|&k| r.lens[k] <= room && { room -= r.lens[k]; true });
    let hot: Vec<usize> = (0..r.lens.len()).filter(|&k| copied(k) || new.contains(&k)).collect();
    r.tunes += 1;
    if r.tunes == TUNES { r.short.clear(); } else { r.short.fill(0); }
    // edge-s1: Hot facts change what they count when the epoch moves, also if nothing is added.
    cc.region_epoch.set(cc.region_epoch.get() + 1);
    if new.is_empty() { return; }
    let (bytes, sites) = emitter::region::generate(&r.chunks, &r.pages, &r.loops, &r.leaves, rb.fast, Some(&hot));
    let copies = emitter::region::copy_indices(&r.chunks, &hot).iter().map(|c| c.unwrap_or(0)).collect();
    // SAFETY: as for ready(): the host copies and installs the module; the old one is not running.
    let slot = unsafe { host_jit_compile(bytes.as_ptr(), bytes.len()) };
    if slot == 0 { return; }
    unsafe { host_jit_release(r.slot) };
    (r.slot, r.bytes, r.sites, r.copies) = (slot, bytes.len(), sites, copies);
    own_resumes(r, owner, &cc.covered.borrow());
    #[cfg(feature = "wasm-jit-profile")]
    {
        let st = &cc.region_stats;
        st.tuned.set(st.tuned.get() + 1);
        st.bytes.set(st.bytes.get() + r.bytes as u64);
    }
    #[cfg(feature = "wasm-jit-tests")]
    REGION_STATS[12].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Test and profile counters of one region call.
#[inline(always)]
#[allow(unused_variables)]
fn region_stats(cc: &CodeCache, result: u32, budget: u32, site: Option<ExitSite>) {
    #[cfg(feature = "wasm-jit-tests")]
    {
        use std::sync::atomic::Ordering::Relaxed;
        REGION_STATS[2].fetch_max(result & 0xffff, Relaxed);
        REGION_STATS[3 + ((result >> 16) & 7) as usize].fetch_add(1, Relaxed);
        REGION_STATS[11].fetch_max(budget, Relaxed);
    }
    #[cfg(feature = "wasm-jit-profile")]
    {
        let st = &cc.region_stats;
        st.calls.set(st.calls.get() + 1);
        let exit = ((result >> 16) & 7) as usize;
        if exit == CODE_REJECT as usize {
            st.rejected.set(st.rejected.get() + 1);
        } else {
            st.retired.set(st.retired.get() + (result & 0xffff) as u64);
            st.exits[exit].set(st.exits[exit].get() + 1);
            if exit == CODE_LEFT as usize {
                let kind = site.expect("a region LEFT exit has a site").1 as usize;
                st.left_kinds[kind].set(st.left_kinds[kind].get() + 1);
            }
        }
    }
}

/// EX168 s2: a by-reference panic closure forced entry, done, budget, looping, LCOUNT and the
/// result into stack memory on every call. Pass them by value to a cold function instead.
#[cold]
#[inline(never)]
#[allow(clippy::too_many_arguments)]
fn bad_offset(b: &Block, entry: u32, done: u32, budget: u32, looping: Option<usize>, initial_lcount: u32, lcount: u32, result: u32) -> ! {
    panic!("block {:x} {:?} entry {entry} done {done} budget {budget} looping {looping:?} lcount {initial_lcount}->{lcount} result {result:#x}",
        b.pc, b.instructions.iter().map(|i| i.insn.op).collect::<Vec<_>>())
}

/// The block's own module: whole, resumed or as a retained hardware loop.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
unsafe fn run_block_body<B: Bus>(cc: &CodeCache, code: u32, cpu: &mut Cpu, bus: &mut B, h: &Helpers, budget: u32, entry: u32, tlb: *const TlbEntry, versions: *mut u32) -> u32 {
    type Run<B> =
        extern "C" fn(*mut Cpu, *mut B, *const Helpers, u32, u32, *const TlbEntry, *mut u32) -> u32;
    let r = &cc.recs[code as usize];
    // SAFETY: host_jit_compile installs exactly this signature in the shared WASM table.
    let f: Run<B> = unsafe { std::mem::transmute(r.slot.get() as usize) };
    let looping = loop_len(cc, code, cpu, bus);
    let initial_lcount = cpu.lcount;
    let result = if let Some((_, pages)) = looping {
        let mut guarded = *h;
        let pv = bus.page_versions();
        if let (Some(a), Some(z)) = (pv.get(pages[0] as usize), pv.get(pages[1] as usize)) {
            guarded.loop_end = cpu.lend;
            guarded.version_ptrs = [a as *const u32, z as *const u32];
            guarded.versions = [*a, *z];
        }
        f(cpu, bus, &guarded, budget.min(0xffff), entry, tlb, versions)
    } else {
        f(cpu, bus, h, budget.min(0xffff), entry, tlb, versions)
    };
    let looping = looping.map(|(n, _)| n);
    let done = result & 0xffff;
    if cfg!(feature = "wasm-cpu-profile") {
        let b = &cc.blocks[code as usize];
        let bytes = b.pcs.last().unwrap().wrapping_add(b.instructions.last().unwrap().insn.len as u32).wrapping_sub(b.pc);
        let noloop = initial_lcount == 0 || cpu.lend.wrapping_sub(b.pc) > bytes;
        if entry == 0 && budget as usize >= b.instructions.len() { census(3, 1); census(4, done as u64); }
        else if noloop { census(5, 1); census(6, done as u64); if entry != 0 { census(2, done as u64); } }
        else { census(7, done as u64); }
    }
    // LCOUNT changes only at the admitted hardware backedge. Subtract repeated
    // prefixes to locate both the last retired instruction and a cut continuation.
    let repeated = looping.map_or(0, |n| (initial_lcount - cpu.lcount) as usize * n);
    let offset = (entry + done) as usize - repeated;
    if done > 0 {
        #[cfg(feature = "wasm-jit-profile")]
        if looping.is_some() {
            let retained = initial_lcount - cpu.lcount - u32::from(offset == 0);
            cpu.blocks.profile.record_loop(r.pc, retained);
        }
        // Offset zero means the last retired instruction took a hardware backedge.
        // The destination PC alone cannot prove that: a suffix branch may target LBEG.
        let last = if offset == 0 { looping.unwrap() - 1 } else { offset - 1 };
        let b = &cc.blocks[code as usize];
        let pc = match b.pcs.get(last) { Some(&pc) => pc, None => bad_offset(b, entry, done, budget, looping, initial_lcount, cpu.lcount, result) };
        bus.note_pc(pc);
        if result >> 16 != CODE_CUT && offset != 0 && offset < r.n as usize { crate::block::note_sequential(cpu, pc); }
    }
    #[cfg(feature = "wasm-jit-profile")]
    {
        let st = &cc.region_stats.ex153;
        let add = |i: usize, n: u64| st[i].set(st[i].get() + n);
        let cut = result >> 16 == CODE_CUT;
        if entry != 0 { add(6, 1); add(7, done as u64); if cut { add(8, 1); } }
        else if cut { add(4, 1); add(5, done as u64); }
        else { add(2, 1); add(3, done as u64); }
    }
    // Reuse the offset already reconstructed above instead of scanning decoded PCs
    // again in run_block_inner. Regions never return CODE_CUT.
    if result >> 16 == CODE_CUT {
        debug_assert_eq!(cc.blocks[code as usize].pcs[offset], cpu.pc);
        // lane-s1c: the next dispatch resumes here, which the Hot path takes into this chunk's
        // guarded copy; the memo lets it go there directly (every other check is made at use).
        // Not inside an active loop ending in the region unless it is the chunk's own (loop-s1): a
        // copy resume is refused there.
        // SAFETY: the only writer, the refill in run_inner, is not running.
        let hot = unsafe { &*r.hot.as_ptr() };
        if hot.epoch == cc.region_epoch.get() && hot.copy != 0 && offset != 0 && (offset as u32) < hot.len
            && (cpu.lcount == 0 || cpu.lend.wrapping_sub(hot.lo) > hot.span || hot.lp == Some((cpu.lend, cpu.lbeg))) {
            cpu.blocks.memo = (cpu.pc, code, hot.copy | (offset as u32) << 16, hot.epoch, 1);
        } else { cpu.blocks.memo.0 = 1; }
        result | ((offset as u32) << 19)
    } else { result }
}

/// EX171: RSR of a register whose `Cpu` field is exact mid-dispatch (EX135), so the interpreter
/// may run it inside a wrapper chain.
pub fn exact_rsr(n: u32) -> bool { emitter::rsr_field(n).is_some() }
#[cfg(feature = "wasm-jit-profile")]
pub fn census_supported(i: &crate::Insn, fast: bool) -> bool { emitter::supported_insn(i, fast) }
#[cfg(feature = "wasm-jit-profile")]
pub fn census_terminal(op: crate::Op) -> bool { emitter::terminal_helper(op) }
#[path = "wasm_emit.rs"]
mod emitter;
use emitter::generate;

#[cfg(feature = "wasm-jit-profile")]
#[path = "wasm_profile.rs"]
pub mod profile;

#[cfg(feature = "wasm-jit-tests")]
#[path = "wasm_tests.rs"]
pub mod tests;
