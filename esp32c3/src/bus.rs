//! ESP32-C3 memory map.
//!
//! Simpler than the S3's: one core, no PSRAM, an 8 MB cache window per bus and a flat 128-entry
//! MMU. SRAM1 is dual-mapped (IRAM `0x4038_0000` and DRAM `0x3FC8_0000` are the same bytes);
//! SRAM0 below it is the instruction cache's, reachable only from the instruction bus.

use crate::periph::{Peripherals, PERIPH_BASE, PERIPH_END};
use riscv_rv32::bus::{Bus, Fault};

pub const SRAM_SIZE: usize = 400 * 1024;
pub const IRAM_LOW: u32 = 0x4037_C000;
pub const IRAM_HIGH: u32 = 0x403E_0000;
pub const DRAM_LOW: u32 = 0x3FC8_0000;
pub const DRAM_HIGH: u32 = 0x3FCE_0000;
/// SRAM1 starts 16 KiB into the buffer: SRAM0 in front of it is instruction-bus only.
pub const DRAM_IN_SRAM: usize = 0x4000;
pub const IROM_MASK_LOW: u32 = 0x4000_0000;
pub const IROM_MASK_HIGH: u32 = 0x4006_0000;
pub const DROM_MASK_LOW: u32 = 0x3FF0_0000;
pub const DROM_MASK_HIGH: u32 = 0x3FF2_0000;
pub const RTC_SLOW_LOW: u32 = 0x5000_0000;
pub const RTC_SLOW_HIGH: u32 = 0x5000_2000;
pub const DBUS_LOW: u32 = 0x3C00_0000;
pub const DBUS_HIGH: u32 = 0x3C80_0000;
pub const IBUS_LOW: u32 = 0x4200_0000;
pub const IBUS_HIGH: u32 = 0x4280_0000;
pub const MMU_TABLE: u32 = 0x600C_5000;
pub const MMU_ENTRIES: usize = 128;
/// bit 8 marks an entry invalid; bits 7:0 are the 64 KiB flash page
pub const MMU_INVALID: u32 = 1 << 8;
pub const PAGE: u32 = 0x1_0000;

pub struct SocBus {
    pub sram: Vec<u8>,
    pub irom: Vec<u8>,
    pub drom: Vec<u8>,
    pub rtc_slow: Vec<u8>,
    pub flash: Vec<u8>,
    pub mmu: [u32; MMU_ENTRIES],
    pub periph: Peripherals,
    /// a bare module: nothing on the pins
    pub board: esp_soc::Board,
    pub cycles: u64,
    pub last_fault: Option<(u32, bool)>,
    /// a peripheral write may have moved an interrupt line: re-derive before the next instruction
    pub irq_dirty: bool,
    /// GPIO edges for observers, while one wants them: (cycle, pin, level)
    pub gpio_events: Option<Vec<(u64, u8, bool)>>,
    pub debug: esp_soc::DebugFlags,
}

impl SocBus {
    pub fn new(flash_size: usize, mac: [u8; 6]) -> Self {
        SocBus {
            sram: vec![0; SRAM_SIZE],
            irom: vec![0; (IROM_MASK_HIGH - IROM_MASK_LOW) as usize],
            drom: vec![0; (DROM_MASK_HIGH - DROM_MASK_LOW) as usize],
            rtc_slow: vec![0; (RTC_SLOW_HIGH - RTC_SLOW_LOW) as usize],
            flash: vec![0xff; flash_size],
            mmu: [MMU_INVALID; MMU_ENTRIES],
            periph: Peripherals::new(mac), board: Box::new(esp_soc::NoBoard),
            cycles: 0, last_fault: None, irq_dirty: true, gpio_events: None, debug: Default::default(),
        }
    }

    /// Resolve to (buffer, offset, writable). Cache windows go through the MMU.
    fn resolve(&mut self, addr: u32) -> Option<(&mut Vec<u8>, usize, bool)> {
        match addr {
            DRAM_LOW..=0x3FCD_FFFF => Some((&mut self.sram, (addr - DRAM_LOW) as usize + DRAM_IN_SRAM, true)),
            IRAM_LOW..=0x403D_FFFF => Some((&mut self.sram, (addr - IRAM_LOW) as usize, true)),
            IROM_MASK_LOW..=0x4005_FFFF => Some((&mut self.irom, (addr - IROM_MASK_LOW) as usize, false)),
            DROM_MASK_LOW..=0x3FF1_FFFF => Some((&mut self.drom, (addr - DROM_MASK_LOW) as usize, false)),
            RTC_SLOW_LOW..=0x5000_1FFF => Some((&mut self.rtc_slow, (addr - RTC_SLOW_LOW) as usize, true)),
            DBUS_LOW..=0x3C7F_FFFF | IBUS_LOW..=0x427F_FFFF => {
                // both buses index one flat table; software keeps their page ranges disjoint
                let entry = self.mmu[((addr & 0x7F_FFFF) >> 16) as usize];
                if entry & MMU_INVALID != 0 { return None; }
                let off = (entry & 0xff) as usize * PAGE as usize + (addr & 0xffff) as usize;
                if off < self.flash.len() { Some((&mut self.flash, off, false)) } else { None }
            }
            _ => None,
        }
    }

