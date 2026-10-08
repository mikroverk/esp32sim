//! ESP32-C6 memory map.
//!
//! One address space for instructions and data: the 320 KB mask ROM at `0x4000_0000`, 512 KB of
//! HP SRAM at `0x4080_0000`, 16 KB of LP SRAM at `0x5000_0000`, and a single 16 MB flash cache
//! window at `0x4200_0000` behind a 256-entry MMU. The MMU is programmed through two SPI0
//! registers (item index / item content), not a memory-mapped table as on the C3.

use crate::periph::{Peripherals, CPU_SUB_BASE, CPU_SUB_END, PERIPH_BASE, PERIPH_END};
use esp_periph::read_desc;
use riscv_rv32::bus::{Bus, Fault};

/// GPIO matrix output signal of RMT TX channel 0 (soc/gpio_sig_map.h); channel n is this + n.
pub const RMT_SIG_OUT0: u32 = 71;
pub const ROM_LOW: u32 = 0x4000_0000;
pub const ROM_HIGH: u32 = 0x4005_0000;
pub const SRAM_LOW: u32 = 0x4080_0000;
pub const SRAM_HIGH: u32 = 0x4088_0000;
pub const LP_SRAM_LOW: u32 = 0x5000_0000;
pub const LP_SRAM_HIGH: u32 = 0x5000_4000;
pub const FLASH_LOW: u32 = 0x4200_0000;
pub const FLASH_HIGH: u32 = 0x4300_0000;
pub const MMU_ENTRIES: usize = 256;
/// bit 9 marks an entry valid; bits 8:0 are the flash page
pub const MMU_VALID: u32 = 1 << 9;
/// SPI0 registers the MMU is driven through
pub const SPI_MMU_ITEM_CONTENT: u32 = 0x37c;
pub const SPI_MMU_ITEM_INDEX: u32 = 0x380;
pub const SPI_MMU_POWER_CTRL: u32 = 0x384;

pub struct SocBus {
    pub ble: crate::ble::Ble,
    pub rom: Vec<u8>,
    pub sram: Vec<u8>,
    pub lp_sram: Vec<u8>,
    pub flash: Vec<u8>,
    pub mmu: [u32; MMU_ENTRIES],
    pub mmu_index: u32,
    /// SPI_MEM_MMU_POWER_CTRL: bits 4:3 select the page size (0 = 64 KB, 1 = 32, 2 = 16, 3 = 8)
    pub mmu_power_ctrl: u32,
    pub periph: Peripherals,
    /// a bare module: nothing on the pins
    pub board: esp_soc::Board,
    uart_pins: bool,
    pub cycles: u64,
    pub last_fault: Option<(u32, bool)>,
    /// a peripheral write may have moved an interrupt line: re-derive before the next instruction
    pub irq_dirty: bool,
    /// GPIO edges for observers, while one wants them: (cycle, pin, level)
    pub gpio_events: Option<Vec<(u64, u8, bool)>>,
    pub debug: esp_soc::DebugFlags,
    board_edges: bool,
}

impl SocBus {
    /// Attach controller devices and restore inputs driven by the persistent board.
    pub fn attach_board_devices(&mut self) {
        self.uart_pins = self.board.uses_uart_pins();
        self.board_edges = self.board.uses_gpio_edges();
        for (bus, address, device) in self.board.i2c_devices() {
            if bus == 0 { self.periph.i2c.attach(address, device); }
        }
        for (pin, level) in self.board.input_levels() {
            self.irq_dirty |= self.periph.gpio.set_input(pin, level);
        }
        for pin in self.board.released_inputs() { esp_soc::SocBus::gpio_release_input(self, pin); }
    }

