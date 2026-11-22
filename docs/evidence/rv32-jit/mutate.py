#!/usr/bin/env python3
"""Mutants of the RV32 JIT (AArch64 backend and the dispatch) against `blocks_differential`, EX269.

    mutate.py TREE [NAME ...]

TREE is a clean checkout it may edit (each mutant is undone); prints what killed each one.
"""
import os, re, subprocess, sys
ROOT = sys.argv[1]
J = "riscv-rv32/src/jit.rs"; B = "riscv-rv32/src/block.rs"
ENTRY = "cpu.blocks.jit.is_some() && n == e.n as u32 && cpu.hwlp_count[0] | cpu.hwlp_count[1] == 0 && !bus.block_break()"
REUSE = "if n == e.n as usize && vidx == e.vidx && ver == e.ver => s.code = code"
STALE = "                        check_versions(a, b, stale);\n                        exits.push(Exit { at: stale, k: k32, pc, pend: p, set_pc: %s, status: Some(LEAVE) });"
TR = "a.cmp_imm(REM, b.insns.len() as u32); a.b_cond(Cond::Lo, out);"
def one(name, f, old, new): return (name, f, old, new)
M = [
 one("exit-done-k", J, "a.add_imm(10, DONE, e.k + 1);", "a.add_imm(10, DONE, e.k);"),
 one("transition-no-budget", J, TR, ""),
 one("transition-budget-off-by-one", J, TR, TR.replace("len() as u32)", "len() as u32 - 1)")),
 one("transition-no-versions", J, "            check_versions(a, b, out);\n", ""),
 one("store-no-stale-check", J, STALE % "Some(next)", (STALE % "Some(next)").split("\n")[1]),
 one("exec-no-stale-check", J, STALE % "None", (STALE % "None").split("\n")[1]),
 one("exit-last-pc", J, "set_last(&mut a, e.pc);", "set_last(&mut a, e.pc.wrapping_add(4));"),
 one("load-ignore-break", J, "v as u64 | if bus.block_break() { (STOP as u64) << 32 } else { 0 }", "v as u64"),
 one("store-ignore-break", J, "if bus.block_break() { STOP } else { END }\n}", "END\n}"),
 one("target-ignore-bloom", J, "if self.bloom & pc_bit(pc) != 0 { None } else { self.index(pc) }", "self.index(pc)"),
 one("no-retired", J, "add_counter(a, offset_of!(Cpu, retired_count) as u32, p.retired);", ""),
 one("cycle-off", J, "add_counter(a, offset_of!(Cpu, cycle_count) as u32, p.insns);", "add_counter(a, offset_of!(Cpu, cycle_count) as u32, p.insns.saturating_sub(1));"),
 one("div-no-zero", J, "a.movn(3, 0, 0); a.cmp_imm(1, 0); a.csel(2, 3, 2, Cond::Eq);", ""),
 one("jalr-no-mask", J, "a.mov32(4, !1); a.and(3, 3, 4);", ""),
 one("sltiu-signed", J, "Sltiu => { ri(a); a.cmp(0, 1); a.cset(2, Cond::Lo); }", "Sltiu => { ri(a); a.cmp(0, 1); a.cset(2, Cond::Lt); }"),
 one("hexec-no-leave", J, "if cpu.pc != next || i.op.is_hwlp() || cpu.hwlp_count[0] | cpu.hwlp_count[1] != 0 { return LEAVE; }", ""),
 one("entry-no-hwlp", B, ENTRY, ENTRY.replace(" && cpu.hwlp_count[0] | cpu.hwlp_count[1] == 0", "")),
 one("entry-no-break", B, ENTRY, ENTRY.replace(" && !bus.block_break()", "")),
 one("entry-no-whole-block", B, ENTRY, ENTRY.replace(" && n == e.n as u32", "")),
 one("reuse-no-versions", B, REUSE, REUSE.replace(" && vidx == e.vidx && ver == e.ver", "")),
 one("reuse-no-length", B, REUSE, REUSE.replace("n == e.n as usize && ", "")),
 one("load-trap-pc", J, "cpu.trap(c, va, pc); ctx.trap = Some(Trap::Exception(c)); (TRAP as u64) << 32", "cpu.trap(c, va, pc.wrapping_add(2)); ctx.trap = Some(Trap::Exception(c)); (TRAP as u64) << 32"),
 one("exec-pending-no-flush", J, "                    p.insns += 1;\n                    flush(a, &mut p);\n                    set_pc(a, pc);", "                    p.insns += 1;\n                    set_pc(a, pc);"),
 one("bext-width", J, "a.ubfx(2, 2, 0, 1); }", "a.ubfx(2, 2, 0, 2); }"),
 one("rem-quotient", J, "a.msub(2, 2, 1, 0);", ""),
 one("rol-no-neg", J, "Rol => { rr(a); a.neg(1, 1); rorv(a, 2, 0, 1); }", "Rol => { rr(a); rorv(a, 2, 0, 1); }"),
 one("ctz-as-clz", J, "rbit(a, 2, 0); a.clz(2, 2);", "a.clz(2, 0);"),
 one("minu-signed", J, "Min => Cond::Lt, _ => Cond::Lo };", "Min => Cond::Lt, _ => Cond::Lt };"),
 one("sh3add-as-sh2", J, "let sh = match i.op { Sh1add => 1, Sh2add => 2, _ => 3 };\n                a.add_lsl", "let sh = match i.op { Sh1add => 1, Sh2add => 2, _ => 2 };\n                a.add_lsl"),
 one("rev8-off", J, "rev(a, 2, 0); }", "a.mov(2, 0); }"),
 one("bseti-as-binvi", J, "ld(a, 0, i.rs1); a.mov32(1, 1 << (imm & 31));\n                match i.op { Bclri => a.bic(2, 0, 1), Binvi => a.eor(2, 0, 1), _ => a.orr(2, 0, 1) }", "ld(a, 0, i.rs1); a.mov32(1, 1 << (imm & 31));\n                match i.op { Bclri => a.bic(2, 0, 1), _ => a.eor(2, 0, 1) }"),
 one("mulhu-signed", J, "a.umull(2, 0, 1);", "a.smull(2, 0, 1);"),
 one("leave-last-pc", J, "set_pc(a, pc); set_last(a, last); a.mov(0, DONE); a.b(epilogue);", "set_pc(a, pc); set_last(a, last.wrapping_add(4)); a.mov(0, DONE); a.b(epilogue);"),
 one("gone-last-pc", J, "Succ::Gone => { set_last(a, last);", "Succ::Gone => { set_last(a, last.wrapping_add(4));"),
 one("hexec-ignore-break", J, "    if bus.block_break() { return STOP; }\n", ""),
 one("andn-as-and", J, "Andn => { rr(a); a.bic(2, 0, 1); }", "Andn => { rr(a); a.and(2, 0, 1); }"),
 one("rebuild-keeps-slot", B, "        j.slots[ei] = NO_SLOT;\n", ""),
 one("flush-keeps-code-refs", B, "        self.regions.clear();\n        self.slots.fill(NO_SLOT);\n", ""),
 one("versions-first-page-only", J, "let n = if b.vidx[0] == b.vidx[1] { 1 } else { 2 };\n        for v in 0..n {\n            a.mov32", "let n = 1;\n        for v in 0..n {\n            a.mov32"),
 one("exec-no-note-pc", J, "    bus.note_pc(pc);\n    if let Err(t)", "    if let Err(t)"),
 one("load-no-note-pc", J, "    bus.note_pc(pc);\n    let r = match kind {\n        0 => bus.read8(va).map(|v| v as i8", "    let r = match kind {\n        0 => bus.read8(va).map(|v| v as i8"),
 one("store-no-note-pc", J, "    bus.note_pc(pc);\n    let r = match kind { 0 => bus.write8", "    let r = match kind { 0 => bus.write8"),
 one("run-no-bloom-after-region", B, "                used += done;\n                if cpu.boundary_bloom != 0 && cpu.boundary_bloom & pc_bit(cpu.pc) != 0 { return (used, None); }\n                continue;", "                used += done;\n                continue;"),
]
only = sys.argv[2:]
env = dict(os.environ)
cmd = ["cargo", "test", "--release", "-p", "riscv-rv32", "--test", "blocks_differential"]
def test(extra=[]):
    p = subprocess.Popen(cmd + extra, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, start_new_session=True)
    try: out, _ = p.communicate(timeout=240); return p.returncode, out
    except subprocess.TimeoutExpired: os.killpg(p.pid, 9); p.communicate(); return None, ""
assert test()[0] == 0, "baseline fails"
count = {}
for name, f, old, new in M:
    if only and name not in only: continue
    p = os.path.join(ROOT, f); src = open(p).read()
    if src.count(old) != 1: print(f"{name}: pattern count {src.count(old)}, SKIPPED", flush=True); continue
    open(p, "w").write(src.replace(old, new))
    try:
        rc, out = test(["--no-run"])
        if rc != 0: v = "BUILD ERROR " + out[-200:]
        else:
            rc, out = test()
            tests = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", out, re.M)))
            v = "SURVIVED" if rc == 0 else "HANG" if rc is None else "killed by " + (", ".join(tests) or "") + (" [60 s watchdog]" if "did not return within 60 s" in out else "") + (f" [{out.strip().splitlines()[-1][:80]}]" if not tests and "did not return" not in out else "")
    finally: open(p, "w").write(src)
    count[v.split()[0]] = count.get(v.split()[0], 0) + 1
    print(f"{name}: {v}", flush=True)
print(count)
