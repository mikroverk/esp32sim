use crate::{Device, RegRam, Rmt, WriteEffect};
use emu_core::ClockDomain;

/// The C3/C6 RMT: the S3's transmitter IP with two TX and two RX channels on a compacted register
/// map. The TX channels' CONF0 bits and the interrupt bits the model raises (TX_END n, TX_THR
/// 8+n) are the same, so this only maps offsets onto the shared model; the RX channels read back
/// what was written.
pub struct RmtCompact {
    pub rmt: Rmt,
    ram: RegRam,
}
impl RmtCompact {
    pub fn new(cpu_hz: u64) -> Self {
        RmtCompact {
            rmt: Rmt::new(cpu_hz),
            ram: RegRam::new(),
        }
    }
    fn map(off: u32) -> Option<u32> {
        Some(match off {
            0x00..=0x0c => off,                 // CHnDATA
            0x10 | 0x14 => 0x20 + (off - 0x10), // CH0/1 CONF0 (TX)
            0x28 | 0x2c => 0x50 + (off - 0x28), // CH0/1 STATUS
            0x38 => 0x70,
            0x3c => 0x74,
            0x40 => 0x78,
            0x44 => 0x7c,                       // INT_RAW / ST / ENA / CLR
            0x48 | 0x4c => 0x80 + (off - 0x48), // CH0/1 CARRIER_DUTY
            0x58 | 0x5c => 0xa0 + (off - 0x58), // CH0/1 TX_LIM
            0x68 => 0xc0,
            0x6c => 0xc4,
            0x70 => 0xc8,                           // SYS_CONF, TX_SIM, REF_CNT_RST
            0x400..=0x6fc => 0x800 + (off - 0x400), // symbol memory, 48 words per channel
            _ => return None,
        })
    }
}
impl Device for RmtCompact {
    fn read(&mut self, off: u32) -> u32 {
        match Self::map(off) {
            Some(o) => self.rmt.read(o),
            None => self.ram.read(off),
        }
    }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        match Self::map(off) {
            Some(o) => self.rmt.write(o, v),
            None => self.ram.write(off, v),
        }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 {
        self.rmt.irq() as u64
    }
    fn clock(&self) -> Option<ClockDomain> {
        Some(ClockDomain::Cpu)
    }
    fn tick(&mut self, cycles: u64) {
        self.rmt.tick(cycles)
    }
    /// A channel mid-transmission raises TX_END/TX_THR from its own symbol clock: while one
    /// runs, time may only be skipped one symbol at a time (the shortest WS2812 symbol is
    /// 0.4 µs; 32 cycles is under that at any RMT divider the led_strip driver uses).
    fn has_deadline(&self) -> bool {
        true
    }
    fn next_deadline(&self) -> Option<u64> {
        Some(if self.rmt.ch.iter().any(|c| c.running) {
            32
        } else {
            u64::MAX
        })
    }
}