    #[inline]
    fn is_periph(addr: u32) -> bool { (PERIPH_BASE..PERIPH_END).contains(&addr) }

    fn periph_read(&mut self, addr: u32, size: u32) -> u32 {
        if (MMU_TABLE..MMU_TABLE + (MMU_ENTRIES as u32) * 4).contains(&addr) {
            return self.mmu[((addr - MMU_TABLE) >> 2) as usize];
        }
        let w = self.periph.read32(addr & !3);
        match size { 1 => (w >> ((addr & 3) * 8)) & 0xff, 2 => (w >> ((addr & 2) * 8)) & 0xffff, _ => w }
    }

    fn periph_write(&mut self, addr: u32, v: u32, size: u32) {
        if (MMU_TABLE..MMU_TABLE + (MMU_ENTRIES as u32) * 4).contains(&addr) {
            self.mmu[((addr - MMU_TABLE) >> 2) as usize] = v & 0x1ff;
            return;
        }
        let a = addr & !3;
        let v = match size {
            4 => v,
            1 => { let old = self.periph.read32(a); let sh = (addr & 3) * 8; (old & !(0xff << sh)) | ((v & 0xff) << sh) }
            _ => { let old = self.periph.read32(a); let sh = (addr & 2) * 8; (old & !(0xffff << sh)) | ((v & 0xffff) << sh) }
        };
        self.periph.write32(a, v);
        // A SPI flash command must complete before the guest can read its result: firmware kicks
        // the command and polls/reads the data registers a few instructions later, well inside one
        // scheduling quantum. Running it at the quantum boundary instead loses the race and the
        // read returns zeros — which is exactly how `E memspi: no response` showed up on a
        // non-power-on boot while a power-on boot happened to survive it.
        if self.periph.spi_exec { self.run_spi(); }
        self.irq_dirty = true;
    }

    /// Execute a pending SPI1 command against the flash image.
    fn run_spi(&mut self) {
        self.periph.spi_exec = false;
        let mut no_psram = Vec::new();
        self.periph.spi1.execute(&mut self.flash, &mut no_psram);
        self.periph.spi1.dirty.clear();
    }

    /// Write straight into flash (image loaders, not the guest).
    pub fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> {
        let target = self.flash.get_mut(offset..).and_then(|tail| tail.get_mut(..data.len()))
            .ok_or("flash image too large")?;
        target.copy_from_slice(data);
        Ok(())
    }

