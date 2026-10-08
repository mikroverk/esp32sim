#!/usr/bin/env python3
"""Run EX220 rule-removal checks from a clean working tree; restore each file afterwards."""
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
SHA = ["-p", "esp32c3", "-p", "esp32c6", "--test", "sha_dma"]
ECC = ["-p", "esp32c6", "--lib", "ecc::tests"]
WIFI = ["-p", "esp32c6", "--test", "wifi"]
CASES = [
    ("SHA start wiring C3", "esp32c3/src/bus.rs", "self.periph.sha.dma_pending &&", "false &&", SHA),
    ("SHA start wiring C6", "esp32c6/src/bus.rs", "self.periph.sha.dma_pending &&", "false &&", SHA),
    ("GDMA late start C3", "esp32c3/src/bus.rs", "0x6003_b000 | 0x6003_f000", "0x6003_b000", SHA),
    ("GDMA late start C6", "esp32c6/src/bus.rs", "0x6008_9000 | 0x6008_0000", "0x6008_9000", SHA),
    ("DMA first resets digest", "esp-periph/src/sha.rs", "let mut first = self.dma_first;", "let mut first = false;", SHA),
    ("DMA continue retains digest", "esp-periph/src/sha.rs", "let mut first = self.dma_first;", "let mut first = true;", SHA),
    ("DMA initializes only the first block", "esp-periph/src/sha.rs", "            first = false;", "            first = self.dma_first;", SHA),
    ("DMA consumes pending request", "esp-periph/src/sha.rs", "self.dma_pending = false;", "self.dma_pending = true;", SHA),
    ("DMA clears busy", "esp-periph/src/sha.rs", "self.busy = false;\n        let want", "self.busy = true;\n        let want", SHA),
    ("DMA rejects short input", "esp-periph/src/sha.rs", "input.filter(|bytes| bytes.len() == want)", "input", SHA),
    ("DMA error interrupt", "esp-periph/src/sha.rs", "memory.out_channel().int_raw |= 1 << 2;", "memory.out_channel().int_raw |= 0;", SHA),
    ("DMA error stops channel", "esp-periph/src/sha.rs", "memory.out_channel().running = false;", "memory.out_channel().running = true;", SHA),
    ("DMA owner check", "esp-periph/src/gdma.rs", "memory.out_channel().conf1 & (1 << 12) != 0 && !d.owner_dma", "false", SHA),
    ("DMA check-owner gate", "esp-periph/src/gdma.rs", "memory.out_channel().conf1 & (1 << 12) != 0 && !d.owner_dma", "!d.owner_dma", SHA),
    ("DMA ownership return", "esp-periph/src/gdma.rs", "memory.writeback(desc, control & !(1 << 31))", "memory.writeback(desc, control)", SHA),
    ("DMA EOF interrupt", "esp-periph/src/gdma.rs", "memory.out_channel().int_raw |= 1 << 1;", "memory.out_channel().int_raw |= 0;", SHA),
    ("DMA DONE interrupt", "esp-periph/src/gdma.rs", "memory.out_channel().int_raw |= 1;", "memory.out_channel().int_raw |= 0;", SHA),
    ("DMA EOF descriptor", "esp-periph/src/gdma.rs", "memory.out_channel().eof_desc = desc;", "memory.out_channel().eof_desc = 0;", SHA),
    ("DMA cycle detection", "esp-periph/src/gdma.rs", "!visited.insert(desc)", "!visited.insert(desc) && false", SHA),
    ("DMA descriptor budget", "esp-periph/src/gdma.rs", "visited.len() > GDMA_DESCRIPTOR_STEP_BUDGET", "false", ["-p", "esp-periph", "--lib", "zero_progress_chain"]),
    ("S3 writeback side effects", "esp32s3/src/bus/dma.rs", "self.bus.write32_unpriced(address, value)", "{ let saved = self.bus.periph.gdma.out[self.channel]; let result = self.bus.write32_unpriced(address, value); self.bus.periph.gdma.out[self.channel] = saved; result }", ["-p", "esp32s3", "--lib", "crypto_descriptor_writeback"]),
    ("TX DC completion", "esp32c6/src/wifi.rs", "u32::from(self.ram.read(off) & 1 != 0) << 22", "0", WIFI),
    ("Calibration completion", "esp32c6/src/wifi.rs", "{ 7 << 14 } else { 0 }", "{ 0 } else { 0 }", WIFI),
    ("TX DC comparator mask", "esp32c6/src/wifi.rs", "self.ram.read(off) & 0x003f_ffff", "self.ram.read(off)", WIFI),
    ("ECC on-curve equation", "esp32c6/src/ecc.rs", "zero(&sub(&mul(y, y, p), &rhs, p))", "true", ECC),
    ("ECC START self-clear", "esp32c6/src/ecc.rs", "self.conf & !(1 | 256)", "self.conf & !256", ECC),
    ("ECC completion interrupt", "esp32c6/src/ecc.rs", "self.raw = 1;", "self.raw = 0;", ECC),
    ("ECC masked interrupt", "esp32c6/src/ecc.rs", "(self.raw & self.ena) as u64", "self.raw as u64", ["-p", "esp32c6", "--test", "ecc"]),
    ("ECC canonical coordinates", "esp32c6/src/ecc.rs", "!(x.iter().rev().cmp(p.iter().rev()).is_lt() && y.iter().rev().cmp(p.iter().rev()).is_lt())", "false", ECC),
    ("ECC unsupported key length", "esp32c6/src/ecc.rs", "self.conf & 4 == 0", "false", ECC),
    ("ECC scalar multiplication", "esp32c6/src/ecc.rs", "self.mem[bit / 32] >> (bit % 32) & 1 != 0", "false", ECC),
    ("ECC verification gates multiplication", "esp32c6/src/ecc.rs", "(mode == 0 || valid)", "true", ECC),
    ("ECC verify-only mode", "esp32c6/src/ecc.rs", "mode != 2 &&", "true &&", ECC),
    ("ECC result read-only", "esp32c6/src/ecc.rs", "(value & !256) | (self.conf & 256)", "value", ECC),
    ("ECC unsupported mode", "esp32c6/src/ecc.rs", "!matches!(mode, 0 | 2 | 3)", "false", ECC),
    ("ECC reset", "esp32c6/src/ecc.rs", "*self = Self::default();", "self.conf = 0;", ECC),
    ("ECC W1C", "esp32c6/src/ecc.rs", "self.raw &= !value", "self.raw |= value", ECC),
    ("ECC MMIO mapping", "esp32c6/src/periph.rs", '0x8b "ECC_MULT"', '0x8e "ECC_MULT"', ["-p", "esp32c6", "--test", "ecc"]),
    ("ECC interrupt routing", "esp32c6/src/periph.rs", '(ecc) => [src::ECC]', '(ecc) => []', ["-p", "esp32c6", "--test", "ecc"]),
]