    pub fn new(flash_size: usize, mac: [u8; 6]) -> Self {
        SocBus {
            ble: Default::default(),
            rom: vec![0; (ROM_HIGH - ROM_LOW) as usize],
            sram: vec![0; (SRAM_HIGH - SRAM_LOW) as usize],
            lp_sram: vec![0; (LP_SRAM_HIGH - LP_SRAM_LOW) as usize],
            flash: vec![0xff; flash_size],
            mmu: [0; MMU_ENTRIES], mmu_index: 0, mmu_power_ctrl: 0,
            periph: Peripherals::new(mac), board: Box::new(esp_soc::NoBoard), uart_pins: false,
            cycles: 0, last_fault: None, irq_dirty: true, gpio_events: None, debug: Default::default(), board_edges: false,
        }
    }

    /// log2 of the MMU page size
    #[inline]
    pub fn page_shift(&self) -> u32 { 16 - ((self.mmu_power_ctrl >> 3) & 3) }

    /// Resolve to (buffer, offset, writable). The flash window goes through the MMU.
    fn resolve(&mut self, addr: u32) -> Option<(&mut Vec<u8>, usize, bool)> {
        match addr {
            SRAM_LOW..=0x4087_FFFF => Some((&mut self.sram, (addr - SRAM_LOW) as usize, true)),
            ROM_LOW..=0x4004_FFFF => Some((&mut self.rom, (addr - ROM_LOW) as usize, false)),
            LP_SRAM_LOW..=0x5000_3FFF => Some((&mut self.lp_sram, (addr - LP_SRAM_LOW) as usize, true)),
            FLASH_LOW..=0x42FF_FFFF => {
                let shift = self.page_shift();
                let idx = ((addr - FLASH_LOW) >> shift) as usize;
                if idx >= MMU_ENTRIES { return None; }
                let entry = self.mmu[idx];
                if entry & MMU_VALID == 0 { return None; }
                let off = (((entry & 0x1ff) as usize) << shift) + (addr & ((1 << shift) - 1)) as usize;
                if off < self.flash.len() { Some((&mut self.flash, off, false)) } else { None }
            }
            _ => None,
        }
    }

    #[inline]
    fn is_periph(addr: u32) -> bool { (PERIPH_BASE..PERIPH_END).contains(&addr) || (CPU_SUB_BASE..CPU_SUB_END).contains(&addr) }

    fn periph_read(&mut self, addr: u32, size: u32) -> u32 {
        // IDF v5.5.4 components/soc/esp32c6/register/soc/gpio_reg.h: GPIO_IN/IN1.
        if self.board_edges && matches!(addr & !3, 0x6009_103c | 0x6009_1040) { self.irq_dirty |= esp_soc::gpio::deliver_board_inputs(&mut *self.board, &mut self.periph.gpio, &mut self.gpio_events, self.cycles); }
        let w = if (CPU_SUB_BASE..CPU_SUB_END).contains(&addr) {
            self.periph.cpu_sub_read(addr - CPU_SUB_BASE)
        } else if (addr & !0xfff) == PERIPH_BASE + 0x2000 && matches!(addr & 0xfff, SPI_MMU_ITEM_CONTENT | SPI_MMU_ITEM_INDEX | SPI_MMU_POWER_CTRL) {
            match addr & 0xfff {
                SPI_MMU_ITEM_CONTENT => self.mmu[(self.mmu_index as usize) & (MMU_ENTRIES - 1)],
                SPI_MMU_ITEM_INDEX => self.mmu_index,
                _ => self.mmu_power_ctrl,
            }
        } else {
            self.periph.read32(addr & !3)
        };
        match size { 1 => (w >> ((addr & 3) * 8)) & 0xff, 2 => (w >> ((addr & 2) * 8)) & 0xffff, _ => w }
    }

