//! ESP8266 LX106 interpreter profile. Shared integer execution, separate architectural entry.
//!
//! Configuration source: Arduino ESP8266 3.1.2 (210897ef),
//! libraries/GDBStub/src/xtensa/config/core-isa.h, checked against PlatformIO
//! toolchain-xtensa 2.100300.220621 (GCC 10.3.0), include/xtensa/config/core-isa.h.
//! This adapter intentionally never enters the LX6/LX7 block caches or either JIT.
use crate::{decode, disasm, exec, Bus, Core, Cpu, Insn, Op, Trap};
use crate::state::{exc, ps, sr};
use emu_core::{StepKind, StepOutcome};

// core-isa.h:260-343,390-419. Reset selection 1 is the ESP8266 ROM entry
// (_ResetVector in Arduino 3.1.2 tools/sdk/ld/eagle.rom.addr.v6.ld).
const LEVEL: u32 = 0x3f;
const EDGE: u32 = 0x3f00;
const SOFTWARE: u32 = 0x80;
const TIMER: u32 = 0x40;
const NMI: u32 = 0x4000;
const LITBASE: u32 = 5;

/// Whether a decoded instruction belongs to the LX106 ISA (core-isa.h:49-94).
/// A positive list prevents future extensions from silently becoming LX106 instructions.
pub fn supports(i: &Insn) -> bool {
    use Op::*;
    match i.op {
        Nop | NopN | Break | BreakN | Syscall | Waiti | Rsil
        | Isync | Rsync | Esync | Dsync | Excw | Memw | Extw
        | J | Jx | Call0 | Callx0 | Ret | RetN | Rfe | Rfde
        | Beqz | Bnez | Bltz | Bgez | BeqzN | BnezN | Beqi | Bnei | Blti | Bgei | Bltui | Bgeui
        | Bnone | Beq | Blt | Bltu | Ball | Bbc | Bbci | Bany | Bne | Bge | Bgeu | Bnall | Bbs | Bbsi
        | L8ui | L16ui | L16si | L32i | L32iN | L32r | S8i | S16i | S32i | S32iN
        | Movi | MoviN | Mov | MovN | Add | AddN | Addi | AddiN | Addmi | Sub
        | Addx2 | Addx4 | Addx8 | Subx2 | Subx4 | Subx8 | And | Or | Xor | Neg | Abs | Extui
        | Moveqz | Movnez | Movltz | Movgez | Slli | Srai | Srli | Sll | Srl | Sra | Src
        | Ssr | Ssl | Ssa8l | Ssa8b | Ssai | Nsa | Nsau | Mull | Mul16u | Mul16s | Rer | Wer
        | Ritlb0 | Iitlb | Pitlb | Witlb | Ritlb1 | Rdtlb0 | Idtlb | Pdtlb | Wdtlb | Rdtlb1 => true,
        Rsr => readable_sr(i.imm as u32),
        Wsr => writable_sr(i.imm as u32),
        Xsr => exchangeable_sr(i.imm as u32),
        Rfi => matches!(i.imm, 2 | 3),
        _ => false,
    }
}

fn valid_sr(n: u32) -> bool {
    // No loops, windows, booleans, MAC, coprocessors, THREADPTR, MISC,
    // second debug comparator, or CCOMPARE1/2 (core-isa.h:49-94,264,428-429).
    matches!(n, 3 | 5 | 96 | 104 | 128 | 144 | 160 | 176..=179 | 192 | 194..=195
        | 208..=211 | 226..=228 | 230..=238 | 240)
}

// Xtensa ISA manual tables 5-159, 5-169..171, 5-181; QEMU v10.1.0
// target/xtensa/translate.c wsr/xsr.configid*, .debugcause, .prid and xsr.int*.
fn readable_sr(n: u32) -> bool {
    // INTCLEAR reads are architecturally undefined (table 5-171). Keep the
    // existing profile policy of rejecting them, not a fabricated read value.
    valid_sr(n) && n != sr::INTCLEAR
}

