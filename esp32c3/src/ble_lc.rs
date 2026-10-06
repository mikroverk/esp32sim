//! Opt-in C3 link controller, derived from the rev3 ROM and ESP-IDF v5.5.5 controller.
//! Fields remain inferred unless explicitly marked checked on C3 rev v0.3.
//! See docs/evidence/ble-c3-advertising/README.md for derivation and limits.
use std::collections::VecDeque;
use emu_core::ClockDomain;
use esp_periph::{Device, RegRam, WriteEffect};

const REQUEST: u32 = 1 << 31;
const TIMER: u32 = 1 << 11;
const HALF_US_CYCLES: u64 = crate::periph::CPU_HZ / 2_000_000;
const PERIOD: u64 = (1 << 28) * 625;

#[derive(Default)]
pub struct BleLc {
    ram: RegRam,
    pub(crate) accessible: bool,
    state: Option<Box<State>>,
}

#[derive(Default)]
struct State {
    cycles: u64,
    reset: Option<u64>,
    latch: Option<u64>,
    alarm: Option<u64>,
    raw: u32,
    fifo: VecDeque<u32>,
    kicks: VecDeque<u8>,
    event: Option<Event>,
    scan_response: Vec<u8>,
    dropped: u64,
    pub_log: bool,
    packets: VecDeque<String>,
}

impl BleLc {
    pub fn enable(&mut self) {
        self.state = Some(Box::default());
        self.accessible = true;
        // Inferred reset configuration; initialized readbacks checked on C3 rev v0.3.
        // +48 is preserved by guest RMW; +7c is never written by the probe guest.
        self.ram.write(0x048, 0x0003_fff7);
        self.ram.write(0x07c, 0xe400_e400);
    }
    pub fn observe(&mut self, log: bool) { if let Some(s) = &mut self.state { s.pub_log = log; } }
    pub fn logging(&self) -> bool { self.state.as_ref().is_some_and(|s| s.pub_log) }
    pub fn take_observation(&mut self) -> Option<String> {
        let s = self.state.as_mut()?;
        if s.dropped != 0 { return Some(format!("[ble-observer] dropped={}", std::mem::take(&mut s.dropped))) }
        s.packets.pop_front()
    }
    pub fn enabled(&self) -> bool { self.state.is_some() }
}

fn half(bytes: &[u8], offset: usize) -> u16 { u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) }

struct Event {
    entry: usize,
    due: u64,
    advertising: Option<Advertising>,
}

struct Advertising {
    channels: u8,
    pdu: Vec<u8>,
    scan_response: Vec<u8>,
}

impl State {
    fn raise(&mut self, source: u32) {
        self.raw |= source;
        // Coalesce repeated pending sources; only TIMER and END exist in this model.
        if !self.fifo.contains(&source) { self.fifo.push_back(source); }
    }
    fn observe(&mut self, line: String) {
        if self.pub_log { eprintln!("{line}"); }
        if self.packets.len() == 1024 { self.packets.pop_front(); self.dropped += 1; }
        self.packets.push_back(line);
    }
}

impl BleLc {
    // Inferred mapping: r_emi_get_mem_addr_by_offset, 0x400069f6 / 0x40006a68.
    // The final eight mappings follow a seven-register gap (0x40006a8c).
    fn mapped(&self, offset: u32, len: usize, sram: &[u8]) -> Option<usize> {
        let mut mapping = None;
        let mut end = 0x10000;
        for i in 0..56 {
            let entry = self.ram.read(0x204 + (if i < 48 { i } else { i + 7 }) * 4);
            if entry == 0 { continue }
            let start = (entry >> 18) << 2;
            if start > offset { end = end.min(start); }
            if start <= offset && mapping.is_none_or(|(old, _)| old < start) {
                mapping = Some((start, entry));
            }
        }
        if offset.checked_add(len.try_into().ok()?)? > end { return None }
        let (start, entry) = mapping?;
        if entry & 0x3ffff == 0 { return None }
        let addr = (0x3fc00000 | ((entry << 2) & 0xffffc)).checked_add(offset - start)?;
        let index = addr.checked_sub(crate::bus::DRAM_LOW)? as usize + crate::bus::DRAM_IN_SRAM;
        sram.get(index..index.checked_add(len)?)?;
        Some(index)
    }