    fn sram_bytes(&self, addr: u32, len: usize) -> Option<&[u8]> {
        let offset = addr.checked_sub(DRAM_LOW)? as usize + DRAM_IN_SRAM;
        self.sram.get(offset..offset.checked_add(len)?)
    }
    fn sram32(&self, addr: u32) -> u32 {
        self.sram_bytes(addr, 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn sram_store(&mut self, addr: u32, data: &[u8]) -> bool {
        let Some(offset) = addr.checked_sub(DRAM_LOW).map(|a| a as usize + DRAM_IN_SRAM) else { return false; };
        let Some(end) = offset.checked_add(data.len()) else { return false; };
        let Some(dst) = self.sram.get_mut(offset..end) else { return false; };
        dst.copy_from_slice(data);
        true
    }

    fn aes_dma_step(&mut self) {
        let (out_ch, in_ch) = { let g = &self.periph.gdma; (g.out_channel_for(6), g.in_channel_for(6)) };
        if let (Some(out_ch), Some(in_ch)) = (out_ch, in_ch) {
            self.periph.aes.dma_pending = false;
            let mut input = Vec::new();
            let (mut desc, mut last) = (self.periph.gdma.out[out_ch].desc, 0);
            for _ in 0..4096 {
                if desc == 0 { break; }
                let d = esp_periph::read_desc(&|a| self.sram32(a), desc);
                let Some(bytes) = self.sram_bytes(d.buf, d.length as usize) else { break };
                input.extend_from_slice(bytes);
                last = desc;
                if d.eof { break; }
                desc = d.next;
            }
            let c = &mut self.periph.gdma.out[out_ch];
            c.running = false; c.desc = 0; c.eof_desc = last;
            c.int_raw |= (1 << 0) | (1 << 1) | (1 << 3);                 // OUT_DONE, OUT_EOF, OUT_TOTAL_EOF

            let output = self.periph.aes.transform_blocks(&input);
            let (mut desc, mut pos, mut last) = (self.periph.gdma.inp[in_ch].desc, 0usize, 0);
            for _ in 0..4096 {
                if desc == 0 || pos >= output.len() { break; }
                let d = esp_periph::read_desc(&|a| self.sram32(a), desc);
                let n = (d.size as usize).min(output.len() - pos);
                if n == 0 || !self.sram_store(d.buf, &output[pos..pos + n]) { break; }
                pos += n;
                let eof = pos == output.len();
                let dw0 = (self.sram32(desc) & !(0xfff << 12) & !(3 << 30)) | (n as u32) << 12 | if eof { 1 << 30 } else { 0 };
                self.sram_store(desc, &dw0.to_le_bytes());
                last = desc;
                desc = d.next;
            }
            let c = &mut self.periph.gdma.inp[in_ch];
            c.running = false; c.desc = 0; c.eof_desc = last;
            c.int_raw |= (1 << 0) | (1 << 1);                            // IN_DONE, IN_SUC_EOF
        }
        if self.periph.aes.dma_pending { return; }
        self.periph.aes.state = 2;                                       // DONE
        self.periph.aes.int_raw |= 1;
        self.irq_dirty = true;
    }

    /// WiFi MAC transmit: fetch the queued frames from their DMA descriptors and complete them.
    fn wifi_tx_step(&mut self) {
        let pending = std::mem::take(&mut self.periph.wifi.tx_pending);
        for (slot, desc) in pending {
            let dw0 = self.sram32(desc); let pkt = self.sram32(desc.wrapping_add(4));
            let len = ((dw0 >> 12) & 0xfff) as usize;
            let frame = self.sram_bytes(pkt, len).unwrap_or_default().to_vec();
            if self.periph.wifi.log || self.debug.has("wifi-frames") { eprintln!("[wifi] TX slot {} desc {:#010x} pkt {:#010x} {}", slot, desc, pkt, esp_soc::wifi::describe(&frame)); }
            self.periph.wifi.tx_done(slot);
            self.irq_dirty = true;
            let now_us = self.cycles / (crate::periph::CPU_HZ / 1_000_000);
            if let Some(ap) = &mut self.periph.wifi.ap {
                if let Some(data) = ap.on_station_tx(&frame, now_us) {
                    if let Some(eth) = esp_soc::wifi::data_to_eth(&data) {
                        if !self.periph.wifi.relay || (eth.len() <= 1518 && self.periph.wifi.eth_tx.len() < 64) { self.periph.wifi.eth_tx.push(eth); }
                    }
                }
            }
        }
    }

    /// The virtual air: beacons/responses from the AP and frames from the network backend land in the RX ring.
    fn wifi_air_step(&mut self) {
        let now_us = self.cycles / (crate::periph::CPU_HZ / 1_000_000);
        // The blob's RX path only *indicates* a frame up the 802.11 stack while the descriptor ring is
        // shallow; with several filled descriptors pending it switches to batch block-recycle and drops
        // them. So hold off until the previously delivered descriptor has been recycled by software
        // (has_data cleared) — that is what a real radio sees at low traffic — and never deliver two
        // frames closer than a frame's airtime.
        if now_us.wrapping_sub(self.periph.wifi.last_rx_us) < 400 { return; }
        // ... but if software stops recycling altogether, don't stall the air forever: after 50 ms
        // the frame is dropped, exactly as a real ring would overflow.
        let busy = { let d = self.periph.wifi.last_rx_desc; d != 0 && self.sram32(d) & (1 << 30) != 0 };
        if busy && now_us.wrapping_sub(self.periph.wifi.last_rx_us) < 50_000 { return; }
        let mut due = { let ap = self.periph.wifi.ap.as_mut().unwrap(); ap.step(now_us) };
        let eth_in = if self.periph.wifi.relay {
            if due.is_empty() && !self.periph.wifi.eth_rx.is_empty() { vec![self.periph.wifi.eth_rx.remove(0)] } else { Vec::new() }
        } else { std::mem::take(&mut self.periph.wifi.eth_rx) };
        for e in eth_in { if let Some(f) = self.periph.wifi.ap.as_mut().unwrap().data_from_ds(&e) { due.push(esp_soc::wifi::AirFrame { at_us: now_us, frame: f }); } }
        if due.is_empty() { return; }
        // management responses (auth, assoc, probe) go before beacons: a connect exchange must not be
        // crowded out by beacon traffic
        due.sort_by_key(|a| (esp_soc::wifi::is_beacon(&a.frame), a.at_us));
        let first = due.remove(0);
        self.wifi_rx_deliver(&first.frame, now_us);
        self.periph.wifi.last_rx_us = now_us;
        if let Some(ap) = &mut self.periph.wifi.ap { for a in due { ap.queue.push(a); } }
    }

    /// Write one received frame into the next RX descriptor (rx_ctrl header + frame + FCS) and raise the RX event.
    fn wifi_rx_deliver(&mut self, frame: &[u8], now_us: u64) {
        if self.periph.wifi.rx_next == 0 { self.periph.wifi.rx_dropped += 1; return; }
        let desc = self.periph.wifi.rx_next | esp_periph::DMA_ADDR_BASE;
        let dw0 = self.sram32(desc); let buf = self.sram32(desc.wrapping_add(4)); let next = self.sram32(desc.wrapping_add(8));
        let size = (dw0 & 0xfff) as usize;
        let total = 48 + frame.len() + 4;
        if dw0 & (3 << 30) != 1 << 31 || buf == 0 || size < total { self.periph.wifi.rx_dropped += 1; return; }
        let (chan, log) = { let ap = self.periph.wifi.ap.as_ref().unwrap(); (ap.cfg.channel as u32, ap.log) };
        let mut b = Vec::with_capacity(total);
        let bcast = frame.len() >= 5 && frame[4] & 1 == 1;
        // filter-match nibble: bit 28 is the "accepted by the address filter" bit the blob's RX path
        // requires (silicon: a broadcast beacon reads 0x111b20ad); unicast frames add bit 29.
        let fm = if bcast { 1u32 << 28 } else { (1u32 << 28) | (1u32 << 29) };
        let w0: u32 = fm | 0xd8u32;   // rssi -40 dBm, 1 Mbps, legacy
        let w2: u32 = (chan << 16) | (chan << 20);                                        // channel, secondary
        let w5: u32 = 0xa6;                                                                // noise floor -90
        let w11: u32 = (frame.len() + 4) as u32 & 0xfff;                    // sig_len (incl. FCS), rx_state OK
        for w in [w0, 0, w2, now_us as u32, 0, w5, 0, 0, 0, 0, 0, w11] { b.extend_from_slice(&w.to_le_bytes()); }
        b.extend_from_slice(frame); b.extend_from_slice(&esp_soc::wifi::fcs(frame).to_le_bytes());
        if !self.sram_store(buf, &b) { self.periph.wifi.rx_dropped += 1; return; }
        let ndw0 = (dw0 & !(0xfff << 12)) | ((total as u32) << 12) | (1 << 30) | (1 << 31);   // length; owner AND has_data set (verified on silicon 2026-08-25: dw0=0xc0..)
        self.sram_store(desc, &ndw0.to_le_bytes());
        let w = &mut self.periph.wifi;
        w.rx_last = (desc & 0xf_ffff) | (1 << 24); w.rx_next = next & 0xf_ffff; w.last_rx_desc = desc; w.rx_frames += 1; w.events |= (1 << 14) | (1 << 24);   // RX data (wDev_ProcessFiq tests 0x1004000)   // registers hold masked descriptor addrs; rx_last has a 0x01 prefix (silicon)
        if log { let d = esp_soc::wifi::describe(frame); if d.contains("auth")||d.contains("assoc") { eprintln!("[wifi] RX AUTH/ASSOC -> desc {:#010x} buf {:#010x} {}", desc, buf, d); } else { eprintln!("[wifi] RX -> desc {:#010x} {}", desc, d); } }
        self.irq_dirty = true;
    }

    /// The network behind the access point: what the station sent is answered at once, the host
    /// sockets are read every 500 us (syscalls every round would cost more than the CPU).
    fn wifi_net_step(&mut self) {
        let now_us = self.cycles / (crate::periph::CPU_HZ / 1_000_000);
        let mac = &mut self.periph.wifi;
        if mac.relay { return; }
        let Some(net) = mac.net.as_mut() else { return };
        let out = std::mem::take(&mut mac.eth_tx);
        let due = now_us.wrapping_sub(mac.net_polled_us) >= 500;
        if out.is_empty() && !due { return; }
        if due { mac.net_polled_us = now_us; }
        for e in out { let r = net.handle(&e, now_us); mac.eth_rx.extend(r); }
        let r = net.poll(now_us); mac.eth_rx.extend(r);
    }

    pub fn load_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        for (i, b) in data.iter().enumerate() {
            let a = addr.wrapping_add(i as u32);
            match self.resolve(a) {
                Some((buf, off, _)) if off < buf.len() => buf[off] = *b,
                _ => return Err(format!("load: address {:#010x} not mapped", a)),
            }
        }
        Ok(())
    }

    /// Run the SPI1 controller if the guest just kicked it, then advance device time.
    fn devices(&mut self, cycles: u32) {
        if self.periph.spi_exec { self.run_spi(); }
        self.periph.tick(cycles as u64);
        if self.periph.aes.dma_pending { self.aes_dma_step(); }
        if !self.periph.wifi.tx_pending.is_empty() { self.wifi_tx_step(); }
        if self.periph.wifi.ap.is_some() { self.wifi_air_step(); self.wifi_net_step(); }
    }
}

macro_rules! rd {
    ($self:ident, $addr:expr, $n:expr, $conv:expr) => {{
        let addr = $addr;
        match $self.resolve(addr) {
            Some((b, o, _)) if b.len().saturating_sub(o) >= $n => Ok($conv(&b[o..o + $n])),
            _ => { $self.last_fault = Some((addr, false)); Err(Fault::Unmapped) }
        }
    }};
}

impl Bus for SocBus {
    fn note_code_page(&mut self, _vidx: u32) {} // All writes already update versions, or this bus has no decode cache.
    fn read8(&mut self, addr: u32) -> Result<u8, Fault> {
        if Self::is_periph(addr) { return Ok(self.periph_read(addr, 1) as u8); }
        rd!(self, addr, 1, |b: &[u8]| b[0])
    }
    fn read16(&mut self, addr: u32) -> Result<u16, Fault> {
        if Self::is_periph(addr) { return Ok(self.periph_read(addr, 2) as u16); }
        rd!(self, addr, 2, |b: &[u8]| u16::from_le_bytes(b.try_into().unwrap()))
    }
    fn read32(&mut self, addr: u32) -> Result<u32, Fault> {
        if Self::is_periph(addr) { return Ok(self.periph_read(addr, 4)); }
        rd!(self, addr, 4, |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap()))
    }
    fn write8(&mut self, addr: u32, v: u8) -> Result<(), Fault> {
        if Self::is_periph(addr) { self.periph_write(addr, v as u32, 1); return Ok(()); }
        match self.resolve(addr) {
            Some((b, o, true)) if o < b.len() => { b[o] = v; Ok(()) }
            _ => { self.last_fault = Some((addr, true)); Err(Fault::Prohibited) }
        }
    }
    fn write16(&mut self, addr: u32, v: u16) -> Result<(), Fault> {
        if Self::is_periph(addr) { self.periph_write(addr, v as u32, 2); return Ok(()); }
        match self.resolve(addr) {
            Some((b, o, true)) if o + 2 <= b.len() => { b[o..o + 2].copy_from_slice(&v.to_le_bytes()); Ok(()) }
            _ => { self.last_fault = Some((addr, true)); Err(Fault::Prohibited) }
        }
    }
    fn write32(&mut self, addr: u32, v: u32) -> Result<(), Fault> {
        if Self::is_periph(addr) { self.periph_write(addr, v, 4); return Ok(()); }
        match self.resolve(addr) {
            Some((b, o, true)) if o + 4 <= b.len() => { b[o..o + 4].copy_from_slice(&v.to_le_bytes()); Ok(()) }
            _ => { self.last_fault = Some((addr, true)); Err(Fault::Prohibited) }
        }
    }
    fn fetch(&mut self, pc: u32) -> Result<[u8; 4], Fault> {
        match self.resolve(pc) {
            Some((b, o, _)) if o < b.len() => {
                let mut r = [0u8; 4];
                for i in 0..4 { if o + i < b.len() { r[i] = b[o + i]; } }
                Ok(r)
            }
            _ => { self.last_fault = Some((pc, false)); Err(Fault::Unmapped) }
        }
    }
    fn tick(&mut self, cycles: u32) -> u32 {
        self.cycles += cycles as u64;
        self.devices(cycles);
        1
    }
    #[inline(always)]
    fn note_pc(&mut self, pc: u32) { self.periph.misc.cur_pc = pc; }
    /// a peripheral write may have moved a line: the core's run stops so the machine re-derives it
    #[inline(always)]
    fn block_break(&self) -> bool { self.irq_dirty }
}