fn writable_sr(n: u32) -> bool {
    valid_sr(n) && !matches!(n, sr::CONFIGID0 | sr::CONFIGID1 | sr::DEBUGCAUSE | sr::PRID)
}

fn exchangeable_sr(n: u32) -> bool {
    // INTERRUPT/INTSET share SR226 but do not support exchange, even though
    // separate reads and writes are legal. INTCLEAR also has no exchange form.
    readable_sr(n) && writable_sr(n) && n != sr::INTSET
}

/// Interpreter-only core, selectable as a distinct `emu_core::Core` type.
/// The backing state is private: callers cannot enable windows, extra timers or JIT.
#[derive(Clone)]
pub struct Lx106 {
    cpu: Cpu,
    litbase: u32,
}

impl Default for Lx106 {
    fn default() -> Self { Self::new() }
}

impl Lx106 {
    pub fn new() -> Self {
        let mut c = Self { cpu: Cpu::new(0), litbase: 0 };
        c.reset();
        c
    }

    /// Read-only diagnostic state. Register writes go through the profile's accessors.
    pub fn state(&self) -> &Cpu { &self.cpu }
    pub fn get_ar(&self, n: u8) -> u32 { assert!(n < 16); self.cpu.get_ar(n) }
    pub fn set_ar(&mut self, n: u8, v: u32) { assert!(n < 16); self.cpu.set_ar(n, v); }

    pub fn read_sr(&mut self, n: u32) -> Option<u32> {
        if !readable_sr(n) { return None; }
        if n == LITBASE { return Some(self.litbase); }
        self.cpu.read_sr(n)
    }

    pub fn write_sr(&mut self, n: u32, v: u32) -> Option<()> {
        if !writable_sr(n) { return None; }
        match n {
            LITBASE => self.litbase = v & 0xffff_f001,
            sr::PS => self.cpu.ps = v & 0x3f,
            194 | 195 => self.cpu.eps[(n - 192) as usize] = v & 0x3f,
            sr::INTSET => self.cpu.interrupt |= v & SOFTWARE,
            sr::INTCLEAR => self.cpu.interrupt &= !(v & (SOFTWARE | EDGE)),
            sr::INTENABLE => self.cpu.intenable = v & 0x7fff,
            sr::IBREAKENABLE => self.cpu.ibreakenable = v & 1,
            _ => { self.cpu.write_sr(n, v)?; }
        }
        Some(())
    }

    fn vector(&self, ps: u32) -> u32 {
        self.cpu.vecbase + if ps & ps::EXCM != 0 { 0x70 } else if ps & ps::UM != 0 { 0x50 } else { 0x30 }
    }

    fn raise(&mut self, cause: u32) -> Trap {
        let vector = self.vector(self.cpu.ps);
        let trap = self.cpu.raise(cause);
        self.cpu.pc = vector;
        trap
    }

    fn pending(&self) -> u32 {
        let c = &self.cpu;
        (c.interrupt & NMI) | if !c.excm() && c.intlevel() == 0 { c.interrupt & c.intenable & 0x3fff } else { 0 }
    }

    fn take_interrupt(&mut self) -> Option<Trap> {
        let p = self.pending();
        if p == 0 { return None; }
        if p & NMI != 0 {
            let c = &mut self.cpu;
            c.epc[3] = c.pc;
            c.eps[3] = c.ps;
            c.ps = (c.ps & !ps::INTLEVEL_MASK) | 3 | ps::EXCM;
            c.pc = c.vecbase + 0x20;
            c.interrupt &= !NMI;
            c.waiting = false;
            Some(Trap::Interrupt(14))
        } else {
            self.raise(exc::LEVEL1_INTERRUPT);
            Some(Trap::Interrupt(p.trailing_zeros()))
        }
    }

