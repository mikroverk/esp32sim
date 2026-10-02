//! Original ESP32 RMT transmitter: eight channels sharing 512 words of RAM.
use emu_core::ClockDomain;
use esp_periph::{Device, RegRam, WriteEffect};

const CHANNELS: usize = 8;
const WORDS: usize = 64;
pub const SIGNAL0: usize = 87;
const CONTINUOUS: u32 = 1 << 6;
const APB_SOURCE: u32 = 1 << 17;
const IDLE_LEVEL: u32 = 1 << 18;
const IDLE_ENABLE: u32 = 1 << 19;

pub struct RmtFrame {
    pub channel: usize,
    /// (level, duration in 80 MHz APB ticks), before GPIO matrix inversion.
    pub pulses: Vec<(bool, u64)>,
    pub truncated: bool,
}

#[derive(Default)]
struct Channel {
    running: bool,
    rd: usize,
    fifo_rd: usize,
    fifo_wr: usize,
    phase: u8,
    symbol: u32,
    remaining: u64,
    since_threshold: u32,
    errors: u32,
    level: bool,
    pulses: Vec<(bool, u64)>,
    truncated: bool,
}

pub struct ClassicRmt {
    regs: RegRam,
    mem: [u32; CHANNELS * WORDS],
    ch: [Channel; CHANNELS],
    raw: u32,
    ena: u32,
    pub clock_enabled: bool,
    pub outputs: Vec<(usize, bool)>,
    pub done: Vec<RmtFrame>,
    pub tx_count: u64,
}

impl Default for ClassicRmt {
    fn default() -> Self {
        Self::new()
    }
}

impl ClassicRmt {
    pub fn new() -> Self {
        let mut regs = RegRam::new();
        for n in 0..CHANNELS as u32 {
            regs.write(0x20 + n * 8, 0x3110_0002);
            regs.write(0x24 + n * 8, 0xf20);
            regs.write(0xb0 + n * 4, 0x0040_0040);
            regs.write(0xd0 + n * 4, 128);
        }
        regs.write(0xfc, 0x1602_2600);
        Self {
            regs,
            mem: [0; CHANNELS * WORDS],
            ch: Default::default(),
            raw: 0,
            ena: 0,
            clock_enabled: false,
            outputs: (0..CHANNELS).map(|n| (n, false)).collect(),
            done: Vec::new(),
            tx_count: 0,
        }
    }

    fn conf1(&self, n: usize) -> u32 {
        self.regs.read(0x24 + n as u32 * 8)
    }
    fn words(&self, n: usize) -> usize {
        ((self.regs.read(0x20 + n as u32 * 8) >> 24) & 15) as usize * WORDS
    }
    fn divider(&self, n: usize) -> u64 {
        let div = (self.regs.read(0x20 + n as u32 * 8) & 255) as u64;
        let div = if div == 0 { 256 } else { div };
        div * if self.conf1(n) & APB_SOURCE != 0 {
            1
        } else {
            80
        }
    }
    fn powered(&self) -> bool {
        self.clock_enabled && self.regs.read(0x20) & (1 << 30) == 0
    }
    fn level(&mut self, n: usize, level: bool) {
        self.ch[n].level = level;
        self.outputs.push((n, level));
    }
    fn idle(&mut self, n: usize) {
        let conf = self.conf1(n);
        if conf & IDLE_ENABLE != 0 {
            self.level(n, conf & IDLE_LEVEL != 0);
        }
    }
    fn error(&mut self, n: usize, status: u32) {
        self.ch[n].errors |= status;
        self.raw |= 1 << (3 * n + 2);
    }
    fn finish(&mut self, n: usize) {
        self.tx_count += 1;
        let frame = RmtFrame {
            channel: n,
            pulses: std::mem::take(&mut self.ch[n].pulses),
            truncated: std::mem::take(&mut self.ch[n].truncated),
        };
        // Keep only the latest lap if a caller advances several continuous loops at once.
        self.done.retain(|frame| frame.channel != n);
        self.done.push(frame);
        self.idle(n);
        if self.conf1(n) & CONTINUOUS != 0 {
            self.ch[n].rd = 0;
            self.ch[n].phase = 0;
            self.ch[n].remaining = self.divider(n); // one idle tick between loops
        } else {
            self.ch[n].running = false;
            self.raw |= 1 << (3 * n);
        }
    }

