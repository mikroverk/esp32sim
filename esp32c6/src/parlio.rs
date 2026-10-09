//! Functional C6 PARLIO TX. Register layout: ESP-IDF v5.5.4
//! components/soc/esp32c6/register/soc/parl_io_reg.h:161-355;
//! pcr_reg.h:1061-1098. Clock divisor/reset follow
//! components/hal/esp32c6/include/hal/parlio_ll.h:413-430.
//! Unpack order: components/hal/include/hal/parlio_types.h:28-50;
//! FOSC approximation: components/soc/esp32c6/include/soc/clk_tree_defs.h:47.
//! Transactions complete at MMIO boundaries; no wire-timing or FIFO-backpressure claim.
use esp_periph::{Device, RegRam, WriteEffect};
use std::collections::VecDeque;

// Keep transaction storage out of the hot peripheral layout until MMIO uses it.
#[derive(Default)]
pub struct Parlio {
    pub(crate) tx: Option<Box<Tx>>,
}
impl Parlio {
    pub fn new() -> Self { Self::default() }
    pub fn tx(&mut self) -> &mut Tx { self.tx.get_or_insert_with(Default::default) }
    pub fn set_clock(&mut self, value: u32) { self.tx().set_clock(value); }
}
impl Device for Parlio {
    fn read(&mut self, off: u32) -> u32 { self.tx().read(off) }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect { self.tx().write(off, value) }
    fn irq_sources(&self) -> u64 { self.tx.as_ref().map_or(0, |tx| tx.irq_sources()) }
}

pub struct Tx {
    ram: RegRam,
    pub fifo: VecDeque<u8>,
    cfg: u32,
    raw: u32,
    ena: u32,
    remaining: usize,
    samples: Vec<u16>,
    pub clock_hz: u32,
    pub done: Option<Vec<u16>>,
}

impl Default for Tx { fn default() -> Self { Self::new() } }

impl Tx {
    pub fn new() -> Self {
        Self {
            ram: RegRam::new(),
            fifo: VecDeque::new(),
            cfg: 0,
            raw: 0,
            ena: 0,
            remaining: 0,
            samples: Vec::new(),
            clock_hz: 40_000_000,
            done: None,
        }
    }
    pub fn width(&self) -> u8 {
        16u8.checked_shr((self.cfg >> 27) & 7).unwrap_or(0)
    }
    pub fn running(&self) -> bool {
        self.cfg & (1 << 19) != 0 && self.remaining > 0
    }
    pub fn set_clock(&mut self, value: u32) {
        self.clock_hz = if value & (1 << 18) == 0 {
            0
        } else {
            let source = match (value >> 16) & 3 {
                0 => 40_000_000,
                1 => 240_000_000,
                2 => 17_500_000,
                _ => 0,
            };
            source / ((value & 0xffff) + 1)
        };
        if value & (1 << 19) != 0 {
            self.fifo.clear();
            self.cfg &= !(1 << 19);
            self.remaining = 0;
            self.samples.clear();
        }
    }
}

impl Device for Tx {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x08 => self.cfg,
            0x10 => {
                if !self.fifo.is_empty() {
                    1 << 31
                } else {
                    0
                }
            }
            0x14 => self.ena,
            0x18 => self.raw,
            0x1c => self.raw & self.ena,
            0x3fc => 0x220_2240,
            _ => self.ram.read(off),
        }
    }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        match off {
            0x08 => {
                let start = value & (1 << 19) != 0 && self.cfg & (1 << 19) == 0;
                self.cfg = value;
                if value & (1 << 30) != 0 {
                    self.fifo.clear();
                    self.samples.clear();
                }
                if start {
                    self.remaining = ((value >> 2) & 0xffff) as usize;
                    self.samples.clear();
                }
            }
            0x14 => self.ena = value & 7,
            0x20 => self.raw &= !value,
            _ => self.ram.write(off, value),
        }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 {
        u64::from(self.raw & self.ena != 0)
    }
 }