    fn execute<B: Bus>(&mut self, bus: &mut B, mut i: Insn) -> Result<(), Trap> {
        if !supports(&i) { return Err(self.raise(exc::ILLEGAL)); }
        // core-isa.h:110-113: no hardware unaligned loads/stores. Keep this
        // profile-specific rule out of existing ESP32 memory paths.
        let align = match i.op {
            Op::L16ui | Op::L16si | Op::S16i => 1,
            Op::L32i | Op::L32iN | Op::S32i | Op::S32iN => 3,
            _ => 0,
        };
        let addr = self.get_ar(i.s).wrapping_add(i.imm as u32);
        if addr & align != 0 {
            self.cpu.excvaddr = addr;
            return Err(self.raise(exc::LOAD_STORE_ALIGNMENT));
        }
        let next = self.cpu.pc.wrapping_add(i.len as u32);
        match i.op {
            // The SoC must eventually supply these external-register operations.
            // Do not inherit the generic executor's zero/no-op placeholders.
            Op::Rer | Op::Wer => return Err(Trap::Unimplemented(self.cpu.pc, i.raw)),
            Op::Rsr | Op::Wsr | Op::Xsr => {
                let n = i.imm as u32;
                let value = self.get_ar(i.t);
                // supports() validates both directions before any read or write.
                // Stage readback until the (possibly fallible) write succeeds.
                let old = if i.op != Op::Wsr {
                    let Some(old) = self.read_sr(n) else { return Err(self.raise(exc::ILLEGAL)); };
                    Some(old)
                } else { None };
                if i.op != Op::Rsr && self.write_sr(n, value).is_none() { return Err(self.raise(exc::ILLEGAL)); }
                if let Some(old) = old { self.set_ar(i.t, old); }
                self.cpu.pc = next;
                return Ok(());
            }
            Op::Break | Op::BreakN => {
                let c = &mut self.cpu;
                c.debugcause = 1 << 3;
                c.epc[2] = c.pc; c.eps[2] = c.ps;
                c.ps = (c.ps & !ps::INTLEVEL_MASK) | 2 | ps::EXCM;
                c.pc = c.vecbase + 0x10;
                c.waiting = false;
                return Err(Trap::Exception(0x108));
            }
            Op::L32r if self.litbase & 1 != 0 => {
                // Xtensa ISA L32R: signed, word-scaled 16-bit literal displacement;
                // extended mode uses LITBASE[31:12] instead of aligned PC+3.
                i.imm = (self.litbase & 0xffff_f000).wrapping_add(((i.raw >> 8) << 2) | 0xfffc_0000) as i32;
            }
            _ => {}
        }
        let vector = self.vector(self.cpu.ps);
        let result = exec::exec_insn(&mut self.cpu, bus, &i);
        if matches!(result, Err(Trap::Exception(_))) { self.cpu.pc = vector; }
        result
    }
}

