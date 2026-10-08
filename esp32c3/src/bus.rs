//! ESP32-C3 memory map.
//!
//! Simpler than the S3's: one core, no PSRAM, an 8 MB cache window per bus and a flat 128-entry
//! MMU. SRAM1 is dual-mapped (IRAM `0x4038_0000` and DRAM `0x3FC8_0000` are the same bytes);
//! SRAM0 below it is the instruction cache's, reachable only from the instruction bus.

mod pins;

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
    pub ble: esp_soc::ble::vhci::Ble,
    pub sram: Vec<u8>,
    pub irom: Vec<u8>,
    pub drom: Vec<u8>,
    pub rtc_slow: Vec<u8>,
    pub flash: Vec<u8>,
    pub mmu: [u32; MMU_ENTRIES],
    pub periph: Peripherals,
    /// a bare module: nothing on the pins
    pub board: esp_soc::Board,
    uart_pins: bool,
    pub(crate) board_edges: bool,
    pub(crate) pins_active: bool,
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
            ble: Default::default(),
            sram: vec![0; SRAM_SIZE],
            irom: vec![0; (IROM_MASK_HIGH - IROM_MASK_LOW) as usize],
            drom: vec![0; (DROM_MASK_HIGH - DROM_MASK_LOW) as usize],
            rtc_slow: vec![0; (RTC_SLOW_HIGH - RTC_SLOW_LOW) as usize],
            flash: vec![0xff; flash_size],
            mmu: [MMU_INVALID; MMU_ENTRIES],
            periph: Peripherals::new(mac), board: Box::new(esp_soc::NoBoard), uart_pins: false, board_edges: false, pins_active: false,
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

    #[inline(always)]
    fn deliver_board_inputs(&mut self) {
        if !self.board_edges { return; }
        self.board.advance_to(self.cycles);
        for edge in self.board.take_edges() {
            self.periph.gpio.set_input(edge.pin, edge.level);
            if let Some(events) = &mut self.gpio_events { events.push((edge.cycle, edge.pin, edge.level)); }
            self.irq_dirty = true;
        }
        for pin in self.board.released_inputs() { esp_soc::SocBus::gpio_release_input(self, pin); }
    }

    fn periph_read(&mut self, addr: u32, size: u32) -> u32 {
        // IDF v5.5.4 components/soc/esp32c3/register/soc/gpio_reg.h: GPIO_IN/IN1.
        if matches!(addr & !3, 0x6000_403c | 0x6000_4040) { self.deliver_board_inputs(); }
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
        let drive = (self.periph.gpio.enable, self.periph.gpio.out);
        self.periph.write32(a, v);
        if a & !0xfff == 0x6001_6000 { self.pins_active = self.board_edges || self.uart_pins || self.periph.rmt.rmt.is_running(); }
        if drive != (self.periph.gpio.enable, self.periph.gpio.out) {
            self.deliver_gpio_output();
        }
        if let Some(port) = match a { 0x60000000 => Some(0), 0x60010000 => Some(1), _ => None } {
            if self.uart_pins { self.board.uart_tx(self.cycles, self.periph.uart_route(port), v as u8); }
        }
        // A SPI flash command must complete before the guest can read its result: firmware kicks
        // the command and polls/reads the data registers a few instructions later, well inside one
        // scheduling quantum. Running it at the quantum boundary instead loses the race and the
        // read returns zeros — which is exactly how `E memspi: no response` showed up on a
        // non-power-on boot while a power-on boot happened to survive it.
        if self.periph.spi_exec { self.run_spi(); }
        if a == 0x6002_4000 && self.periph.spi2.has_pending_transfer() { self.deliver_spi2_transfer(); }
        self.irq_dirty = true;
    }

    /// Attach controller 0 devices and reconnect board inputs after a reset.
    pub fn attach_board_devices(&mut self) {
        self.uart_pins = self.board.uses_uart_pins();
        self.board_edges = self.board.uses_gpio_edges();
        self.pins_active = self.board_edges || self.uart_pins || self.periph.rmt.rmt.is_running();
        for (bus, address, device) in self.board.i2c_devices() {
            if bus == 0 { self.periph.i2c.attach(address, device); }
        }
        for (pin, level) in self.board.input_levels() { self.periph.gpio.set_input(pin, level); }
        self.periph.gpio.input_changes.clear();
        self.irq_dirty = true;
        for pin in self.board.released_inputs() { esp_soc::SocBus::gpio_release_input(self, pin); }
    }

    fn deliver_gpio_output(&mut self) {
        let changes = &self.periph.gpio.changes;
        if let Some(events) = &mut self.gpio_events {
            events.extend(changes.iter().map(|&(pin, level)| (self.cycles, pin, level)));
        }
        self.board.gpio_output_at(self.cycles, changes, self.periph.gpio.enable, self.periph.gpio.out);
        self.periph.gpio.changes.clear();
    }

    fn deliver_spi2_transfer(&mut self) {
        let Some(transfer) = self.periph.spi2.take_transfer() else { return };
        let rx = if self.board.uses_spi_pins() {
            self.board.spi_transfer_pins(2, self.periph.spi2_pins(), &transfer.tx, transfer.rx_len)
        } else {
            self.board.spi_transfer(2, &transfer.tx, transfer.rx_len)
        };
        self.periph.spi2.finish_transfer(transfer, &rx);
        self.irq_dirty = true;
    }

    #[inline(never)]
    fn receive_uart_input(&mut self) {
        for input in self.board.uart_rx(self.cycles) {
            self.periph.uart_pin_input(&input);
            self.irq_dirty = true;
        }
    }

    /// Execute a pending SPI1 command against the flash image.
    fn run_spi(&mut self) {
        self.periph.spi_exec = false;
        let mut no_psram = Vec::new();
        self.periph.spi1.execute(&mut self.flash, &mut no_psram);
        self.periph.spi1.dirty.clear();
        self.periph.refresh_work();
    }

    /// Write straight into flash (image loaders, not the guest).
    pub fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> {
        let target = self.flash.get_mut(offset..).and_then(|tail| tail.get_mut(..data.len()))
            .ok_or("flash image too large")?;
        target.copy_from_slice(data);
        Ok(())
    }

    pub(crate) fn flash_off(&mut self, addr: u32) -> Option<usize> {
        if !(IBUS_LOW..IBUS_HIGH).contains(&addr) && !(DBUS_LOW..DBUS_HIGH).contains(&addr) { return None; }
        self.resolve(addr).map(|(_, off, _)| off)
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
        let (out_ch, in_ch) = { let g = &self.periph.gdma.state; (g.out_channel_for(6), g.in_channel_for(6)) };
        if let (Some(out_ch), Some(in_ch)) = (out_ch, in_ch) {
            self.periph.aes.dma_pending = false;
            let mut input = Vec::new();
            let (mut desc, mut last) = (self.periph.gdma.state.out[out_ch].desc, 0);
            for _ in 0..4096 {
                if desc == 0 { break; }
                let d = esp_periph::read_desc(&|a| self.sram32(a), desc);
                let Some(bytes) = self.sram_bytes(d.buf, d.length as usize) else { break };
                input.extend_from_slice(bytes);
                last = desc;
                if d.eof { break; }
                desc = d.next;
            }
            let c = &mut self.periph.gdma.state.out[out_ch];
            c.running = false; c.desc = 0; c.eof_desc = last;
            c.int_raw |= (1 << 0) | (1 << 1) | (1 << 3);                 // OUT_DONE, OUT_EOF, OUT_TOTAL_EOF

            let output = self.periph.aes.transform_blocks(&input);
            let (mut desc, mut pos, mut last) = (self.periph.gdma.state.inp[in_ch].desc, 0usize, 0);
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
            let c = &mut self.periph.gdma.state.inp[in_ch];
            c.running = false; c.desc = 0; c.eof_desc = last;
            c.int_raw |= (1 << 0) | (1 << 1);                            // IN_DONE, IN_SUC_EOF
        }
        if self.periph.aes.dma_pending { return; }
        self.periph.aes.state = 2;                                       // DONE
        self.periph.aes.int_raw |= 1;
        self.irq_dirty = true;
    }

    fn now_us(&self) -> u64 { self.cycles / (crate::periph::CPU_HZ / 1_000_000) }

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
            let now_us = self.now_us();
            self.periph.wifi.link.station_tx(&frame, now_us);
        }
    }

    /// The virtual air: beacons/responses from the AP and frames from the network backend land in the RX ring.
    fn wifi_air_step(&mut self) {
        let now_us = self.now_us();
        if self.periph.wifi.link.nothing_due(now_us) { return; }
        let busy = { let d = self.periph.wifi.link.last_rx_desc(); d != 0 && self.sram32(d) & esp_soc::wifi::RX_DESC_HAS_DATA != 0 };
        if let Some(frame) = self.periph.wifi.link.next_rx(now_us, busy) { self.wifi_rx_deliver(&frame, now_us); }
    }

    /// Write one received frame into the next RX descriptor (rx_ctrl header + frame + FCS) and raise the RX event.
    fn wifi_rx_deliver(&mut self, frame: &[u8], now_us: u64) {
        if self.periph.wifi.rx_next == 0 { self.periph.wifi.link.drop_rx(); return; }
        let desc = self.periph.wifi.rx_next | esp_periph::DMA_ADDR_BASE;
        let dw0 = self.sram32(desc); let buf = self.sram32(desc.wrapping_add(4)); let next = self.sram32(desc.wrapping_add(8));
        let size = (dw0 & 0xfff) as usize;
        let total = 48 + frame.len() + 4;
        if dw0 & (3 << 30) != 1 << 31 || buf == 0 || size < total { self.periph.wifi.link.drop_rx(); return; }
        let (chan, log) = { let ap = self.periph.wifi.link.ap().unwrap(); (ap.cfg.channel as u32, ap.log) };
        let mut b = Vec::with_capacity(total);
        let bcast = frame.len() >= 5 && frame[4] & 1 == 1;
        // filter-match nibble: bit 28 is the "accepted by the address filter" bit the blob's RX path
        // requires in the C3 blob; inherited from the S3 model, not measured on C3 silicon.
        let fm = if bcast { 1u32 << 28 } else { (1u32 << 28) | (1u32 << 29) };
        let w0: u32 = fm | 0xd8u32;   // rssi -40 dBm, 1 Mbps, legacy
        let w2: u32 = (chan << 16) | (chan << 20);                                        // channel, secondary
        let w5: u32 = 0xa6;                                                                // noise floor -90
        let w11: u32 = (frame.len() + 4) as u32 & 0xfff;                    // sig_len (incl. FCS), rx_state OK
        for w in [w0, 0, w2, now_us as u32, 0, w5, 0, 0, 0, 0, 0, w11] { b.extend_from_slice(&w.to_le_bytes()); }
        b.extend_from_slice(frame); b.extend_from_slice(&esp_soc::wifi::fcs(frame).to_le_bytes());
        if !self.sram_store(buf, &b) { self.periph.wifi.link.drop_rx(); return; }
        let ndw0 = (dw0 & !(0xfff << 12)) | ((total as u32) << 12) | (1 << 30) | (1 << 31);   // length; owner AND has_data set (S3-derived, checked with C3 firmware only)
        self.sram_store(desc, &ndw0.to_le_bytes());
        let w = &mut self.periph.wifi;
        w.rx_last = (desc & 0xf_ffff) | (1 << 24); w.rx_next = next & 0xf_ffff; w.link.rx_delivered(desc); w.events |= (1 << 14) | (1 << 24);   // RX data (wDev_ProcessFiq tests 0x1004000)   // registers hold masked descriptor addrs; rx_last uses the S3-derived 0x01 prefix
        if log { let d = esp_soc::wifi::describe(frame); if d.contains("auth")||d.contains("assoc") { eprintln!("[wifi] RX AUTH/ASSOC -> desc {:#010x} buf {:#010x} {}", desc, buf, d); } else { eprintln!("[wifi] RX -> desc {:#010x} {}", desc, d); } }
        self.irq_dirty = true;
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

    #[inline(always)]
    fn devices(&mut self, cycles: u32) {
        if self.periph.work_pending { self.pending_work(); }
        self.periph.tick(cycles as u64);
    }

    #[inline(never)]
    fn pending_work(&mut self) {
        if self.periph.ble_lc.enabled() {
            self.periph.ble_lc.service(&mut self.sram);
            esp_periph::Dispatch::refresh_optional(&mut self.periph, 0x31);
        }
        if self.periph.spi_exec { self.run_spi(); }
        if self.periph.aes.dma_pending { self.aes_dma_step(); }
        if !self.periph.wifi.tx_pending.is_empty() { self.wifi_tx_step(); }
        if self.periph.wifi.link.ap().is_some() { self.wifi_air_step(); let now_us = self.now_us(); self.periph.wifi.link.net_step(now_us); }
        self.periph.refresh_work();
    }

    #[inline(never)]
    fn tick_with_pins(&mut self, cycles: u32) -> u32 {
        self.devices(cycles);
        if self.periph.rmt.rmt.is_running() { self.periph.rmt.rmt.tick(cycles as u64); }
        for (ch, bits) in std::mem::take(&mut self.periph.rmt.rmt.done) {
            let pin = self.periph.gpio.pin_for_signal(51 + ch as u32).unwrap_or(u8::MAX);
            self.board.rmt_frame(pin, &bits);
            self.irq_dirty = true;
        }
        self.pins_active = self.board_edges || self.uart_pins || self.periph.rmt.rmt.is_running();
        if self.board_edges && self.board.next_deadline().is_some_and(|cycle| cycle <= self.cycles) {
            self.deliver_board_inputs();
        }
        self.periph.gpio.input_changes.clear();
        if self.uart_pins { self.receive_uart_input(); }
        1
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
        if self.pins_active { return self.tick_with_pins(cycles); }
        self.devices(cycles);
        1
    }
    #[inline(always)]
    fn note_pc(&mut self, pc: u32) { self.periph.misc.cur_pc = pc; }
    /// a peripheral write may have moved a line: the core's run stops so the machine re-derives it
    #[inline(always)]
    fn block_break(&self) -> bool { self.irq_dirty }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_clock_participation_stops_when_rmt_finishes() {
        let mut bus = SocBus::new(4 << 20, [0; 6]);
        assert!(!bus.pins_active);
        assert_eq!(bus.tick(64), 1);
        bus.write32(0x6001_6010, 1 | 1 << 8 | 1 << 16).unwrap();
        assert!(bus.pins_active);
        assert!(esp_soc::SocBus::next_deadline(&bus).is_some_and(|n| n <= 31));
        assert_eq!(bus.tick(64), 1);
        assert!(!bus.pins_active);
        assert!(bus.periph.rmt.rmt.done.is_empty());
        assert_eq!(bus.tick(64), 1);
    }
}