    fn periph_write(&mut self, addr: u32, v: u32, size: u32) {
        let a = addr & !3;
        let merge = |old: u32| match size {
            4 => v,
            1 => { let sh = (addr & 3) * 8; (old & !(0xff << sh)) | ((v & 0xff) << sh) }
            _ => { let sh = (addr & 2) * 8; (old & !(0xffff << sh)) | ((v & 0xffff) << sh) }
        };
        if (CPU_SUB_BASE..CPU_SUB_END).contains(&a) {
            let old = self.periph.cpu_sub_read(a - CPU_SUB_BASE);
            self.periph.cpu_sub_write(a - CPU_SUB_BASE, merge(old));
            self.irq_dirty = true;
            return;
        }
        if (a & !0xfff) == PERIPH_BASE + 0x2000 {
            match a & 0xfff {
                SPI_MMU_ITEM_CONTENT => { self.mmu[(self.mmu_index as usize) & (MMU_ENTRIES - 1)] = merge(self.mmu[(self.mmu_index as usize) & (MMU_ENTRIES - 1)]) & 0x7ff; return; }
                SPI_MMU_ITEM_INDEX => { self.mmu_index = merge(self.mmu_index) & 0xff; return; }
                SPI_MMU_POWER_CTRL => { self.mmu_power_ctrl = merge(self.mmu_power_ctrl); return; }
                _ => {}
            }
        }
        let v = if size == 4 { v } else { merge(self.periph.read32(a)) };
        let old_drive = (self.periph.gpio.enable, self.periph.gpio.out);
        self.periph.write32(a, v);
        if old_drive != (self.periph.gpio.enable, self.periph.gpio.out) {
            self.deliver_gpio_output();
        }
        if let Some(port) = match a { 0x60000000 => Some(0), 0x60001000 => Some(1), _ => None } {
            if self.uart_pins { self.board.uart_tx(self.cycles, self.periph.uart_route(port), v as u8); }
        }
        // A SPI flash command must complete before the guest reads its result (see the C3 notes:
        // running it at the quantum boundary loses the race and reads back zeros).
        if self.periph.spi_exec { self.run_spi(); }
        // TX_START: the radio's DMA reads the frame now, at the instruction that started it
        if self.periph.radio.tx_request.is_some() { self.radio_tx_fetch(); }
        // A GP-SPI transfer reaches the board now, after the GPIO edges that preceded it: the
        // display's D/C line is a GPIO the driver sets right before each transaction.
        if self.periph.spi2.dma_tx_pending.is_some() { self.spi2_dma_tx(); }
        if self.periph.spi2.has_pending_transfer() || !self.periph.gpio.changes.is_empty() { self.deliver_board_events(); }
        self.irq_dirty = true;
    }

    /// A word of SRAM for the DMA engines (descriptors and buffers live there).
    fn sram32(&self, addr: u32) -> u32 {
        let o = addr.wrapping_sub(SRAM_LOW) as usize;
        if o.checked_add(4).is_some_and(|end| end <= self.sram.len()) { u32::from_le_bytes(self.sram[o..o + 4].try_into().unwrap()) } else { 0 }
    }

    /// The 802.15.4 TX DMA: `buf[0]` is the PSDU length including the 2-byte FCS the hardware
    /// appends, `buf[1..]` the MAC frame. A buffer outside SRAM or a length under 2 is a driver
    /// bug on real silicon too; here it is reported and the transmission carries no bytes.
    fn radio_tx_fetch(&mut self) {
        let Some(addr) = self.periph.radio.tx_request.take() else { return };
        let o = addr.wrapping_sub(SRAM_LOW) as usize;
        let psdu = match self.sram.get(o) {
            Some(&len) => {
                let mac = (len & 0x7f).saturating_sub(2) as usize;
                if o.checked_add(1 + mac).is_some_and(|end| end <= self.sram.len()) { self.sram[o + 1..o + 1 + mac].to_vec() } else { Vec::new() }
            }
            None => Vec::new(),
        };
        if psdu.is_empty() { eprintln!("[802.15.4] TX_START with an empty or unmapped frame at {:#010x}", addr); }
        self.periph.radio.tx_loaded(psdu);
    }

    /// The 802.15.4 RX DMA: the completed frame, RSSI and LQI go where `DMA_RX_ADDR` points.
    fn radio_rx_store(&mut self) {
        let Some((addr, buf)) = self.periph.radio.rx_write.take() else { return };
        let o = addr.wrapping_sub(SRAM_LOW) as usize;
        if o.checked_add(buf.len()).is_some_and(|end| end <= self.sram.len()) { self.sram[o..o + buf.len()].copy_from_slice(&buf); }
        else { eprintln!("[802.15.4] RX_DONE: DMA_RX_ADDR {:#010x} is not in SRAM, frame lost", addr); }
    }