impl Core for Lx106 {
    type Irq = u32;
    fn reset(&mut self) {
        self.cpu = Cpu::new(0);
        self.cpu.pc = 0x4000_0080;
        self.cpu.configid = [0xc28c_dafa, 0x1082_b6f6];
        self.cpu.memctl = 0;
        self.litbase = 0;
    }
    fn pc(&self) -> u32 { self.cpu.pc }
    fn set_pc(&mut self, pc: u32) { self.cpu.pc = pc; }
    fn waiting(&self) -> bool { self.cpu.waiting }
    fn insn_count(&self) -> u64 { self.cpu.insn_count }
    fn set_irq(&mut self, lines: u32) {
        let rising = lines & !self.cpu.ext_irq_lines & (EDGE | NMI);
        self.cpu.interrupt = (self.cpu.interrupt & !LEVEL) | (lines & LEVEL) | rising;
        self.cpu.ext_irq_lines = lines;
    }
    fn irq_pending(&self) -> bool { self.pending() != 0 }
    fn irq_bits(irq: &u32) -> u32 { *irq }
    fn advance_cycles(&mut self, cycles: u32) {
        let before = self.cpu.ccount;
        self.cpu.ccount = before.wrapping_add(cycles);
        if self.cpu.ccompare[0].wrapping_sub(before).wrapping_sub(1) < cycles { self.cpu.interrupt |= TIMER; }
    }
    fn set_approximate_cpi(&mut self, cycles: u32) { self.cpu.approximate_cpi = cycles.max(1); }
    fn step_charges_cpi(&self) -> bool { true }
    fn cycles_until_wake(&self) -> Option<u64> {
        if !self.waiting() || self.cpu.excm() || self.cpu.intlevel() != 0
            || self.cpu.intenable & TIMER == 0 || self.cpu.interrupt & TIMER != 0 { return None; }
        let d = self.cpu.ccompare[0].wrapping_sub(self.cpu.ccount);
        Some(if d == 0 { 1u64 << 32 } else { d as u64 })
    }
    fn step<B: Bus>(&mut self, bus: &mut B) -> StepOutcome {
        let pc = self.pc();
        let mut out = StepOutcome { pc, next_pc: pc, bytes: None, length: 0, kind: StepKind::Idle, control: None };
        if let Some(t) = self.take_interrupt() { out.kind = StepKind::TrapBefore(t); }
        else if self.waiting() { self.advance_cycles(self.cpu.approximate_cpi); }
        else {
            match bus.fetch(pc) {
                Err(_) => { self.cpu.excvaddr = pc; out.kind = StepKind::TrapBefore(self.raise(exc::IFETCH_ERROR)); }
                Ok(bytes) => {
                    let i = decode(pc, bytes);
                    out.bytes = Some(bytes); out.length = i.len;
                    if supports(&i) { out.control = exec::control_event(&self.cpu, &i); }
                    out.kind = match self.execute(bus, i) { Ok(()) => StepKind::Retired, Err(t) => StepKind::TrapDuring(t) };
                    self.cpu.insn_count += 1;
                    self.advance_cycles(self.cpu.approximate_cpi);
                }
            }
        }
        out.next_pc = self.pc();
        out
    }
    fn regs(&self, out: &mut Vec<(&'static str, u32)>) { self.cpu.regs(out); }
    fn arg<B: Bus>(&self, bus: &mut B, n: usize) -> u32 {
        if n < 6 { self.get_ar(2 + n as u8) } else { bus.read32(self.get_ar(1).wrapping_add(((n - 6) * 4) as u32)).unwrap_or(0) }
    }
    fn return_from_stub<B: Bus>(&mut self, _bus: &mut B, v: u32) {
        self.cpu.pc = self.get_ar(0); self.set_ar(2, v);
        self.cpu.insn_count += 1; self.advance_cycles(self.cpu.approximate_cpi);
    }
    fn disasm(&self, pc: u32, bytes: [u8; 4]) -> String {
        let i = decode(pc, bytes);
        if supports(&i) { disasm::format(&i) } else { "ill".into() }
    }
    fn insn_len(bytes: [u8; 4]) -> u32 { decode(0, bytes).len as u32 }
    const TRACE_WIDTH: usize = 32;
    fn trace_regs(&self) -> String { self.cpu.trace_regs() }
    fn trace_trap(&self, core: usize, pc: u32, trap: &Trap) -> Option<String> { self.cpu.trace_trap(core, pc, trap) }
    fn regtrace_line(&self, pc: u32) -> String { self.cpu.regtrace_line(pc) }
    fn dump(&self, core: usize, sym: &dyn Fn(u32) -> String) -> String { self.cpu.dump(core, sym) }
    fn probe_args<B: Bus>(&self, bus: &mut B) -> String { format!("a2={:#x} a3={:#x} a4={:#x}", self.arg(bus, 0), self.arg(bus, 1), self.arg(bus, 2)) }
    fn return_address<B: Bus>(&self, _bus: &mut B) -> u32 { self.get_ar(0) }
}