    fn advertising(&self, et: usize, sram: &[u8]) -> Result<Advertising, &'static str> {
        // Inferred CS halfword pointer: r_sch_prog_ble_push_hack, ET+8.
        let cs = self.mapped(half(sram, et + 8) as u32 * 2, 90, sram).ok_or("unmapped control structure")?;
        // Inferred CS fields: r_lld_adv_start 0x400186b8 (TX pointer),
        // 0x4001887e..ca (AdvA), 0x40018942 (channel map bits 7:5).
        // Legacy advertising CS format 4: r_lld_adv_start_set_cs 0x400181e4..e8.
        if half(sram, cs) & 0x1f != 4 { return Err("unsupported control structure format") }
        let channels = ((half(sram, cs + 38) >> 5) & 7) as u8;
        let first = half(sram, cs + 28) as u32;
        let mut next = first;
        let mut advertising = None;
        let mut scan_response = Vec::new();
        // Inferred nine 14-byte TX descriptors per activity: r_lld_adv_start 0x40018684..b8.
        for _ in 0..9 {
            let tx = self.mapped(next, 14, sram).ok_or("unmapped TX descriptor")?;
            // Inferred TX link/header/data: r_lld_adv_start_init_evt_param 0x40017b9e / 0x40017c50 (15-bit link),
            // r_lld_adv_adv_data_set 0x40015858/64 (length includes six-byte AdvA).
            let header = half(sram, tx + 2);
            let len = (header >> 8) as usize;
            if !(6..=37).contains(&len) { return Err("invalid legacy PDU length") }
            let data = self.mapped(half(sram, tx + 4) as u32, len - 6, sram).ok_or("unmapped AD buffer")?;
            let mut pdu = header.to_le_bytes().to_vec();
            pdu.extend_from_slice(&sram[cs + 6..cs + 12]);
            pdu.extend_from_slice(&sram[data..data + len - 6]);
            match header & 15 {
                0 | 2 | 6 if advertising.is_none() => advertising = Some(pdu),
                4 if scan_response.is_empty() => scan_response = pdu,
                _ => return Err("unsupported advertising descriptor chain"),
            }
            next = (half(sram, tx) & 0x7fff) as u32;
            if next == 0 || next == first {
                return Ok(Advertising { channels, pdu: advertising.ok_or("missing advertising PDU")?, scan_response });
            }
        }
        Err("unterminated TX descriptor chain")
    }

    pub(crate) fn service(&mut self, sram: &mut [u8]) {
        let Some(s) = &mut self.state else { return };
        let now = s.cycles;
        if s.event.is_none() {
            if let Some(index) = s.kicks.pop_front() {
                let Some(entry) = self.mapped(index as u32 * 16, 16, sram) else {
                    self.state.as_mut().unwrap().observe("[ble-error] unmapped event entry".into());
                    return;
                };
                // Inferred coarse/fine timestamp: r_sch_prog_push 0x40030f1a / 0x40030f3c / 0x40030f9e.
                let coarse = half(sram, entry + 2) as u64 | ((half(sram, entry + 4) as u64 & 0xfff) << 16);
                let target = coarse * 625 + 624u64.saturating_sub(half(sram, entry + 6) as u64);
                let delta = (target + PERIOD - now / HALF_US_CYCLES % PERIOD) % PERIOD;
                let due = if delta >= PERIOD / 2 { now } else { (now / HALF_US_CYCLES + delta) * HALF_US_CYCLES };
                self.state.as_mut().unwrap().event = Some(Event { entry, due, advertising: None });
            }
        }
        let Some(mut event) = self.state.as_mut().unwrap().event.take() else { return };
        if event.due > now {
            self.state.as_mut().unwrap().event = Some(event);
            return;
        }
        if event.advertising.is_none() {
            match self.advertising(event.entry, sram) {
                Ok(advertising) => {
                    let s = self.state.as_mut().unwrap();
                    if advertising.scan_response != s.scan_response {
                        s.scan_response.clone_from(&advertising.scan_response);
                        if !s.scan_response.is_empty() {
                            s.observe(format!("[ble-config] SCAN_RSP {} data={}",
                                ad_fields(&s.scan_response[8..]), hex(&s.scan_response[8..])));
                        }
                    }
                    // Inferred START bit4; unmasked advertising snapshot checked on C3 rev v0.3.
                    s.raw |= 1 << 4;
                    event.advertising = Some(advertising);
                }
                Err(reason) => {
                    self.state.as_mut().unwrap().observe(format!("[ble-error] {reason}"));
                    self.complete(event.entry, 4, sram);
                    return;
                }
            }
        }
        let s = self.state.as_mut().unwrap();
        let advertising = event.advertising.as_mut().unwrap();
        if advertising.channels != 0 {
            let channel = advertising.channels.trailing_zeros() as u8;
            advertising.channels &= !(1 << channel);
            let pdu = &advertising.pdu;
            let address = pdu[2..8].iter().rev().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":");
            let kind = match pdu[0] & 15 { 0 => "ADV_IND", 2 => "ADV_NONCONN_IND", _ => "ADV_SCAN_IND" };
            s.observe(format!("[ble-air] hus={} channel={} type={} AdvA={} {} pdu={}", event.due / HALF_US_CYCLES,
                37 + channel, kind, address, ad_fields(&pdu[8..]), hex(pdu)));
            // Model choice: 1M PHY airtime (preamble, access address, CRC) plus a
            // 300 us silent receive window. No RX or SCAN_RSP is emitted without RX.
            event.due += (8 * (pdu.len() as u64 + 8) + 300) * 2 * HALF_US_CYCLES;
            s.event = Some(event);
        } else {
            self.complete(event.entry, 3, sram);
        }
    }

    fn complete(&mut self, entry: usize, status: u16, sram: &mut [u8]) {
        // Inferred successful ET state 3 / aborted state 4:
        // r_sch_prog_end_isr_handler supplies callback a2=(state==4).
        let value = (half(sram, entry) & !0x38) | (status << 3);
        sram[entry..entry + 2].copy_from_slice(&value.to_le_bytes());
        // Inferred END interrupt bit 5: r_rwble_isr_hack;
        // r_ip_funcs_p+0x6c0 resolves to r_sch_prog_end_isr_hack.
        self.state.as_mut().unwrap().raise(1 << 5);
    }
}

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

