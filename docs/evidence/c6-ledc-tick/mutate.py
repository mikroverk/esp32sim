"""LEDC timer-tick mutation campaign: each mutant edits one changed (or now load-bearing) line of
esp-periph/src/ledc.rs in a scratch worktree, runs the esp-periph library tests, the classic
ESP32 LEDC library tests and the S3/C3/C6 PWM integration tests, and counts as killed only when a
test FAILS at run time (crates run in order and stop at the first failing one; a compile error is reported as such, never as a kill). Sources are
restored after each case.
usage: mutate.py <mutation-worktree> <out-dir>   (DRY=1: print anchor counts only)"""
import subprocess, sys, json, os
from pathlib import Path
tree, out = Path(sys.argv[1]), Path(sys.argv[2]); out.mkdir(parents=True, exist_ok=True)
T = [(['-p', 'esp-periph', '--lib'], 'esp-periph'), (['-p', 'esp32', '--lib', 'ledc'], 'esp32'),
     (['-p', 'esp32c6', '--test', 'pwm'], 'esp32c6'), (['-p', 'esp32c3', '--test', 'pwm'], 'esp32c3'),
     (['-p', 'esp32s3', '--test', 'pwm'], 'esp32s3')]
L = 'esp-periph/src/ledc.rs'
ACT = '            if self.active & (1 << n) == 0 { continue; }\n'
FAST = '                Some(total) if total < modulus => self.phase[n] = total,\n'
MOD = '            Some(total) => total % modulus,\n'
OVF = '            None => ((self.phase[n] as u128 + ticks as u128 * hz as u128) % modulus as u128) as u64,\n'
RAW = '        self.raw |= 1 << n;\n'
CHN = '            if self.pending[ch] && self.regs.read(off) & 3 == n as u32 {\n'
UPD = '                self.active = (self.active & !(1 << n)) | (u8::from(self.timer(n).is_some()) << n);\n'
cases = [
 ('control-unmutated', ACT, ACT),
 ('active-inverted', ACT, ACT.replace('== 0', '!= 0')),
 ('active-timer0-bit', ACT, ACT.replace('(1 << n)', '1')),
 ('active-next-bit', ACT, ACT.replace('(1 << n)', '(1 << (n + 1))')),
 ('active-break', ACT, ACT.replace('continue', 'break')),
 ('active-check-removed', ACT, ''),
 ('fast-le', FAST, FAST.replace('total < modulus', 'total <= modulus')),
 ('fast-no-store', FAST, FAST.replace('self.phase[n] = total', '{}')),
 ('fast-never', FAST, FAST.replace('total < modulus', 'false')),
 ('wrap-subtract', MOD, MOD.replace('total % modulus', 'total - modulus')),
 ('wrap-overflow-zero', OVF, '            None => 0,\n'),
 ('wrap-no-raw', RAW, ''),
 ('wrap-channel-timer-ne', CHN, CHN.replace('== n as u32', '!= n as u32')),
 ('active-never-cleared', UPD, '                self.active |= u8::from(self.timer(n).is_some()) << n;\n'),
 ('active-never-set', UPD, '                self.active &= !(1 << n);\n'),
]
if os.environ.get('DRY'):
    for name, old, new in cases: print(name, (tree / L).read_text().count(old), old != new)
    sys.exit(0)
env = dict(os.environ, CARGO_TARGET_DIR=str(out / 'target'))
rows = []
for name, old, new in cases:
    p = tree / L; src = p.read_text()
    assert src.count(old) == 1, (name, old)
    p.write_text(src.replace(old, new))
    log, failed, compiled, rc = '', [], True, 0
    try:
        for args, crate in T:
            r = subprocess.run(['nice', '-n', '10', 'cargo', '+1.99.0', 'test', '--release', '-j2', '--no-fail-fast'] + args, cwd=tree, env=env, capture_output=True, text=True, timeout=1800)
            log += f'== {crate}\n' + r.stdout + r.stderr
            rc |= r.returncode
            compiled &= 'error[E' not in r.stderr and 'could not compile' not in r.stderr
            failed += [f'{crate}::{l.split()[1]}' for l in r.stdout.splitlines() if l.startswith('test ') and l.rstrip().endswith('FAILED')]
            if failed and compiled: break   # killed: the remaining crates are not run
    finally:
        p.write_text(src)
    (out / f'{name}.log').write_text(log)
    row = {'mutant': name, 'file': L, 'compiled': compiled, 'killed': compiled and rc != 0 and bool(failed), 'failing_tests': failed}
    rows.append(row); print(json.dumps(row), flush=True)
(out / 'mutations.json').write_text(json.dumps(rows, indent=1) + '\n')
print(sum(r['killed'] for r in rows), '/', len(rows), 'killed')