    /// A frame from the medium (MAC header + payload, no FCS): started `Some(cycles)` ago (RX_DONE
    /// when its air time is up) or complete now (`None`; the buffer is written before the next
    /// instruction). Returns whether the radio took it; the interrupt lines are re-derived before
    /// the next instruction either way.
    pub fn radio_receive(&mut self, frame: &[u8], rssi: i8, lqi: u8, started_ago: Option<u64>) -> bool {
        let taken = self.periph.radio.receive(frame, rssi, lqi, started_ago);
        if self.periph.radio.rx_write.is_some() { self.radio_rx_store(); }
        self.irq_dirty = true;
        taken
    }

    /// GP-SPI2's MOSI data phase through the GDMA out-channel bound to it (trigger 0): walk the
    /// descriptor chain, hand the bytes to the SPI model, and complete the channel.
    /// Bytes of HP SRAM for the WiFi MAC's DMA, or nothing if the range is not all SRAM.
    fn sram_bytes(&self, addr: u32, len: usize) -> Option<&[u8]> {
        let o = addr.wrapping_sub(SRAM_LOW) as usize;
        o.checked_add(len).filter(|&end| end <= self.sram.len()).map(|end| &self.sram[o..end])
    }
    fn sram_store(&mut self, addr: u32, data: &[u8]) -> bool {
        let o = addr.wrapping_sub(SRAM_LOW) as usize;
        match o.checked_add(data.len()).filter(|&end| end <= self.sram.len()) {
            Some(end) => { self.sram[o..end].copy_from_slice(data); true }
            None => false,
        }
    }
    fn now_us(&self) -> u64 { self.cycles / (crate::periph::CPU_HZ / 1_000_000) }

    /// WiFi transmit: the frames the library queued come out of their descriptors (word 0 bits
    /// 27:14 the packet's length, word 1 the packet), complete at once, and go to the access point; what
    /// it passes on as data goes to the network behind it.
    fn wifi_tx_step(&mut self) {
        let now_us = self.now_us();
        for (queue, low) in std::mem::take(&mut self.periph.wifi_mac.tx_pending) {
            let desc = self.periph.wifi_mac.addr(low);
            let (dw0, pkt) = (self.sram32(desc), self.sram32(desc.wrapping_add(4)));
            // The packet is an 8-byte hardware header (word 0 bits 13:0 the frame's length) and the frame.
            const TX_HEADER: usize = 8;
            let packet = self.sram_bytes(pkt, ((dw0 >> 14) & 0x3fff) as usize).unwrap_or_default();
            let len = packet.get(..4).map_or(0, |w| (u32::from_le_bytes(w.try_into().unwrap()) & 0x3fff) as usize);
            let frame = packet.get(TX_HEADER..TX_HEADER + len).unwrap_or_default().to_vec();
            if frame.is_empty() && !packet.is_empty() { eprintln!("[wifi] TX queue {}: a {}-byte packet whose header says {} bytes of frame; not sent", queue, packet.len(), len); }
            if self.periph.wifi_mac.log || self.debug.has("wifi-frames") { eprintln!("[wifi] TX queue {} desc {:#010x} {}", queue, desc, esp_soc::wifi::describe(&frame)); }
            let mac = &mut self.periph.wifi_mac;
            mac.tx_done(queue);
            mac.link.station_tx(&frame, now_us);
            self.irq_dirty = true;
        }
    }

    /// The virtual air: what the access point has due (beacons, responses) and what the network
    /// sends the station, one frame at a time into the RX ring, paced as on the other chips
    /// (`StationLink::next_rx`).
    fn wifi_air_step(&mut self) {
        let now_us = self.now_us();
        if self.periph.wifi_mac.link.nothing_due(now_us) { return; }
        let last_desc = self.periph.wifi_mac.link.last_rx_desc();
        let busy = last_desc != 0 && self.sram32(last_desc) & esp_soc::wifi::RX_DESC_HAS_DATA != 0;
        if let Some(frame) = self.periph.wifi_mac.link.next_rx(now_us, busy) { self.wifi_rx_deliver(&frame, now_us); }
    }