fn ad_fields(data: &[u8]) -> String {
    let mut fields = esp_soc::ble::peer::advertising_fields(data);
    for (kind, value) in esp_soc::ble::peer::ad_structures(data) {
        match (kind, value) {
            (1, [flags]) => fields.push(format!("flags=0x{flags:02x}")),
            (0x12, [lo, hi, lo2, hi2]) => fields.push(format!("connection_interval_units={}:{}",
                u16::from_le_bytes([*lo, *hi]), u16::from_le_bytes([*lo2, *hi2]))),
            (2 | 3 | 6 | 7 | 8 | 9, _) => {},
            _ => fields.push(format!("ad_{kind:02x}={}", hex(value))),
        }
    }
    fields.join(" ")
}

impl Device for BleLc {
    fn read(&mut self, off: u32) -> u32 {
        let Some(s) = &self.state else { return self.ram.read(off) };
        if !self.accessible { return 0 }
        match off {
            // Inferred identity required by app r_lld_core_init.
            // Identity and feature word checked on C3 rev v0.3; subfields inferred.
            0x004 => 0x0900_1b00,
            0x008 => 0x0f22_d0b0,
            // Inferred unmasked status; snapshot bits checked on C3 rev v0.3.
            0x014 => s.raw,
            // RF status checked on C3 rev v0.3 after the guest writes zero.
            // Bit meaning and other RF states remain inferred.
            0x070 => self.ram.read(off) | 2,
            // ROM r_cali_phase_match_p searches phase fields [10:8]/[6:4]
            // and tests bit12. Successful 2/2 readback checked on C3 rev v0.3;
            // the phase acceptance rule and instantaneous completion are inferred.
            0x0f8 => {
                let v = self.ram.read(off) & !(1 << 12);
                v | (u32::from(v & 0x771 == 0x221) << 12)
            },
            // Inferred masked status, read by ROM r_rwble_isr (0x4002e8ee).
            0x010 => s.raw & self.ram.read(0x00c),
            // Inferred W1C readback for read/modify/write acknowledgements in r_rwble_isr.
            0x018 => 0,
            // Inferred FIFO: r_rwble_isr_hack extracts count [9:5]
            // and status [30:10], then writes bit 0 to pop. r_lld_core_init resets bit 31.
            0x2d8 => (self.ram.read(off) & 0x1e) | ((s.fifo.len() as u32) << 5) | (s.fifo.front().copied().unwrap_or(0) << 10),
            _ => self.ram.read(off),
        }
    }

