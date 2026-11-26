#!/usr/bin/env python3
"""Build post-run-only retirement reporters in a disposable checkout.

Usage: build_probe.py TREE OUTPUT_PREFIX; CARGO_TARGET_DIR must be set.
Build unprobed --bins with the same target first and preserve them separately.
"""
import os
from pathlib import Path
import shutil
import subprocess
import sys

tree=Path(sys.argv[1]).resolve()
prefix=Path(sys.argv[2]).resolve()
target=Path(os.environ['CARGO_TARGET_DIR']).resolve()
cli=tree/'cli/src/lib.rs'
original=cli.read_text()
old='fn run<S: Soc>(mut m: Machine<S>, o: &Opts) {'
assert original.count(old)==1
probe=original.replace(old,"fn run<S: Soc>(mut m: Machine<S>, o: &Opts) where S::Core: 'static {")
old='    let dt = t0.elapsed().as_secs_f64();\n    report(&mut m, o, stop, dt);'
new='''    let dt = t0.elapsed().as_secs_f64();
    if let Some(cpu) = (&m.cores[0] as &dyn std::any::Any).downcast_ref::<<esp32c6::soc::C6 as Soc>::Core>() {
        eprintln!("[probe] core0 retired={}", cpu.retired_count);
    }
    report(&mut m, o, stop, dt);'''
assert probe.count(old)==1
try:
    cli.write_text(probe.replace(old,new))
    subprocess.run(['nice','-n','10','cargo','+1.99.0','build','--release','--bins'],cwd=tree,check=True)
    for chip in ['c3','c6']:
        shutil.copy2(target/'release'/('esp32sim-'+chip),str(prefix)+'-'+chip)
finally:
    cli.write_text(original)