firmware = sys.argv[1:] == ["--firmware"]
if firmware:
    tests = lambda chip: ["--release", "-p", "esp32sim", "--test", "goldens", "crypto_tls_" + chip, "--", "--ignored"]
    CASES = [
        ("TLS needs C3 SHA DMA", "esp32c3/src/bus.rs", "self.periph.sha.dma_pending &&", "false &&", tests("c3")),
        ("TLS needs C6 SHA DMA", "esp32c6/src/bus.rs", "self.periph.sha.dma_pending &&", "false &&", tests("c6")),
        ("TLS needs C6 ECC", "esp32c6/src/ecc.rs", "if value & 1 != 0 {", "if false {", tests("c6")),
        ("Unstubbed C6 PHY needs TX DC completion", "esp32c6/src/wifi.rs", "u32::from(self.ram.read(off) & 1 != 0) << 22", "0", tests("c6")),
        ("Unstubbed C6 PHY needs calibration completion", "esp32c6/src/wifi.rs", "{ 7 << 14 } else { 0 }", "{ 0 } else { 0 }", tests("c6")),
    ]

results = []
for name, file, old, new, tests in CASES:
    path = ROOT / file
    original = path.read_text()
    assert original.count(old) == 1, (name, original.count(old))
    try:
        path.write_text(original.replace(old, new))
        command = ["cargo", "+1.99.0", "test", *tests]
        run = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        failed = re.findall(r"^test (.+) \.\.\. FAILED$", run.stdout, re.M)
        results.append({"mutation": name, "file": file, "old": old, "new": new,
                        "command": " ".join(command), "killed_by": failed, "exit": run.returncode})
        print(f"{name}: {'KILLED' if failed else 'SURVIVED/BUILD ERROR'}", flush=True)
        if not failed:
            print(run.stdout[-2500:], flush=True)
    finally:
        path.write_text(original)
output = ROOT / "docs/evidence/c3-c6-sha-dma-wifi-cal" / ("firmware-mutations.json" if firmware else "mutations.json")
output.write_text(json.dumps(results, indent=2) + "\n")
assert all(r["killed_by"] and r["exit"] != 0 for r in results)