    fn write(&mut self, off: u32, v: u32) -> WriteEffect {
        let Some(s) = &mut self.state else {
            self.ram.write(off, v);
            return WriteEffect::NONE;
        };
        match off {
            0x004 | 0x008 | 0x010 | 0x014 | 0x020 => {},
            // Checked on C3 rev v0.3: guest writes 00640064, reads 00000064.
            // Width beyond the exercised low byte remains inferred.
            0x08c => self.ram.write(off, v & 0xffff),
            // Inferred index/request: r_sch_prog_ble_push_hack.
            0x100 => {
                self.ram.write(off, v);
                if v & REQUEST != 0 && s.kicks.len() < 16 { s.kicks.push_back((v & 15) as u8); }
            }
            // Inferred reset/disable completion: r_rwip_driver_init / r_rwble_hw_disable.
            // One half-microsecond latency is a model choice, not a measured hardware delay.
            0x000 => {
                self.ram.write(off, v);
                if v & REQUEST != 0 { s.reset = Some(s.cycles + HALF_US_CYCLES); }
            }
            // Latch completion and 2 half-us ticks/us checked on C3 rev v0.3.
            // Atomicity and 80-cycle completion remain inferred.
            // Inferred atomic snapshot: r_rwip_time_get polls bit 31, masks 28 bits,
            // and returns 624 minus the fine register. Reads never advance time.
            0x01c => {
                if v & REQUEST != 0 {
                    self.ram.write(off, self.ram.read(off) | REQUEST);
                    s.latch = Some(s.cycles + HALF_US_CYCLES);
                }
            }
            // Inferred W1C: r_rwip_timer_hus_set and r_rwble_isr.
            0x018 => s.raw &= !v,
            0x2d8 => {
                self.ram.write(off, v & 0x1e);
                if v & REQUEST != 0 { s.fifo.clear(); } else if v & 1 != 0 { s.fifo.pop_front(); }
            }
            // Inferred alarm pair: r_rwip_timer_hus_set writes coarse then 624-hus.
            // A target behind now by less than half the counter period is due immediately.
            0x0f0 => {
                self.ram.write(off, v & 0x3ff);
                let target = (self.ram.read(0x0ec) as u64 & 0x0fff_ffff) * 625
                    + 624u64.saturating_sub((v & 0x3ff) as u64);
                let now = s.cycles / HALF_US_CYCLES;
                let delta = (target + PERIOD - now % PERIOD) % PERIOD;
                s.alarm = Some(if delta >= PERIOD / 2 { s.cycles } else { (now + delta) * HALF_US_CYCLES });
            }
            _ => self.ram.write(off, v),
        }
        WriteEffect::NONE
    }

