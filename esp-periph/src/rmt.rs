use std::collections::VecDeque;
use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;
use emu_core::ClockDomain;

// ------------------------------------------------------------------ RMT (TX channels 0-3) — enough for WS2812 via the legacy driver
pub const RMT_MEM_WORDS: usize = 48;
#[derive(Clone, Default)]
pub struct RmtTxCh {
    pub conf0: u32, pub tx_lim: u32, pub carrier: u32,
    pub running: bool, pub rd: usize, pub since_thr: u32, pub wr: usize,
    pub mem_empty: bool,
    /// An end marker has been decoded; any preceding pulse must finish first.
    end_pending: bool,
    loop_count: u32,
    pub acc_cycles: i64,
    pub bits: Vec<bool>,
}
pub struct Rmt {
    pub ch: [RmtTxCh; 4],
    pub mem: [u32; RMT_MEM_WORDS * 8],
    pub int_raw: u32, pub int_ena: u32, pub sys_conf: u32,
    ram: RegRam,
    /// completed transmissions: (channel, bits)
    pub done: Vec<(usize, Vec<bool>)>,
    pub tx_count: u64,
    cpu_per_apb: i64,
    running: u8,
    /// IDF v5.5.4 components/soc/esp32s3/register/soc/rmt_struct.h:117-120
    /// puts DMA_ACCESS_EN at bit 25 (v4.4.8 calls bits 25..31 reserved); hal/esp32s3/include/hal/rmt_ll.h:237-240 restricts it to TX3.
    pub dma_fifo: VecDeque<u32>,
}
impl Rmt {
    pub fn new(cpu_hz: u64) -> Self { Rmt { running: 0, cpu_per_apb: (cpu_hz / crate::APB_HZ) as i64, ch: Default::default(), mem: [0; RMT_MEM_WORDS * 8], int_raw: 0, int_ena: 0, sys_conf: 0, ram: RegRam::new(), done: Vec::new(), tx_count: 0, dma_fifo: VecDeque::new() } }
    pub fn is_running(&self) -> bool { self.running != 0 }
    pub fn irq(&self) -> bool { self.int_raw & self.int_ena != 0 }
    pub fn read(&self, off: u32) -> u32 {
        match off {
            // TX_START / MEM_RD_RST / APB_MEM_RST / TX_STOP / AFIFO_RST / CONF_UPDATE are self-clearing command bits: they read as 0
            0x20..=0x2c => { let c = &self.ch[((off - 0x20) / 4) as usize]; c.conf0 & !(1 << 0) & !(1 << 1) & !(1 << 2) & !(1 << 7) & !(1 << 23) & !(1 << 24) }
            0x50..=0x5c => { let n = ((off - 0x50) / 4) as usize; let c = &self.ch[n]; ((c.wr as u32 + (n as u32) * 48) << 11) | ((c.mem_empty as u32) << 25) | if c.running { 2 << 22 } else { 0 } }
            0x70 => self.int_raw, 0x74 => self.int_raw & self.int_ena, 0x78 => self.int_ena,
            0x80..=0x8c => self.ch[((off - 0x80) / 4) as usize].carrier,
            0xa0..=0xac => self.ch[((off - 0xa0) / 4) as usize].tx_lim,
            0xc0 => self.sys_conf, 0xcc => 0x2101271,
            0x800..=0xbfc => self.mem[((off - 0x800) / 4) as usize],
            _ => self.ram.read(off),
        }
    }
    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x0..=0xc => { let n = ((off) / 4) as usize; let c = &mut self.ch[n]; if c.wr < RMT_MEM_WORDS { self.mem[n * RMT_MEM_WORDS + c.wr] = v; c.wr += 1; } }
            0x20..=0x2c => {
                let n = ((off - 0x20) / 4) as usize;
                let c = &mut self.ch[n];
                c.conf0 = v & !(1 << 7);   // TX_STOP is a command, not state: a read-modify-write after a stop must not stop the next start
                if n == 3 && (v & (1 << 23) != 0 || v & (1 << 25) == 0) { self.dma_fifo.clear(); }
                if v & (1 << 2) != 0 { c.wr = 0; }                          // APB_MEM_RST
                if v & (1 << 1) != 0 { c.rd = 0; c.mem_empty = false; c.end_pending = false; }      // MEM_RD_RST
                if v & (1 << 0) != 0 { c.running = true; c.rd = 0; c.mem_empty = false; c.end_pending = false; c.since_thr = 0; c.acc_cycles = 0; c.bits.clear(); c.loop_count = 0; }   // TX_START
                if v & (1 << 7) != 0 { c.running = false; }                 // TX_STOP
                self.running = (self.running & !(1 << n)) | ((c.running as u8) << n);
            }
            0x78 => self.int_ena = v, 0x7c => self.int_raw &= !v,
            0x80..=0x8c => self.ch[((off - 0x80) / 4) as usize].carrier = v,
            0xa0..=0xac => { let c = &mut self.ch[((off - 0xa0) / 4) as usize]; c.tx_lim = v & !(1 << 20); if v & (1 << 20) != 0 { c.loop_count = 0; } },
            0xc0 => self.sys_conf = v,
            0x800..=0xbfc => self.mem[((off - 0x800) / 4) as usize] = v,
            _ => self.ram.write(off, v),
        }
    }
    /// Advance transmitters by CPU cycles; symbols are consumed at their programmed duration.
    pub fn tick(&mut self, cycles: u64) {
        if !self.is_running() { return; }
        for n in 0..4 {
            let c = &mut self.ch[n];
            if !c.running { continue; }
            c.acc_cycles += cycles as i64;
            let div = ((c.conf0 >> 8) & 0xff).max(1) as i64;
            let cycles_per_tick = self.cpu_per_apb * div;   // RMT clock = APB 80 MHz / div
            let mem_words = (((c.conf0 >> 16) & 0xf).max(1) as usize) * RMT_MEM_WORDS;
            let base = n * RMT_MEM_WORDS;
            let dma = n == 3 && c.conf0 & (1 << 25) != 0;
            let mut guard = 0;
            while (c.acc_cycles > 0 || (c.acc_cycles == 0 && c.end_pending)) && guard < 4096 {
                guard += 1;
                if c.end_pending {
                    c.end_pending = false;
                    if c.conf0 & (1 << 3) != 0 {
                        let progressed = c.rd != 0;
                        // Continuous output is not a completed WS2812 frame. Retain only
                        // the current lap, so a buzzer cannot accumulate host memory.
                        c.bits.clear();
                        c.rd = 0;
                        if c.tx_lim & (1 << 19) != 0 {
                            c.loop_count += 1;
                            if c.loop_count >= ((c.tx_lim >> 9) & 0x3ff).max(1) {
                                self.int_raw |= 1 << (12 + n); // TX_LOOP
                                c.loop_count = 0;
                                if c.tx_lim & (1 << 21) != 0 { c.running = false; break; }
                            }
                        }
                        if !progressed { c.acc_cycles = 0; break; }
                        continue;
                    }
                    c.running = false;
                    self.int_raw |= 1 << n;
                    self.tx_count += 1;
                    self.done.push((n, std::mem::take(&mut c.bits)));
                    break;
                }
                // No-wrap exhaustion is an empty-memory error, not an end marker
                // (S3 TRM 37.4). Invalid block allocations must not index host RAM.
                let repeats = c.conf0 & ((1 << 3) | (1 << 4)) != 0; // continuous or wrap TX
                if !dma && (mem_words > self.mem.len() - base || (c.rd >= mem_words && !repeats)) {
                    c.running = false;
                    c.mem_empty = true;
                    self.int_raw |= 1 << (4 + n);
                    break;
                }
                if !dma && c.rd != 0 && c.rd.is_multiple_of(mem_words) && c.conf0 & (1 << 3) != 0 { c.bits.clear(); }
                let sym = if dma {
                    let Some(symbol) = self.dma_fifo.pop_front() else { c.acc_cycles = 0; break; };
                    symbol
                } else { self.mem[base + (c.rd % mem_words)] };
                let (d0, l0, d1, l1) = ((sym & 0x7fff) as i64, sym & 0x8000 != 0, ((sym >> 16) & 0x7fff) as i64, sym & 0x8000_0000 != 0);
                if d0 == 0 { // end marker
                    c.end_pending = true;
                    continue;
                }
                // decode WS2812 bit: compare high vs low durations
                let high = if l0 { d0 } else { 0 } + if l1 { d1 } else { 0 };
                let low = if !l0 { d0 } else { 0 } + if !l1 { d1 } else { 0 };
                c.bits.push(high > low);
                c.acc_cycles -= (d0 + d1) * cycles_per_tick;
                c.rd += 1;
                c.since_thr += 1;
                if !dma && c.tx_lim & 0x1ff != 0 && c.since_thr >= c.tx_lim & 0x1ff { c.since_thr = 0; self.int_raw |= 1 << (8 + n); }   // TX_THR_EVENT
                c.end_pending = d1 == 0;
            }
            if !c.running { self.running &= !(1 << n); }
        }
    }
}

