#!/usr/bin/env python3
"""Instrument Ledc in a separate source checkout; pass its ledc.rs path."""
from pathlib import Path
import sys

path = Path(sys.argv[1])
source = path.read_text()
assert 'static CLOCK_CALLS' not in source
counters = ['CLOCK', 'IRQ', 'TICK', 'WRITE']
statics = ''.join(f'static {c}_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);\n' for c in counters)
statics += '''impl Drop for Ledc {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        eprintln!("PWM_COUNTS clock={} irq={} tick={} write={}", CLOCK_CALLS.load(Relaxed), IRQ_CALLS.load(Relaxed), TICK_CALLS.load(Relaxed), WRITE_CALLS.load(Relaxed));
    }
}
'''
source = source.replace('impl Ledc {', statics + 'impl Ledc {', 1)
for signature, counter in [
    ('fn clock(&self) -> Option<ClockDomain> {', 'CLOCK'),
    ('fn irq_sources(&self) -> u64 {', 'IRQ'),
    ('fn tick(&mut self, ticks: u64) {', 'TICK'),
    ('fn write(&mut self, off: u32, value: u32) -> WriteEffect {', 'WRITE'),
]:
    assert source.count(signature) == 1
    source = source.replace(signature, signature + f' {counter}_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);', 1)
path.write_text(source)