    fn clock(&self) -> Option<ClockDomain> { self.state.as_ref().map(|_| ClockDomain::Cpu) }
    fn has_deadline(&self) -> bool { self.enabled() }
    fn next_deadline(&self) -> Option<u64> {
        let s = self.state.as_ref()?;
        [s.reset, s.latch, s.alarm, s.event.as_ref().map(|e| e.due)].into_iter().flatten().min().map(|t| t.saturating_sub(s.cycles).max(1))
    }
    fn irq_sources(&self) -> u64 {
        self.state.as_ref().is_some_and(|s| s.raw & self.ram.read(0x00c) != 0) as u64
    }
    fn tick(&mut self, ticks: u64) {
        let Some(s) = &mut self.state else { return };
        // Inferred half-slot status bit0; init/adv snapshots checked on C3 rev v0.3.
        if self.ram.read(0) & 0x100 != 0 && s.cycles / (625 * HALF_US_CYCLES) != (s.cycles + ticks) / (625 * HALF_US_CYCLES) { s.raw |= 1; }
        s.cycles += ticks;
        if s.reset.is_some_and(|t| t <= s.cycles) {
            s.reset = None;
            s.alarm = None;
            s.event = None;
            s.kicks.clear();
            s.raw = 0;
            s.fifo.clear();
            self.ram.write(0, self.ram.read(0) & !REQUEST);
        }
        if s.latch.is_some_and(|t| t <= s.cycles) {
            // Snapshot at completion, even if the scheduler advances beyond the deadline.
            let hus = s.latch.take().unwrap() / HALF_US_CYCLES;
            self.ram.write(0x01c, (hus / 625) as u32 & 0x0fff_ffff);
            self.ram.write(0x020, 624 - (hus % 625) as u32);
        }
        if s.alarm.is_some_and(|t| t <= s.cycles) {
            s.alarm = None;
            s.raise(TIMER);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advertising_fixture() -> (BleLc, Vec<u8>) {
        let mut d = BleLc::default();
        d.enable();
        // One synthetic SRAM mapping; pointers exercise the actual exchange-memory decoder.
        d.write(0x204, 0x20000);
        let mut ram = vec![0; 0x14000];
        let base = crate::bus::DRAM_IN_SRAM;
        for (offset, value) in [(0, 2), (2, 10), (6, 624), (8, 0x200),
            (0x400, 4), (0x41c, 0x1400), (0x426, 0xe0),
            (0x1400, 0x140e), (0x1402, 0x0900), (0x1404, 0x2c00),
            (0x140e, 0x1400), (0x1410, 0x0a04), (0x1412, 0x2400)] {
            ram[base + offset..base + offset + 2].copy_from_slice(&(value as u16).to_le_bytes());
        }
        ram[base + 0x406..base + 0x40c].copy_from_slice(&[6, 5, 4, 3, 2, 1]);
        ram[base + 0x2c00..base + 0x2c03].copy_from_slice(&[2, 1, 6]);
        ram[base + 0x2400..base + 0x2404].copy_from_slice(&[3, 9, b'x', b'y']);
        (d, ram)
    }

    #[test]
    fn descriptor_chain_maps_address_data_and_conditional_scan_response() {
        let (mut d, mut ram) = advertising_fixture();
        let et = crate::bus::DRAM_IN_SRAM;
        let a = d.advertising(et, &ram).unwrap();
        assert_eq!(a.pdu, [0, 9, 6, 5, 4, 3, 2, 1, 2, 1, 6]);
        assert_eq!(&a.scan_response[8..], &[3, 9, b'x', b'y']);
        assert_eq!(a.channels, 7);
        ram[et + 0x1403] = 38;
        assert!(d.advertising(et, &ram).is_err());
        ram[et + 0x1403] = 9;
        // An unmapped region must not fall back to the preceding mapping.
        d.write(0x208, 0x2c000000);
        assert!(d.advertising(et, &ram).is_err());
        assert!(d.mapped(0x2bff, 2, &ram).is_none());
        assert!(d.mapped(u32::MAX, usize::MAX, &ram).is_none());
    }

    #[test]
    fn channels_run_in_time_then_completion_is_masked_and_acknowledged() {
        let (mut d, mut ram) = advertising_fixture();
        d.write(0x100, REQUEST);
        d.service(&mut ram);
        assert!(d.take_observation().is_none());
        d.tick(10 * 625 * HALF_US_CYCLES - 1);
        d.service(&mut ram);
        assert!(d.take_observation().is_none());
        d.tick(1);
        d.service(&mut ram);
        assert!(d.take_observation().unwrap().starts_with("[ble-config] SCAN_RSP name=\"xy\""));
        let first = d.take_observation().unwrap();
        assert!(first.contains("hus=6250 channel=37 type=ADV_IND AdvA=01:02:03:04:05:06"));
        for channel in [38, 39] {
            let delta = d.next_deadline().unwrap();
            d.tick(delta - 1);
            d.service(&mut ram);
            assert!(d.take_observation().is_none());
            d.tick(1);
            d.service(&mut ram);
            assert!(d.take_observation().unwrap().contains(&format!("channel={channel} type=ADV_IND")));
        }
        assert_eq!(d.irq_sources(), 0);
        d.tick(d.next_deadline().unwrap());
        d.service(&mut ram);
        assert_eq!((half(&ram, crate::bus::DRAM_IN_SRAM) >> 3) & 7, 3);
        assert_eq!(d.read(0x10), 0);
        d.write(0xc, 1 << 5);
        assert_eq!(d.read(0x10), 1 << 5);
        assert_eq!(d.irq_sources(), 1);
        assert_eq!(d.read(0x2d8), (1 << 5) | (1 << 15));
        d.write(0x18, 1 << 5);
        d.write(0x2d8, 1);
        assert_eq!(d.irq_sources(), 0);
        assert_eq!(d.read(0x2d8), 0);
        assert!(d.take_observation().is_none());
        assert_eq!(d.next_deadline(), None);
    }

    #[test]
    fn channel_map_reset_and_bad_descriptor_do_not_transmit_extra_packets() {
        for map in 0..8 {
            let (mut d, mut ram) = advertising_fixture();
            ram[crate::bus::DRAM_IN_SRAM + 0x426] = map << 5;
            d.write(0x100, REQUEST);
            d.service(&mut ram);
            for _ in 0..4 {
                if let Some(delta) = d.next_deadline() { d.tick(delta); d.service(&mut ram); }
            }
            let mut emitted = Vec::new();
            while let Some(line) = d.take_observation() {
                if line.starts_with("[ble-air]") { emitted.push(line); }
            }
            let channels: Vec<_> = (0..3).filter(|ch| map & (1 << ch) != 0).map(|ch| ch + 37).collect();
            assert_eq!(emitted.len(), channels.len());
            for (line, channel) in emitted.iter().zip(channels) { assert!(line.contains(&format!("channel={channel}"))); }
        }
        let (mut d, mut ram) = advertising_fixture();
        d.write(0x100, REQUEST);
        d.service(&mut ram);
        d.write(0, REQUEST);
        d.tick(1_000_000);
        d.service(&mut ram);
        assert!(d.take_observation().is_none());
        ram[crate::bus::DRAM_IN_SRAM + 0x1403] = 255;
        d.write(0x100, REQUEST);
        d.service(&mut ram);
        assert!(d.take_observation().unwrap().starts_with("[ble-error]"));
        assert_eq!((half(&ram, crate::bus::DRAM_IN_SRAM) >> 3) & 7, 4);
    }


    #[test]
    fn latch_is_atomic_and_wraps_in_modeled_time() {
        let mut d = BleLc::default();
        d.enable();
        for hus in [0, 623, 624, 625, PERIOD - 1, PERIOD, PERIOD + 625] {
            d.state.as_mut().unwrap().cycles = hus * HALF_US_CYCLES;
            d.write(0x01c, REQUEST);
            for _ in 0..10 { assert_ne!(d.read(0x01c) & REQUEST, 0); }
            d.tick(HALF_US_CYCLES - 1);
            assert_ne!(d.read(0x01c) & REQUEST, 0);
            d.tick(1);
            let snapshot = (d.read(0x01c), d.read(0x020));
            assert_eq!(snapshot, (((hus + 1) / 625) as u32 & 0x0fff_ffff, 624 - ((hus + 1) % 625) as u32));
            d.tick(1000);
            assert_eq!(snapshot, (d.read(0x01c), d.read(0x020)));
        }
    }

    #[test]
    fn reset_completes_without_polling_and_cancels_alarm() {
        let mut d = BleLc::default();
        d.enable();
        for control in [0x100, 0] {
            d.write(0x0ec, 10);
            d.write(0x0f0, 624);
            d.write(0, REQUEST | control);
            d.tick(HALF_US_CYCLES - 1);
            assert_eq!(d.read(0), REQUEST | control);
            d.tick(1);
            assert_eq!(d.read(0), control);
            assert_eq!(d.read(4), 0x0900_1b00);
            d.tick(1_000_000);
            assert_eq!(d.state.as_ref().unwrap().raw, u32::from(control & 0x100 != 0));
        }
    }

    #[test]
    fn alarm_mask_ack_rearm_and_wrap() {
        let mut d = BleLc::default();
        d.enable();
        for now in [0, PERIOD - 2, PERIOD + 10] {
            d.state.as_mut().unwrap().cycles = now * HALF_US_CYCLES;
            let target = (now + 3) % PERIOD;
            d.write(0x00c, 0);
            d.write(0x0ec, (target / 625) as u32);
            d.write(0x0f0, 624 - (target % 625) as u32);
            assert_eq!(d.next_deadline(), Some(3 * HALF_US_CYCLES));
            d.tick(3 * HALF_US_CYCLES - 1);
            assert_eq!(d.state.as_ref().unwrap().raw, 0);
            d.tick(1);
            assert_eq!(d.state.as_ref().unwrap().raw, TIMER);
            assert_eq!(d.irq_sources(), 0);
            d.write(0x00c, TIMER);
            assert_eq!(d.read(0x010), TIMER);
            assert_eq!(d.irq_sources(), 1);
            assert_eq!(d.read(0x2d8), (TIMER << 10) | (1 << 5));
            d.write(0x2d8, 1);
            assert_eq!(d.read(0x2d8), 0);
            d.write(0x018, 1);
            assert_eq!(d.irq_sources(), 1);
            d.write(0x018, TIMER);
            assert_eq!(d.irq_sources(), 0);
            d.tick(10_000);
            assert_eq!(d.read(0x010), 0);
        }
    }

    #[test]
    fn silicon_readbacks_follow_configuration_and_power_state() {
        let mut p = crate::periph::Peripherals::new([0; 6]);
        p.ble_lc.enable();
        assert_eq!(p.read32(0x60026014), 0xfffc_e030);
        assert_eq!(p.read32(0x600c0024), 0x0200_1001);
        assert_eq!(p.read32(0x60031004), 0);
        p.write32(0x60026014, 0xffff_ffdf);
        assert_eq!(p.read32(0x60031004), 0x0900_1b00);
        for (addr, mask) in [(0x60026014, 1 << 16), (0x60026018, 1 << 11),
            (0x60008088, 1 << 11), (0x6000808c, 1 << 22)] {
            let before = p.read32(addr);
            p.write32(addr, before ^ mask);
            assert_eq!(p.read32(0x60031004), 0);
            p.write32(addr, before);
            assert_eq!(p.read32(0x60031004), 0x0900_1b00);
        }
        assert_eq!(p.read32(0x60031008), 0x0f22_d0b0);
        assert_eq!(p.read32(0x6003107c), 0xe400_e400);
        let config = p.read32(0x60031048);
        p.write32(0x60031048, config | 0x1f0);
        assert_eq!(p.read32(0x60031048), 0x0003_fff7);
        p.write32(0x60031070, 0);
        assert_eq!(p.read32(0x60031070), 2);
        p.write32(0x6003108c, 0x0064_0064);
        assert_eq!(p.read32(0x6003108c), 100);
        for (value, expected) in [(0x331, 0x331), (0x221, 0x1221), (0x1220, 0x220)] {
            p.write32(0x600310f8, value);
            assert_eq!(p.read32(0x600310f8), expected);
        }
    }

    #[test]
    fn optional_device_routes_source_eight_only_when_enabled() {
        let mut p = crate::periph::Peripherals::new([0; 6]);
        assert_eq!(p.ble_lc.clock(), None);
        assert!(!p.ble_lc.has_deadline());
        p.write32(0x6003101c, REQUEST);
        p.tick(1000);
        assert_eq!(p.read32(0x6003101c), REQUEST);
        assert!(p.misc.active_optional.is_empty());
        p.ble_lc.enable();
        p.write32(0x600c2020, 9);
        p.write32(0x6003100c, TIMER);
        p.write32(0x600310ec, 1);
        p.write32(0x600310f0, 624);
        p.tick(625 * HALF_US_CYCLES);
        p.refresh_lines();
        assert_eq!(p.source_status()[0] & (1 << 8), 1 << 8);
        assert_ne!(p.intc.lines.level & (1 << 9), 0);
        p.write32(0x60031018, TIMER);
        p.refresh_lines();
        assert_eq!(p.intc.lines.level & (1 << 9), 0);
    }
}