    /// One received frame into the next RX descriptor, behind the control header this MAC puts in
    /// front. The layout is the one `wDev_ProcessRxSucData` reads, which is not the public
    /// `esp_wifi_rxctrl_t`: 84 fixed bytes (byte 0 the RSSI, byte 3 bits 4 and 5 the address
    /// match, byte 8 the end state, bytes 12..15 the timestamp, bytes 33..34 bits 9:0 the length
    /// of a channel-estimate dump), that dump, then 8 bytes with the frame length (14 bits, FCS
    /// included) and the receive state, then the frame and its FCS. No channel estimate is
    /// produced, so the header is 92 bytes. The library writes the channel into byte 21 itself.
    fn wifi_rx_deliver(&mut self, frame: &[u8], now_us: u64) {
        const RX_CTRL: usize = 92;
        let mac = &self.periph.wifi_mac;
        if mac.rx_next == 0 { self.periph.wifi_mac.link.drop_rx(); return; }
        let desc = mac.addr(mac.rx_next);
        let log = mac.link.ap().is_some_and(|ap| ap.log);
        let (dw0, buf, next) = (self.sram32(desc), self.sram32(desc.wrapping_add(4)), self.sram32(desc.wrapping_add(8)));
        let total = RX_CTRL + frame.len() + 4;
        // hardware-owned, empty, and big enough (size is the low 14 bits)
        if dw0 & (1 << 31) == 0 || dw0 & (1 << 30) != 0 || ((dw0 & 0x3fff) as usize) < total { self.periph.wifi_mac.link.drop_rx(); return; }
        let group = frame.len() >= 5 && frame[4] & 1 == 1;
        let mut words = [0u32; RX_CTRL / 4];
        words[0] = 0xd8 | 1 << 28 | if group { 0 } else { 1 << 29 };   // rssi -40 dBm, 1 Mbps legacy; match 0, and match 1 for our own address
        words[3] = now_us as u32;                                       // timestamp
        words[5] = 0xa6;                                                // noise floor -90 dBm
        words[21] = (frame.len() as u32 + 4) & 0x3fff;                  // byte 84: the frame's length with its FCS
        words[22] = 0;                                                  // byte 88: receive state, 0 = good
        let mut b: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        b.extend_from_slice(frame);
        b.extend_from_slice(&esp_soc::wifi::fcs(frame).to_le_bytes());
        if !self.sram_store(buf, &b) { self.periph.wifi_mac.link.drop_rx(); return; }
        let filled = (dw0 & !(0x3fff << 14)) | (total as u32) << 14 | 1 << 30;           // length, has_data; size and owner stay
        self.sram_store(desc, &filled.to_le_bytes());
        self.periph.wifi_mac.rx_filled(desc, next);
        if log { eprintln!("[wifi] RX -> desc {:#010x} (was {:#010x}, next {:#010x}) buf {:#010x} {}", desc, dw0, next, buf, esp_soc::wifi::describe(frame)); }
        self.irq_dirty = true;
    }

    fn spi2_dma_tx(&mut self) {
        let Some(bits) = self.periph.spi2.dma_tx_pending else { return };
        let want = (bits as usize).div_ceil(8);
        let Some(ch) = self.periph.gdma.gdma.out_channel_for(0) else { return };   // not started yet: the round's end retries
        let mut data = Vec::with_capacity(want);
        let (mut desc, mut last) = (self.periph.gdma.gdma.out[ch].desc, self.periph.gdma.gdma.out[ch].desc);
        let mut guard = 0;
        while desc != 0 && data.len() < want && guard < 4096 {
            guard += 1;
            let d = read_desc(&|a| self.sram32(a), desc);
            let n = (d.length as usize).min(want - data.len());
            let o = d.buf.wrapping_sub(SRAM_LOW) as usize;
            if o.checked_add(n).is_some_and(|end| end <= self.sram.len()) { data.extend_from_slice(&self.sram[o..o + n]); } else { break; }
            last = desc;
            if d.eof { break; }
            desc = d.next;
        }
        let c = &mut self.periph.gdma.gdma.out[ch];
        c.running = false; c.desc = 0; c.eof_desc = last;
        c.int_raw |= (1 << 0) | (1 << 1) | (1 << 3);                 // OUT_DONE, OUT_EOF, OUT_TOTAL_EOF
        self.periph.spi2.complete_dma_tx(&data);
    }