    fn advance(&mut self, n: usize, mut ticks: u64) {
        while self.ch[n].running {
            if ticks < self.ch[n].remaining {
                self.ch[n].remaining -= ticks;
                break;
            }
            ticks -= self.ch[n].remaining;
            self.ch[n].remaining = 0;
            if self.ch[n].phase == 2 {
                self.ch[n].phase = 0;
                self.ch[n].rd += 1;
                self.ch[n].since_threshold += 1;
                let limit = self.regs.read(0xd0 + n as u32 * 4) & 0x1ff;
                if limit != 0 && self.ch[n].since_threshold >= limit {
                    self.ch[n].since_threshold = 0;
                    self.raw |= 1 << (24 + n);
                }
            }
            if self.ch[n].phase == 0 {
                let words = self.words(n);
                if words == 0 || words > self.mem.len() {
                    self.error(n, 1 << 29);
                    self.ch[n].running = false;
                    self.idle(n);
                    break;
                }
                if self.ch[n].rd >= words {
                    if self.regs.read(0xf0) & 2 != 0 {
                        self.ch[n].rd = 0;
                    } else {
                        self.error(n, 1 << 29);
                        self.ch[n].running = false;
                        self.idle(n);
                        break;
                    }
                }
                self.ch[n].symbol = self.mem[(n * WORDS + self.ch[n].rd) % self.mem.len()];
            }
            let half = (self.ch[n].symbol >> (16 * self.ch[n].phase)) as u16;
            self.ch[n].phase += 1;
            let duration = u64::from(half & 0x7fff) * self.divider(n);
            self.level(n, half & 0x8000 != 0);
            if duration == 0 {
                self.finish(n);
                continue;
            }
            let c = &mut self.ch[n];
            c.remaining = duration;
            // ponytail: retain at most 4096 symbols per frame; stream the observer if larger strips need it.
            if c.pulses.len() < 8192 {
                c.pulses.push((c.level, duration));
            } else {
                c.truncated = true;
            }
        }
    }

    fn fifo_index(&mut self, n: usize, write: bool) -> Option<usize> {
        if self.regs.read(0xf0) & 1 != 0 {
            return None;
        }
        let pos = if write {
            self.ch[n].fifo_wr
        } else {
            self.ch[n].fifo_rd
        };
        if pos >= self.words(n) || self.words(n) > self.mem.len() {
            self.error(n, 1 << if write { 30 } else { 31 });
            return None;
        }
        if write {
            self.ch[n].fifo_wr += 1;
        } else {
            self.ch[n].fifo_rd += 1;
        }
        Some((n * WORDS + pos) % self.mem.len())
    }
}