impl Tx {
    /// Functional TX completion after the complete DMA payload is staged.
    /// FIFO backpressure and wire timing are not modeled.
    pub fn complete(&mut self) {
        let width = self.width();
        if !self.running() || width == 0 || self.clock_hz == 0 {
            return;
        }
        let bytes = if width == 16 { 2 } else { 1 };
        let count = 8 * bytes / usize::from(width);
        if self.remaining < bytes || !self.remaining.is_multiple_of(bytes) || self.fifo.len() < self.remaining { return; }
        while self.remaining >= bytes {
            let mut value = u16::from(self.fifo.pop_front().unwrap());
            if bytes == 2 {
                value |= u16::from(self.fifo.pop_front().unwrap()) << 8;
            }
            let mask = u16::MAX >> (16 - width);
            for i in 0..count {
                let shift = if self.cfg & (1 << 26) == 0 {
                    i
                } else {
                    count - i - 1
                } * usize::from(width);
                self.samples.push((value >> shift) & mask);
            }
            self.remaining -= bytes;
            if self.remaining == 0 {
                self.raw |= 4;
                self.cfg &= !(1 << 19);
                self.done = Some(std::mem::take(&mut self.samples));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fifo_readiness_lane_samples_and_eof_interrupt() {
        let mut p = Tx::new();
        p.set_clock((1 << 18) | (1 << 16) | 59); // 240MHz / 60
        assert_eq!(p.read(0x10), 0);
        p.fifo.extend([0b11_10_01_00, 0b00_01_10_11]);
        assert_eq!(p.read(0x10), 1 << 31);
        p.write(0x14, 4);
        p.write(0x08, (2 << 2) | (1 << 19) | (3 << 27) | (1 << 26));
        p.complete();
        let frame = p.done.take().unwrap();
        assert_eq!(frame, vec![3, 2, 1, 0, 0, 1, 2, 3]);
        assert_eq!(p.width(), 2);
        assert_eq!(p.clock_hz, 4_000_000);
        assert_eq!(p.irq_sources(), 1);
        p.write(0x20, 4);
        assert_eq!(p.irq_sources(), 0);
        p.write(0x08, (1 << 2) | (1 << 19) | (3 << 27));
        p.fifo.push_back(0b11_10_01_00);
        p.complete();
        assert_eq!(p.done.take().unwrap(), vec![0, 1, 2, 3]);
    }
    #[test]
    fn absent_clock_and_fifo_do_not_complete_transfer() {
        let mut p = Tx::new();
        p.write(0x08, (1 << 2) | (1 << 19) | (1 << 27));
        p.complete();
        assert!(p.done.is_none());
        assert!(p.running());
        p.set_clock(0);
        p.fifo.push_back(5);
        p.complete();
        assert!(p.done.is_none());
        assert_eq!(p.read(0x10), 1 << 31);
        p.set_clock(1 << 18);
        p.complete();
        assert_eq!(p.done.take().unwrap(), vec![5]);
    }
    #[test]
    fn widths_reset_and_invalid_formats() {
        for (select, expected) in [(0, vec![0x1287]), (1, vec![0x87, 0x12]),
            (2, vec![7, 8, 2, 1]), (3, vec![3, 1, 0, 2, 2, 0, 1, 0]),
            (4, vec![1, 1, 1, 0, 0, 0, 0, 1, 0, 1, 0, 0, 1, 0, 0, 0])] {
            let mut p = Tx::new();
            p.fifo.extend([0x87, 0x12]);
            p.write(8, (2 << 2) | (1 << 19) | (select << 27));
            p.complete();
            assert_eq!(p.done.take().unwrap(), expected);
            assert!(!p.running());
        }
        let mut p = Tx::new();
        p.fifo.push_back(1);
        p.write(8, (1 << 2) | (1 << 19));
        p.complete();
        assert!(p.done.is_none(), "16-bit samples require even byte lengths");
        p.write(8, 0);
        p.write(8, (1 << 2) | (1 << 19) | (7 << 27));
        p.complete();
        assert!(p.done.is_none(), "reserved width must not shift or consume input");
        p.write(8, 1 << 30);
        assert!(p.fifo.is_empty());
        p.fifo.push_back(1);
        p.set_clock((1 << 18) | (1 << 19));
        assert!(p.fifo.is_empty());
        assert!(!p.running());
        for (source, hz) in [(0, 40_000_000), (1, 240_000_000), (2, 17_500_000), (3, 0)] {
            p.set_clock((1 << 18) | (source << 16) | 4);
            assert_eq!(p.clock_hz, hz / 5);
        }
    }
}
