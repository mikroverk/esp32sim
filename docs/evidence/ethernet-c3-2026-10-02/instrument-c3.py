"""Instrument a disposable source export, then build with its own CARGO_TARGET_DIR.
Usage: python3 instrument-c3.py /path/to/export
Counters are diagnostic only; never time this executable.
"""
from pathlib import Path
import sys
r=Path(sys.argv[1])
p=r/'esp32c3/src/bus.rs';s=p.read_text();start=s.index('    fn devices(&mut self, cycles: u32)');pos=s.index('{',start)+1;s=s[:pos]+'\n        crate::PROFILE_TICKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);'+s[pos:]
s=s.replace('    fn pending_work(&mut self) {','    fn pending_work(&mut self) {\n        crate::PROFILE_WORK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);');p.write_text(s)
p=r/'esp32c3/src/periph.rs';s=p.read_text().replace('pub fn source_status(&self) -> [u32; 4] {','pub fn source_status(&self) -> [u32; 4] { crate::PROFILE_IRQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);');p.write_text(s)
p=r/'esp32c3/src/lib.rs';s=p.read_text();s+='\n'+''.join(f'pub static PROFILE_{n}: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);\n' for n in ['TICKS','IRQ','WORK']);p.write_text(s)
p=r/'cli/src/lib.rs';s=p.read_text().replace('    eprintln!("\\n[emu] stop:', '    eprintln!("[profile-c3] ticks={} irq_refresh={} pending_work={}", esp32c3::PROFILE_TICKS.load(std::sync::atomic::Ordering::Relaxed), esp32c3::PROFILE_IRQ.load(std::sync::atomic::Ordering::Relaxed), esp32c3::PROFILE_WORK.load(std::sync::atomic::Ordering::Relaxed));\n    eprintln!("\\n[emu] stop:');p.write_text(s)