impl Device for ClassicRmt {
    fn read(&mut self, off: u32) -> u32 {
        match off {
            0x00..=0x1c => self
                .fifo_index((off / 4) as usize, false)
                .map_or(0, |i| self.mem[i]),
            0x60..=0x7c => {
                let n = ((off - 0x60) / 4) as usize;
                let c = &self.ch[n];
                c.errors
                    | (u32::from(c.running) << 24)
                    | (((n * WORDS + c.rd) as u32 & 0x3ff) << 12)
                    | (n * WORDS) as u32
            }
            0x80..=0x9c => {
                let c = &self.ch[((off - 0x80) / 4) as usize];
                c.fifo_rd as u32 | (c.fifo_wr as u32) << 16
            }
            0xa0 => self.raw,
            0xa4 => self.raw & self.ena,
            0xa8 => self.ena,
            0xac => 0,
            0x800..=0xffc => self.mem[((off - 0x800) / 4) as usize],
            _ => self.regs.read(off),
        }
    }
    fn write(&mut self, off: u32, value: u32) -> WriteEffect {
        match off {
            0x00..=0x1c => {
                if let Some(i) = self.fifo_index((off / 4) as usize, true) {
                    self.mem[i] = value;
                }
            }
            0x24..=0x5c if off % 8 == 4 => {
                let n = ((off - 0x24) / 8) as usize;
                self.regs.write(off, value & 0xfffff & !0x1d & !(1 << 16));
                let c = &mut self.ch[n];
                if value & (1 << 4) != 0 {
                    c.fifo_rd = 0;
                    c.fifo_wr = 0;
                    c.errors &= !0xc000_0000;
                }
                if value & (1 << 3) != 0 {
                    c.rd = 0;
                    c.phase = 0;
                    c.remaining = 0;
                    c.errors &= !(1 << 29);
                }
                if value & 1 != 0 {
                    c.running = true;
                    c.phase = 0;
                    c.remaining = 0;
                    c.since_threshold = 0;
                    c.pulses.clear();
                    c.truncated = false;
                }
                // Ownership protects the receiver. IDF's TX driver leaves the reset owner bit set.
                if value & 2 != 0 && value & (1 << 5) == 0 {
                    self.error(n, 1 << 27);
                }
                if !self.ch[n].running {
                    self.idle(n);
                }
                if self.powered() {
                    self.advance(n, 0);
                }
            }
            0x60..=0xa4 => {}
            0xa8 => self.ena = value,
            0xac => self.raw &= !value,
            0xd0..=0xec => self.regs.write(off, value & 0x1ff),
            0xf0 => self.regs.write(off, value & 3),
            0x800..=0xffc => self.mem[((off - 0x800) / 4) as usize] = value,
            _ => self.regs.write(off, value),
        }
        WriteEffect::NONE
    }
    fn irq_sources(&self) -> u64 {
        u64::from(self.raw & self.ena != 0)
    }
    fn clock(&self) -> Option<ClockDomain> {
        Some(ClockDomain::Apb)
    }
    fn tick(&mut self, ticks: u64) {
        if self.powered() {
            for n in 0..CHANNELS {
                self.advance(n, ticks);
            }
        }
    }
    fn has_deadline(&self) -> bool {
        true
    }
    fn next_deadline(&self) -> Option<u64> {
        if !self.powered() {
            return None;
        }
        self.ch
            .iter()
            .filter(|c| c.running)
            .map(|c| c.remaining.max(1))
            .min()
    }
}

pub struct RmtObserver {
    frames: [Option<RmtFrame>; 40],
}

impl Default for RmtObserver {
    fn default() -> Self {
        Self {
            frames: std::array::from_fn(|_| None),
        }
    }
}

impl RmtObserver {
    pub fn observe(&mut self, mut frame: RmtFrame, pin: u8, inverted: bool) -> Option<Vec<bool>> {
        for (level, _) in &mut frame.pulses {
            *level ^= inverted;
        }
        let bits = Self::ws2812_bits(&frame);
        self.frames[pin as usize] = Some(frame);
        bits
    }

    fn ws2812_bits(frame: &RmtFrame) -> Option<Vec<bool>> {
        if frame.truncated {
            return None;
        }
        let mut pulses: Vec<(bool, u64)> = Vec::new();
        for &(level, ticks) in &frame.pulses {
            if let Some(last) = pulses.last_mut().filter(|last| last.0 == level) {
                last.1 += ticks;
            } else {
                pulses.push((level, ticks));
            }
        }
        let start = usize::from(pulses.first().is_some_and(|&(level, _)| !level));
        let mut bits = Vec::new();
        for pair in pulses[start..].chunks(2) {
            let [(true, high), (false, low)] = pair else {
                return None;
            };
            // WS2812B: T0H=0.4 us, T1H=0.8 us, each +/-150 ns; one APB tick is 12.5 ns.
            let (bit, low_range) = match high {
                20..=44 => (false, 56..=80),
                52..=76 => (true, 24..=48),
                _ => return None,
            };
            let last = start + (bits.len() + 1) * 2 == pulses.len();
            if !low_range.contains(low) && !(last && *low >= 4000) {
                return None;
            }
            bits.push(bit);
        }
        (!bits.is_empty() && bits.len().is_multiple_of(24)).then_some(bits)
    }