impl Device for Rmt {
    fn read(&mut self, off: u32) -> u32 { Rmt::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { Rmt::write(self, off, v); WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { self.irq() as u64 }
    fn clock(&self) -> Option<ClockDomain> { Some(ClockDomain::Cpu) }
    fn tick(&mut self, cycles: u64) { Rmt::tick(self, cycles) }
}

#[cfg(test)]
mod tx_stop_tests {
    use super::*;
    #[test]
    fn running_mask_tracks_stop_and_completion_on_each_channel() {
        let mut r = Rmt::new(160_000_000);
        assert!(!r.is_running());
        for n in 0..4 {
            r.write(0x20 + n * 4, 1);
            r.write(0x20 + ((n + 1) % 4) * 4, 1);
            r.write(0x20 + n * 4, 1 << 7);
            assert!(r.is_running());
            r.tick(1);
            assert!(!r.is_running());
            assert!(!r.ch.iter().any(|c| c.running));
            assert_eq!(r.done.pop().unwrap().0, ((n + 1) % 4) as usize);
        }
    }
    /// The IDF 4.4 legacy driver stops a channel (TX_STOP = 1) and later starts it with a
    /// read-modify-write of CONF0: the stop must not come back and cancel the start.
    #[test]
    fn tx_stop_is_a_command_not_state() {
        let mut r = Rmt::new(240_000_000);
        r.write(0x20, 1 << 7);                    // stop
        assert_eq!(r.read(0x20) & (1 << 7), 0);
        let v = r.read(0x20) | 1;                 // RMW start
        r.write(0x20, v);
        assert!(r.ch[0].running);
    }
}

#[cfg(test)]
mod dma_tests {
    use super::*;
    #[test]
    fn dma_reset_disable_starvation_and_threshold_suppression() {
        let mut r = Rmt::new(240_000_000);
        r.write(0x2c, (1 << 25) | 1);
        r.tick(1000);
        assert!(r.is_running());
        assert_eq!(r.int_raw, 0);
        r.write(0xac, 1);
        r.dma_fifo.extend([0x8000 | 35 | (15 << 16), 0]);
        r.tick(1000);
        assert_eq!(r.done, [(3, vec![true])]);
        assert_eq!(r.int_raw, 1 << 3);
        r.dma_fifo.push_back(123);
        r.write(0x2c, (1 << 25) | (1 << 23));
        assert!(r.dma_fifo.is_empty());
        r.dma_fifo.push_back(456);
        r.write(0x2c, 0);
        assert!(r.dma_fifo.is_empty());
    }
}