    /// AES through GDMA (peripheral 6), which is the only way ESP-IDF's driver uses the block on
    /// this chip: the OUT chain is the input, the cipher is the shared model's, the result goes
    /// into the IN chain, each descriptor written back with its length, SUC_EOF on the last and
    /// the owner handed to the CPU. All of it happens in the round that set the trigger. A chain
    /// that leaves SRAM, or an IN chain too short for the result, ends the transform where it is:
    /// the driver sees the AES done with what was delivered.
    fn aes_dma_step(&mut self) {
        self.periph.aes.dma_pending = false;
        let (out_ch, in_ch) = { let g = &self.periph.gdma.gdma; (g.out_channel_for(6), g.in_channel_for(6)) };
        if let (Some(out_ch), Some(in_ch)) = (out_ch, in_ch) {
            let mut input = Vec::new();
            let (mut desc, mut last) = (self.periph.gdma.gdma.out[out_ch].desc, 0);
            for _ in 0..4096 {
                if desc == 0 { break; }
                let d = read_desc(&|a| self.sram32(a), desc);
                let Some(bytes) = self.sram_bytes(d.buf, d.length as usize) else { break };
                input.extend_from_slice(bytes);
                last = desc;
                if d.eof { break; }
                desc = d.next;
            }
            let c = &mut self.periph.gdma.gdma.out[out_ch];
            c.running = false; c.desc = 0; c.eof_desc = last;
            c.int_raw |= (1 << 0) | (1 << 1) | (1 << 3);                 // OUT_DONE, OUT_EOF, OUT_TOTAL_EOF

            let output = self.periph.aes.transform_blocks(&input);
            let (mut desc, mut pos, mut last) = (self.periph.gdma.gdma.inp[in_ch].desc, 0usize, 0);
            for _ in 0..4096 {
                if desc == 0 || pos >= output.len() { break; }
                let d = read_desc(&|a| self.sram32(a), desc);
                let n = (d.size as usize).min(output.len() - pos);
                if n == 0 || !self.sram_store(d.buf, &output[pos..pos + n]) { break; }
                pos += n;
                let eof = pos == output.len();
                let dw0 = (self.sram32(desc) & !(0xfff << 12) & !(3 << 30)) | (n as u32) << 12 | if eof { 1 << 30 } else { 0 };
                self.sram_store(desc, &dw0.to_le_bytes());
                last = desc;
                desc = d.next;
            }
            let c = &mut self.periph.gdma.gdma.inp[in_ch];
            c.running = false; c.desc = 0; c.eof_desc = last;
            c.int_raw |= (1 << 0) | (1 << 1);                            // IN_DONE, IN_SUC_EOF
        }
        self.periph.aes.state = 2;                                       // DONE
        self.periph.aes.int_raw |= 1;
        self.irq_dirty = true;
    }

    fn deliver_gpio_output(&mut self) {
        let ch = &self.periph.gpio.changes;
        if let Some(ev) = &mut self.gpio_events { for &(pin, level) in ch { ev.push((self.cycles, pin, level)); } }
        self.board.gpio_output_at(self.cycles, ch, self.periph.gpio.enable, self.periph.gpio.out);
        self.periph.gpio.changes.clear();
    }

