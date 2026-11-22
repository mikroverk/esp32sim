#!/usr/bin/env python3
"""The differential of riscv-rv32/tests/blocks_differential.rs on the JIT's WebAssembly backend,
under Node with web/wasm/jit.mjs as the host (EX269).

    wasm_fuzz.py TREE OUT_DIR [FIRST SEEDS]      (seeds 0..300: 2,400 programs)

Writes a cdylib crate around the test's `one` and its three region tests to OUT_DIR (a new directory)
and runs it. Needs the wasm32-unknown-unknown target; on macOS DYLD_FALLBACK_LIBRARY_PATH at the
toolchain's lib. REFUSE=N makes the host refuse every Nth module.
"""
import os, subprocess, sys
tree, out = os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2])
first, seeds = (sys.argv[3:5] + ['0', '300'])[:2] if len(sys.argv) > 4 else ('0', '300')
os.makedirs(f'{out}/src')
s = open(f'{tree}/riscv-rv32/tests/blocks_differential.rs').read()
cut = lambda a, b: s[s.index(a):s.index(b)]
s = s[:s.index('/// Compiled code that overruns its budget may never return')] + cut('/// A store in one block rewrites the next block', '/// A store into the next instruction of its own block').replace('#[test]\n', 'pub ')
s = '\n'.join(l for l in s.split('\n') if not l.startswith('//!')) + '''
#[link(wasm_import_module = "env")] extern "C" { fn say(p: *const u8, n: usize); }
#[no_mangle] pub extern "C" fn setup() { std::panic::set_hook(Box::new(|i| { let t = i.to_string(); unsafe { say(t.as_ptr(), t.len()) } })); }
#[no_mangle] pub extern "C" fn directed() { a_store_into_the_next_block_of_a_region_takes_effect(); a_store_into_a_blocks_second_page_takes_effect(); a_region_never_runs_past_its_budget(); }
#[no_mangle] pub extern "C" fn fuzz(seed: u32, kind: u32, loops: u32) -> f64 { one(seed as u64 * 4 + kind as u64, kind, loops != 0).2 as f64 }
#[no_mangle] pub extern "C" fn compiled() -> f64 { COMPILED.load(Relaxed) as f64 }
#[no_mangle] pub extern "C" fn runs() -> f64 { RUNS.load(Relaxed) as f64 }
'''
open(f'{out}/src/lib.rs', 'w').write(s)
open(f'{out}/Cargo.toml', 'w').write(f'''[package]
name = "wasmfuzz"
version = "0.0.0"
edition = "2021"
[lib]
crate-type = ["cdylib"]
[dependencies]
riscv-rv32 = {{ path = "{tree}/riscv-rv32", features = ["wasm-jit-host"] }}
emu-core = {{ path = "{tree}/emu-core" }}
[workspace]
''')
open(f'{out}/run.mjs', 'w').write(f'''import {{ readFileSync }} from 'node:fs';
import {{ createJitHost }} from '{tree}/web/wasm/jit.mjs';
let w; const jit = createJitHost(() => w);
const say = (p, n) => console.error(new TextDecoder().decode(new Uint8Array(w.memory.buffer, p, n)));
const every = Number(process.env.REFUSE || 0); let asked = 0;
const host_jit_compile = (p, n) => ++asked % every === 0 ? 0 : jit.imports.host_jit_compile(p, n);
const {{ instance }} = await WebAssembly.instantiate(readFileSync(process.argv[2]), {{ env: {{ ...jit.imports, host_jit_compile, say }} }});
w = instance.exports; w.setup(); w.directed();
const first = Number(process.argv[3]), seeds = Number(process.argv[4]), insns = [0, 0]; let programs = 0;
for (let s = first; s < first + seeds; s++) for (let k = 0; k < 4; k++) for (const l of [0, 1]) {{ insns[l] += w.fuzz(s, k, l); programs++; }}
console.log(JSON.stringify({{ programs, runs: w.runs(), compiled: w.compiled(), asked, insns, stats: jit.stats }}));
''')
env = dict(os.environ, RUSTFLAGS='-Clink-arg=--export-table -Clink-arg=--growable-table')
subprocess.run(['cargo', 'build', '--release', '--target', 'wasm32-unknown-unknown'], cwd=out, env=env, check=True)
subprocess.run(['node', 'run.mjs', 'target/wasm32-unknown-unknown/release/wasmfuzz.wasm', first, seeds], cwd=out, check=True)