    pub fn report(&self) -> String {
        let mut lines = Vec::new();
        for (pin, frame) in self.frames.iter().enumerate() {
            let Some(frame) = frame else {
                continue;
            };
            let suffix = if frame.truncated {
                " (capture truncated)"
            } else {
                ""
            };
            lines.push(format!(
                "[emu] rmt GPIO{pin} channel{}: {} pulses{suffix}",
                frame.channel,
                frame.pulses.len()
            ));
            let shown = frame.pulses.len().min(32);
            let pulses: Vec<_> = frame.pulses[..shown]
                .iter()
                .map(|&(level, ticks)| format!("{}:{ticks}", level as u8))
                .collect();
            let suffix = if shown < frame.pulses.len() {
                " ..."
            } else {
                ""
            };
            lines.push(format!(
                "[emu] rmt GPIO{pin} level:APB-ticks (12.5ns): {}{suffix}",
                pulses.join(" ")
            ));
            if let Some(bits) = Self::ws2812_bits(frame) {
                let mut strip = esp_soc::devices::Ws2812Chain::new(bits.len() / 24);
                strip.from_bits(&bits);
                lines.push(format!("[emu] rmt GPIO{pin} WS2812 RGB {:?}", strip.leds));
            }
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod observer_tests {
    use super::*;

    fn frame(bytes: &[u8], inverted: bool) -> RmtFrame {
        let mut pulses = Vec::new();
        for byte in bytes {
            for shift in (0..8).rev() {
                let one = byte & (1 << shift) != 0;
                pulses.push((!inverted, if one { 64 } else { 32 }));
                pulses.push((inverted, if one { 32 } else { 64 }));
            }
        }
        RmtFrame {
            channel: 0,
            pulses,
            truncated: false,
        }
    }

    #[test]
    fn ws2812_observer_decodes_grb_and_keeps_latest_frame_per_pin() {
        let mut observer = RmtObserver::default();
        assert_eq!(
            observer
                .observe(frame(&[0, 255, 64], true), 4, true)
                .unwrap()
                .len(),
            24
        );
        assert!(observer
            .report()
            .contains("GPIO4 WS2812 RGB [[255, 0, 64]]"));
        observer.observe(frame(&[1, 2, 3], false), 5, false);
        let mut replacement = frame(&[6, 7, 8], false);
        replacement.channel = 7;
        observer.observe(replacement, 4, false);
        let report = observer.report();
        assert!(report.contains("GPIO4 channel7"));
        assert!(report.contains("GPIO4 WS2812 RGB [[7, 6, 8]]"));
        assert!(report.contains("GPIO5 WS2812 RGB [[2, 1, 3]]"));
    }

    #[test]
    fn raw_or_truncated_pulses_are_not_reported_as_pixels() {
        let mut raw = frame(&[0, 0, 0], false);
        raw.pulses[0].1 = 45;
        assert!(RmtObserver::ws2812_bits(&raw).is_none());
        let mut truncated = frame(&[0, 0, 0], false);
        truncated.truncated = true;
        assert!(RmtObserver::ws2812_bits(&truncated).is_none());
        let mut reset = frame(&[0, 0, 0], false);
        reset.pulses.push((false, 4000));
        assert_eq!(RmtObserver::ws2812_bits(&reset).unwrap().len(), 24);
        let mut short = frame(&[0, 0], false);
        assert!(RmtObserver::ws2812_bits(&short).is_none());
        short.pulses.push((true, 32));
        assert!(RmtObserver::ws2812_bits(&short).is_none());
    }
}
#[cfg(test)]
mod tests {
    use super::ClassicRmt;
    use esp_periph::Device;

    const APB: u32 = 1 << 17;
    const OWNER: u32 = 1 << 5;
    const IDLE: u32 = 1 << 19;

    fn model() -> ClassicRmt {
        let mut r = ClassicRmt::new();
        r.clock_enabled = true;
        r.write(0xf0, 1);
        r
    }

    fn conf1(n: u32) -> u32 {
        0x24 + 8 * n
    }
    fn ram(word: u32) -> u32 {
        0x800 + 4 * word
    }
    fn item(high: u32, low: u32) -> u32 {
        high | (1 << 15) | (low << 16)
    }
    fn configure(r: &mut ClassicRmt, n: u32, blocks: u32, div: u32, apb: bool) {
        r.write(0x20 + 8 * n, (blocks << 24) | div);
        r.write(conf1(n), OWNER | IDLE | if apb { APB } else { 0 });
    }
    fn start(r: &mut ClassicRmt, n: u32) {
        let conf = r.read(conf1(n));
        r.write(conf1(n), conf | 1 | (1 << 3) | (1 << 4));
    }

    #[test]
    fn eight_channels_have_independent_ram_and_tx_interrupts() {
        let mut r = model();
        let ends = (0..8).fold(0, |bits, n| bits | (1 << (3 * n)));
        r.write(0xa8, ends);
        for n in 0..8 {
            configure(&mut r, n, 1, 1, true);
            r.write(ram(64 * n), item(n + 1, 2));
            r.write(ram(64 * n + 1), 0);
            start(&mut r, n);
        }
        assert_eq!(r.read(0xa0), 0);
        r.tick(3);
        assert_eq!(r.read(0xa4), 1);
        r.tick(7);
        assert_eq!(r.read(0xa4), ends);
        assert_eq!(r.irq_sources(), 1);
        for n in 0..8 {
            assert_eq!(r.read(0x60 + 4 * n) & (7 << 24), 0);
            assert_eq!(r.read(conf1(n)) & (1 | (1 << 3) | (1 << 4)), 0);
        }
        r.write(0xac, ends);
        assert_eq!(r.read(0xa0), 0);
        assert_eq!(r.irq_sources(), 0);
    }

    #[test]
    fn borrowed_blocks_share_ram_and_wrap_after_word_511() {
        for n in [2, 7] {
            let mut r = model();
            configure(&mut r, n, 2, 1, true);
            for i in 0..65 {
                r.write(ram((64 * n + i) % 512), item(1, 1));
            }
            r.write(ram((64 * n + 65) % 512), 0);
            start(&mut r, n);
            r.tick(129);
            assert_eq!(r.read(0xa0) & (1 << (3 * n)), 0);
            r.tick(1);
            assert_eq!(r.read(0xa0), 1 << (3 * n));
        }
    }

    #[test]
    fn zero_divider_is_256_for_both_clock_sources() {
        for apb in [true, false] {
            let mut r = model();
            configure(&mut r, 0, 1, 0, apb);
            r.write(ram(0), item(1, 1));
            r.write(ram(1), 0);
            start(&mut r, 0);
            let half = if apb { 256 } else { 256 * 80 };
            assert_eq!(r.next_deadline(), Some(half));
            r.tick(half - 1);
            assert_eq!(r.read(0xa0), 0);
            assert_eq!(r.next_deadline(), Some(1));
            r.tick(1);
            assert_eq!(r.next_deadline(), Some(half));
            r.tick(half - 1);
            assert_eq!(r.read(0xa0), 0);
            r.tick(1);
            assert_eq!(r.read(0xa0), 1);
            assert_eq!(r.next_deadline(), None);
        }
    }

    #[test]
    fn zero_duration_ends_at_the_first_or_second_half_boundary() {
        for first in [0, 7] {
            let mut r = model();
            configure(&mut r, 0, 1, 3, true);
            let second = if first == 0 { 32767 } else { 0 };
            r.write(ram(0), item(first, second));
            start(&mut r, 0);
            if first != 0 {
                r.tick(20);
                assert_eq!(r.read(0xa0), 0);
                assert_eq!(r.next_deadline(), Some(1));
                r.tick(1);
            }
            assert_eq!(r.read(0xa0), 1);
            assert_eq!(r.read(0xa4), 0, "raw completion latches while masked");
            r.write(0xa8, 1);
            assert_eq!(r.read(0xa4), 1);
            assert_eq!(r.next_deadline(), None);
            r.write(0xac, 1);
            r.write(ram(0), item(2, 0));
            start(&mut r, 0);
            r.tick(5);
            assert_eq!(r.read(0xa0), 0);
            r.tick(1);
            assert_eq!(r.read(0xa0), 1, "a reset pointer starts the next frame");
        }
    }

    #[test]
    fn threshold_interrupts_allow_refilling_a_wrapped_96_word_stream() {
        let mut r = model();
        configure(&mut r, 0, 1, 1, true);
        r.write(0xf0, 3);
        r.write(0xd0, 32);
        r.write(0xa8, 1 | (1 << 24));
        for i in 0..64 {
            r.write(ram(i), item(4, 4));
        }
        start(&mut r, 0);
        r.tick(255);
        assert_eq!(r.read(0xa4), 0);
        r.tick(1);
        assert_eq!(r.read(0xa4), 1 << 24);
        r.write(0xac, 1 << 24);
        for i in 0..32 {
            r.write(ram(i), item(4, 4));
        }
        r.tick(256);
        assert_eq!(r.read(0xa4), 1 << 24);
        r.write(0xac, 1 << 24);
        r.write(ram(32), 0);
        r.tick(255);
        assert_eq!(r.read(0xa4), 0);
        r.tick(1);
        assert_eq!(r.read(0xa4), 1 | (1 << 24));
        assert_eq!(r.next_deadline(), None);
    }

    #[test]
    fn continuous_output_has_an_idle_tick_and_stops_after_eof() {
        let mut r = model();
        configure(&mut r, 0, 1, 1, true);
        r.write(conf1(0), APB | OWNER | IDLE | (1 << 6));
        r.write(0xd0, 0xffff_fe00);
        assert_eq!(r.read(0xd0), 0, "classic has no finite loop-count fields");
        r.write(ram(0), item(2, 3));
        r.write(ram(1), 0);
        start(&mut r, 0);
        r.tick(5);
        assert_eq!(r.read(0xa0), 0);
        assert_eq!(r.next_deadline(), Some(1), "one idle tick between laps");
        r.tick(1);
        assert_eq!(r.next_deadline(), Some(2));
        r.tick(4);
        assert_eq!(r.next_deadline(), Some(1));
        r.write(conf1(0), APB | OWNER | IDLE);
        for i in 0..64 {
            r.write(ram(i), 0);
        }
        assert_eq!(r.read(0xa0), 0);
        r.tick(1);
        assert_eq!(r.read(0xa0), 1);
        assert_eq!(r.next_deadline(), None);
    }

    #[test]
    fn fifo_and_direct_access_share_ram_and_apb_reset_rewinds_cursors() {
        let mut r = model();
        configure(&mut r, 1, 1, 1, true);
        r.write(0xf0, 0);
        r.write(4, 0x1234_5678);
        r.write(4, 0x89ab_cdef);
        assert_eq!(r.read(4), 0x1234_5678);
        assert_eq!(r.read(4), 0x89ab_cdef);
        r.write(conf1(1), APB | OWNER | IDLE | (1 << 4));
        assert_eq!(r.read(4), 0x1234_5678);
        r.write(4, 0xaabb_ccdd);
        r.write(0xf0, 1);
        assert_eq!(r.read(ram(64)), 0xaabb_ccdd);
        assert_eq!(r.read(ram(65)), 0x89ab_cdef);
        r.write(4, 0);
        assert_eq!(r.read(ram(65)), 0x89ab_cdef, "FIFO access is masked");
        r.write(0xf0, 0);
        r.write(conf1(1), APB | OWNER | IDLE | (1 << 4));
        assert_eq!(r.read(4), 0xaabb_ccdd);
    }

    #[test]
    fn rx_without_memory_ownership_raises_its_error_interrupt() {
        let mut r = model();
        configure(&mut r, 3, 1, 1, true);
        r.write(conf1(3), APB | 2);
        assert_eq!(r.read(0x60 + 4 * 3) & (1 << 27), 1 << 27);
        assert_eq!(r.read(0xa0), 1 << 11);
        assert_eq!(r.read(0xa4), 0);
        r.write(0xa8, 1 << 11);
        assert_eq!(r.irq_sources(), 1);
        r.write(0xac, 1 << 11);
        assert_eq!(r.irq_sources(), 0);
    }

    #[test]
    fn no_wrap_exhaustion_and_a_shrunken_allocation_raise_memory_empty() {
        for shrink in [false, true] {
            let mut r = model();
            configure(&mut r, 0, if shrink { 2 } else { 1 }, 1, true);
            for i in 0..65 {
                r.write(ram(i), item(1, 1));
            }
            start(&mut r, 0);
            if shrink {
                r.tick(128);
                r.write(0x20, (1 << 24) | 1);
                r.tick(1);
            } else {
                r.tick(127);
            }
            assert_eq!(r.read(0xa0), 0);
            r.tick(1);
            assert_eq!(r.read(0xa0), 4, "exhaustion is an error, not TX_END");
            assert_ne!(r.read(0x60) & (1 << 29), 0);
            assert_eq!(r.next_deadline(), None);
        }
    }

    #[test]
    fn pointer_reset_discards_the_old_pulse_deadline() {
        let mut r = model();
        configure(&mut r, 0, 1, 1, true);
        r.write(ram(0), item(10, 0));
        start(&mut r, 0);
        r.tick(5);
        r.write(conf1(0), APB | OWNER | IDLE | (1 << 3));
        assert_eq!(r.next_deadline(), Some(10));
        r.tick(9);
        assert_eq!(r.read(0xa0), 0);
        r.tick(1);
        assert_eq!(r.read(0xa0), 1);
    }
}