    /// GPIO edges precede SPI transfers and completed RMT frames.
    fn deliver_board_events(&mut self) {
        if !self.periph.gpio.changes.is_empty() { self.deliver_gpio_output(); }
        if let Some(transfer) = self.periph.spi2.take_transfer() {
            let rx = if self.board.uses_spi_pins() {
                // IDF v5.5.4 components/soc/esp32c6/include/soc/gpio_sig_map.h:115-126,197-209;
                // register/soc/io_mux_reg.h:182-209,243-272 gives native function 2 and pins.
                let pins = esp_soc::pins::ChipPins::C6.routes(&self.periph.gpio, &self.periph.io_mux)
                    .spi_pins(&self.periph.spi2, [63, 64, 65, 68, 101, 102, 103, 104, 105], 128, &[(2, 6, 7, 2, &[16, 17, 18, 19, 20, 21])]);
                self.board.spi_transfer_pins(2, pins, &transfer.tx, transfer.rx_len)
            } else {
                self.board.spi_transfer(2, &transfer.tx, transfer.rx_len)
            };
            self.periph.spi2.finish_transfer(transfer, &rx);
        }
        if !self.periph.rmt.rmt.done.is_empty() { for (ch, bits) in std::mem::take(&mut self.periph.rmt.rmt.done) { let pin = self.periph.gpio.pin_for_signal(RMT_SIG_OUT0 + ch as u32).unwrap_or(u8::MAX); self.board.rmt_frame(pin, &bits); } self.irq_dirty = true; }
    }

    /// Execute a pending SPI1 command against the flash image.
    fn run_spi(&mut self) {
        self.periph.spi_exec = false;
        let mut no_psram = Vec::new();
        self.periph.spi1.0.execute(&mut self.flash, &mut no_psram);
        self.periph.spi1.0.dirty.clear();
    }

    /// Write straight into flash (image loaders, not the guest).
    pub fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> {
        let target = self.flash.get_mut(offset..).and_then(|tail| tail.get_mut(..data.len()))
            .ok_or("flash image too large")?;
        target.copy_from_slice(data);
        Ok(())
    }

    pub(crate) fn flash_off(&mut self, addr: u32) -> Option<usize> {
        if !(FLASH_LOW..FLASH_HIGH).contains(&addr) { return None; }
        self.resolve(addr).map(|(_, off, _)| off)
    }

    pub fn load_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        // The mask ROM ELF links its .eh_frame at the flash window; nothing ever reads it.
        if (FLASH_LOW..FLASH_HIGH).contains(&addr) { return Ok(()); }
        for (i, b) in data.iter().enumerate() {
            let a = addr.wrapping_add(i as u32);
            match self.resolve(a) {
                Some((buf, off, _)) if off < buf.len() => buf[off] = *b,
                _ => return Err(format!("load: address {:#010x} not mapped", a)),
            }
        }
        Ok(())
    }

    /// Run the SPI1 controller if the guest just kicked it, advance device time, deliver what
    /// the devices produced to the board.
    fn devices(&mut self, cycles: u32) {
        if self.periph.spi_exec { self.run_spi(); }
        self.periph.tick(cycles as u64);
        self.periph.gpio.input_changes.clear();
        if self.board_edges { self.irq_dirty |= esp_soc::gpio::deliver_board_inputs(&mut *self.board, &mut self.periph.gpio, &mut self.gpio_events, self.cycles); }
        if self.uart_pins {
            for input in self.board.uart_rx(self.cycles) {
                self.periph.uart_pin_input(&input);
                self.irq_dirty = true;
            }
        }
        if self.periph.radio.rx_write.is_some() { self.radio_rx_store(); }
        if self.periph.spi2.dma_tx_pending.is_some() { self.spi2_dma_tx(); }
        if self.periph.aes.dma_pending { self.aes_dma_step(); }
        if !self.periph.wifi_mac.tx_pending.is_empty() { self.wifi_tx_step(); }
        if self.periph.wifi_mac.link.ap().is_some() { self.wifi_air_step(); let now_us = self.now_us(); self.periph.wifi_mac.link.net_step(now_us); }
        self.deliver_board_events();
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
