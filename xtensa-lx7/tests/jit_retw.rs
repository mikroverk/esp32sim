//! Native guarded windowed returns match interpreter state and exception paths.
#![cfg(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux")))]
use xtensa_lx7::block::run_block;
use xtensa_lx7::state::ps;
use xtensa_lx7::{decode, Bus, Cpu, Fault, FlatRam, Op, Trap};
const CODE: u32 = 0x4037_0000;
struct CodeBus { code: FlatRam, versions: [u32; 2], interpreted: u32 }
impl Bus for CodeBus {
    fn note_code_page(&mut self, _: u32) {}
    fn read8(&mut self, _: u32) -> Result<u8, Fault> { Err(Fault::Unmapped) }
    fn read16(&mut self, _: u32) -> Result<u16, Fault> { Err(Fault::Unmapped) }
    fn read32(&mut self, _: u32) -> Result<u32, Fault> { Err(Fault::Unmapped) }
    fn write8(&mut self, _: u32, _: u8) -> Result<(), Fault> { Err(Fault::Unmapped) }
    fn write16(&mut self, _: u32, _: u16) -> Result<(), Fault> { Err(Fault::Unmapped) }
    fn write32(&mut self, _: u32, _: u32) -> Result<(), Fault> { Err(Fault::Unmapped) }
    fn fetch(&mut self, pc: u32) -> Result<[u8; 4], Fault> { self.code.fetch(pc) }
    fn page_versions(&self) -> &[u32] { &self.versions }
    fn note_pc(&mut self, _: u32) { self.interpreted += 1; }
}
struct Run { result: (u32, Option<Trap>), cpu: Cpu, bus: CodeBus }
fn run(op: Op, bytes: [u8; 3], jit: bool, woe: bool, inc: u32, live: bool, wb: u32) -> Run {
    let mut bus = CodeBus { code: FlatRam::new(CODE, 16), versions: [0; 2], interpreted: 0 };
    bus.code.mem[..3].copy_from_slice(&bytes);
    assert_eq!(decode(CODE, bus.code.fetch(CODE).unwrap()).op, op);
    let mut cpu = Cpu::new(0);
    cpu.pc = CODE; cpu.ps = if woe { ps::WOE } else { 0 }; cpu.windowbase = wb;
    cpu.windowstart = (1 << wb) | if live { 1 << (wb.wrapping_sub(inc) & 15) } else { 0 };
    cpu.blocks.jit_enabled = jit;
    cpu.set_ar(0, (inc << 30) | 0x0037_0040);
    let result = run_block(&mut cpu, &mut bus, 1);
    if jit { assert!(cpu.blocks.jit_instructions > 0); }
    Run { result, cpu, bus }
}
type State = (u32, Option<Trap>, u32, [u32; 64], u32, u32, u32, u32, u32, u64, u32);
fn state(r: &Run) -> State {
    (r.result.0, r.result.1, r.cpu.pc, r.cpu.ar, r.cpu.windowbase, r.cpu.windowstart, r.cpu.ps, r.cpu.epc[1], r.cpu.exccause, r.cpu.insn_count, r.cpu.ccount)
}
#[test]
fn retw_guarded_path_matches_interpreter() {
    for (op, bytes) in [(Op::Retw, [0x90, 0, 0]), (Op::RetwN, [0x1d, 0xf0, 0])] {
        for wb in [0, 3, 15] { for inc in 1..=3 {
            let (n, i) = (run(op, bytes, true, true, inc, true, wb), run(op, bytes, false, true, inc, true, wb));
            assert_eq!(state(&n), state(&i), "{op:?} wb={wb} inc={inc}");
            assert_eq!(n.bus.interpreted, 0, "{op:?}: guarded return must not call helper");
        }}
    }
}
#[test]
fn retw_timer_interrupt_is_delivered_at_the_next_boundary() {
    for (op, bytes) in [(Op::Retw, [0x90, 0, 0]), (Op::RetwN, [0x1d, 0xf0, 0])] {
        for return_to_next in [false, true] { for prid in 0..=1 {
            let mut results = Vec::new();
            for jit in [false, true] {
                let mut bus = CodeBus { code: FlatRam::new(CODE, 16), versions: [0; 2], interpreted: 0 };
                bus.code.mem[..3].copy_from_slice(&bytes);
                let len = decode(CODE, bus.code.fetch(CODE).unwrap()).len as u32;
                let target = CODE + if return_to_next { len } else { 8 };
                let mut cpu = Cpu::new(prid);
                cpu.pc = CODE; cpu.ps = ps::WOE; cpu.windowbase = 3; cpu.windowstart = (1 << 3) | (1 << 2);
                cpu.blocks.jit_enabled = jit; cpu.set_ar(0, (1 << 30) | (target & 0x3fff_ffff));
                cpu.ccompare[0] = 1; cpu.intenable = 1 << 6;
                let first = run_block(&mut cpu, &mut bus, 1);
                assert_eq!(first, (1, None), "{op:?}");
                assert_eq!(cpu.pc, target, "{op:?}");
                if jit { assert!(cpu.blocks.jit_instructions > 0); assert_eq!(bus.interpreted, 0); }
                let second = run_block(&mut cpu, &mut bus, 1);
                assert!(matches!(second.1, Some(Trap::Interrupt(_))), "{op:?}");
                results.push((first, second, cpu.pc, cpu.ps, cpu.windowbase, cpu.windowstart,
                    cpu.epc, cpu.insn_count, cpu.ccount, cpu.interrupt));
            }
            assert_eq!(results[0], results[1], "{op:?}, return_to_next={return_to_next}, prid={prid}");
        }}
    }
}
#[test]
fn retw_guard_misses_use_the_existing_helper() {
    for (op, bytes) in [(Op::Retw, [0x90, 0, 0]), (Op::RetwN, [0x1d, 0xf0, 0])] {
        for (woe, inc, live) in [(false, 1, true), (true, 0, true), (true, 1, false), (true, 2, false), (true, 3, false)] {
            let (n, i) = (run(op, bytes, true, woe, inc, live, 3), run(op, bytes, false, woe, inc, live, 3));
            assert_eq!(state(&n), state(&i), "{op:?} woe={woe} inc={inc} live={live}");
            assert!(n.result.1.is_some(), "{op:?} must trap");
            assert_eq!(n.bus.interpreted, 1, "{op:?} guard miss must call helper");
        }
    }
}
