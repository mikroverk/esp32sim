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

/// Decoded-code versions (`Bus::page_versions`): one counter per 256-byte page of internal SRAM,
/// indexed by its offset in `sram`, so the IRAM (`0x4037_C000`) and DRAM (`0x3FC8_0000`) views of
/// SRAM1 share one; one per page of the RTC memory at `0x5000_0000` (fetchable: IDF v5.5.4
/// components/soc/esp32c3/include/soc/soc.h:152-177, SOC_RTC_IRAM_LOW); one for the mask ROM
/// (both views); one for everything the two cache windows show through the flat MMU table.
const CODE_PAGE_SHIFT: u32 = emu_core::bus::VPAGE_SHIFT;
const SRAM_PAGES: usize = SRAM_SIZE >> CODE_PAGE_SHIFT;
const RTC_PAGES: usize = ((RTC_SLOW_HIGH - RTC_SLOW_LOW) >> CODE_PAGE_SHIFT) as usize;
const ROM_CODE: usize = SRAM_PAGES + RTC_PAGES;
const FLASH_CODE: usize = ROM_CODE + 1;
const CODE_PAGES: usize = FLASH_CODE + 1;
/// Every bus starts its versions where no earlier bus's can still be (a core outliving its bus).
static CODE_EPOCH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// A RAM address as (first version index of its memory, offset in that memory, pages in it).
#[inline(always)]
fn ram_code(addr: u32) -> Option<(usize, usize, usize)> {
    match addr {
        DRAM_LOW..=0x3FCD_FFFF => Some((0, (addr - DRAM_LOW) as usize + DRAM_IN_SRAM, SRAM_PAGES)),
        IRAM_LOW..=0x403D_FFFF => Some((0, (addr - IRAM_LOW) as usize, SRAM_PAGES)),
        RTC_SLOW_LOW..=0x5000_1FFF => Some((SRAM_PAGES, (addr - RTC_SLOW_LOW) as usize, RTC_PAGES)),
        _ => None,
    }
}

/// A write of `n` bytes at `addr` may have changed code: move the versions of the SRAM or RTC
/// pages it touches, and of the page before when it reaches into the first three bytes (a
/// four-byte instruction there starts on that page). Other addresses have no RAM version.
#[inline(always)]
fn bump_code(ver: &mut [u32], addr: u32, n: usize) {
    let Some((base, off, pages)) = ram_code(addr) else { return };
    let first = off.saturating_sub(emu_core::bus::PREV_PAGE_BYTES as usize) >> CODE_PAGE_SHIFT;
    let last = ((off + n.max(1) - 1) >> CODE_PAGE_SHIFT).min(pages - 1);
    for v in &mut ver[base + first..=base + last] { *v = v.wrapping_add(1); }
}

/// GDMA OUT memory of the crypto and SPI2 channels (`SramOut`), whose descriptor write-back
/// moves the code version of the page it lands on.
struct CodeOut<'a> { sram: esp_periph::gdma::SramOut<'a>, ver: &'a mut [u32] }
impl<'a> CodeOut<'a> {
    fn new(sram: &'a mut [u8], ver: &'a mut [u32], channel: &'a mut esp_periph::gdma::GdmaOutCh) -> Self {
        CodeOut { sram: esp_periph::gdma::SramOut { base: DRAM_LOW, bytes: &mut sram[DRAM_IN_SRAM..], channel }, ver }
    }
}
impl esp_periph::gdma::OutMemory for CodeOut<'_> {
    fn out_channel(&mut self) -> &mut esp_periph::gdma::GdmaOutCh { self.sram.out_channel() }
    fn word(&mut self, address: u32) -> Result<u32, Fault> { self.sram.word(address) }
    fn append(&mut self, address: u32, count: usize, out: &mut Vec<u8>) -> Result<(), (u32, Fault)> { self.sram.append(address, count, out) }
    fn writeback(&mut self, address: u32, value: u32) -> Result<(), Fault> {
        self.sram.writeback(address, value)?;
        bump_code(self.ver, address, 4);
        Ok(())
    }
}

pub struct SocBus {
    pub ble: esp_soc::ble::vhci::Ble,
    pub sram: Vec<u8>,
    pub irom: Vec<u8>,
    pub drom: Vec<u8>,
    pub rtc_slow: Vec<u8>,
    pub flash: Vec<u8>,
    pub mmu: [u32; MMU_ENTRIES],
    /// `Bus::page_versions`: see `CODE_PAGES`
    code_ver: Vec<u32>,
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
    /// A UART interrupt enable was written: its level is presented after the delivery latency (see `periph_write`).
    ena_latency: esp_periph::uart::EnaLatency,
    /// Host input is queued on a UART or an INT_ENA latency is pending: the only time the tick and the idle
    /// deadline look at the UARTs, so an ordinary run pays one flag test for them.
    pub(crate) uart_busy: bool,
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
            code_ver: vec![CODE_EPOCH.fetch_add(1 << 20, std::sync::atomic::Ordering::Relaxed); CODE_PAGES],
            periph: Peripherals::new(mac), board: Box::new(esp_soc::NoBoard), uart_pins: false, board_edges: false, pins_active: false,
            cycles: 0, last_fault: None, irq_dirty: true, ena_latency: Default::default(), uart_busy: false, gpio_events: None, debug: Default::default(),
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

    /// Decoded code may no longer match anything: after loading or changing memory or the
    /// configuration an instruction fetch depends on from outside the guest's own accesses.
    pub fn invalidate_code(&mut self) { for v in &mut self.code_ver { *v = v.wrapping_add(1); } }
    /// What the cache windows show may have changed (the MMU table or mapped flash bytes).
    fn bump_flash_code(&mut self) { self.code_ver[FLASH_CODE] = self.code_ver[FLASH_CODE].wrapping_add(1); }
    /// Whether a valid MMU entry shows any byte of flash `off..off + len`.
    fn flash_mapped(&self, off: usize, len: usize) -> bool {
        self.mmu.iter().any(|&e| e & MMU_INVALID == 0 && {
            let page = (e & 0xff) as usize * PAGE as usize;
            page < off.saturating_add(len) && off < page + PAGE as usize
        })
    }
    /// A store of `n` bytes at `addr` from outside the guest's bus accesses (loaders): moves the
    /// version of whatever `fetch` reads there.
    fn note_code_store(&mut self, addr: u32, n: usize) {
        match addr {
            IROM_MASK_LOW..=0x4005_FFFF | DROM_MASK_LOW..=0x3FF1_FFFF => self.code_ver[ROM_CODE] = self.code_ver[ROM_CODE].wrapping_add(1),
            DBUS_LOW..=0x3C7F_FFFF | IBUS_LOW..=0x427F_FFFF => self.bump_flash_code(),
            _ => bump_code(&mut self.code_ver, addr, n),
        }
    }

    #[inline]
    fn is_periph(addr: u32) -> bool { (PERIPH_BASE..PERIPH_END).contains(&addr) }

    fn periph_read(&mut self, addr: u32, size: u32) -> u32 {
        // IDF v5.5.4 components/soc/esp32c3/register/soc/gpio_reg.h: GPIO_IN/IN1.
        if self.board_edges && matches!(addr & !3, 0x6000_403c | 0x6000_4040) { self.irq_dirty |= esp_soc::gpio::deliver_board_inputs(&mut *self.board, &mut self.periph.gpio, &mut self.gpio_events, self.cycles); }
        if (MMU_TABLE..MMU_TABLE + (MMU_ENTRIES as u32) * 4).contains(&addr) {
            return self.mmu[((addr - MMU_TABLE) >> 2) as usize];
        }
        let a = addr & !3;
        // RXFIFO_FULL follows the FIFO level (as on the classic ESP32): the mask ROM's shared UART0/UART1 handler reads
        // one FIFO and never clears the interrupt, so a stale level would re-enter it with nothing to read.
        let uart = match a { 0x6000_0000 => Some(0), 0x6001_0000 => Some(1), _ => None };
        let before = uart.map(|i| self.periph.uart[i].irq());
        let w = self.periph.read32(a);
        if let (Some(i), Some(before)) = (uart, before) { self.irq_dirty |= self.periph.uart[i].irq() != before; }
        match size { 1 => (w >> ((addr & 3) * 8)) & 0xff, 2 => (w >> ((addr & 2) * 8)) & 0xffff, _ => w }
    }

    /// SHA DMA after a write, out of line so idle MMIO writes test only `dma_pending`.
    /// IDF v5.5.5 components/soc/esp32c3/include/soc/gdma_channel.h:15: SHA trigger 7.
    /// Bases: the same version's register/soc/reg_base.h:12-16.
    /// SHA and GDMA writes are the only events that can make this transfer ready.
    #[cold]
    #[inline(never)]
    fn sha_dma_write(&mut self, a: u32) {
        if !matches!(a & !0xfff, 0x6003_b000 | 0x6003_f000) { return; }
        if let Some(ch) = self.periph.gdma.gdma.out_channel_for(7) {
            let mut memory = CodeOut::new(&mut self.sram, &mut self.code_ver, &mut self.periph.gdma.gdma.out[ch]);
            self.periph.sha.dma_step(&mut memory);
        }
    }

    fn periph_write(&mut self, addr: u32, v: u32, size: u32) {
        if (MMU_TABLE..MMU_TABLE + (MMU_ENTRIES as u32) * 4).contains(&addr) {
            let entry = &mut self.mmu[((addr - MMU_TABLE) >> 2) as usize];
            if *entry != v & 0x1ff { *entry = v & 0x1ff; self.bump_flash_code(); }
            return;
        }
        let a = addr & !3;
        // A sub-word store merges into the register's current value, but reading a UART FIFO pops a received byte:
        // TX FIFO stores merge into 0 instead.
        let fifo = matches!(a, 0x6000_0000 | 0x6001_0000);
        let v = match size {
            4 => v,
            _ if fifo => if size == 1 { (v & 0xff) << ((addr & 3) * 8) } else { (v & 0xffff) << ((addr & 2) * 8) },
            1 => { let old = self.periph.read32(a); let sh = (addr & 3) * 8; (old & !(0xff << sh)) | ((v & 0xff) << sh) }
            _ => { let old = self.periph.read32(a); let sh = (addr & 2) * 8; (old & !(0xffff << sh)) | ((v & 0xffff) << sh) }
        };
        let ena_port = match a { 0x6000_000c => Some(0), 0x6001_000c => Some(1), _ => None };
        let old_ena = ena_port.map_or(0, |i| self.periph.uart[i].int_ena);
        let drive = (self.periph.gpio.enable, self.periph.gpio.out);
        // Reuse the peripheral's pin-block classification; only those blocks start or stop I2C/RMT.
        if self.periph.write32(a, v) { self.pins_active = self.pins_active(); }
        if matches!(a & !0xfff, 0x6000_4000 | 0x6000_9000) && self.board.uses_gpio_waveform() {
            self.board.gpio_waveform_at(self.cycles, esp_soc::pins::ChipPins::C3.routes(&self.periph.gpio, &self.periph.io_mux));
        }
        if self.periph.sha.dma_pending { self.sha_dma_write(a); }
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
        // SPI2 (0x6002_4000) or GDMA (0x6003_f000): either can be the last write before a DMA
        // data phase can run, as drivers start the out link before or after CMD.USR.
        if matches!(a & !0xfff, 0x6002_4000 | 0x6003_f000) && self.periph.spi2.has_pending_transfer() {
            if self.periph.spi2.dma_tx_pending.is_some() { self.spi2_dma_step(); }
            self.deliver_spi2_transfer();
        }
        // Interrupt delivery has latency on silicon (the interrupt matrix and the CPU's interrupt controller). The mask ROM's
        // uart_buff_switch enables UART0's interrupt and stores which UART its handler serves in the next instruction; taking
        // the interrupt between the two runs the handler for the previous UART (USB) and faults. So an INT_ENA write lets the
        // block run on and its level is presented after `EnaLatency::CYCLES`.
        match ena_port {
            Some(i) => { if self.ena_latency.on_write(old_ena, self.periph.uart[i].int_ena) { self.irq_dirty = true; } else { self.uart_busy = true; } }
            None => self.irq_dirty = true,
        }
    }

    /// Supply a GP-SPI2 MOSI data phase from the GDMA out channel bound to it, through the shared
    /// `gather_out` like the C6's `spi2_dma_tx` and the SHA path above.
    /// SPI2 is GDMA peripheral 0 (IDF v5.5.4 components/soc/esp32c3/include/soc/gdma_channel.h).
    /// OUT_TOTAL_EOF (bit 3 of the shared layout) is raised when the chain ends at a null link. A chain
    /// that faults or ends short raises OUT_DSCR_ERR and the command still completes, without data, as on the S3.
    /// The data phase completes at once, without wire time.
    /// Model choice, as on the S3: a chain that ends before the SPI data length raises OUT_DSCR_ERR and
    /// sends nothing, where silicon would not flag a descriptor error and the SPI side would underflow.
    #[cold]
    #[inline(never)]
    fn spi2_dma_step(&mut self) {
        let Some(bits) = self.periph.spi2.dma_tx_pending else { return };
        let Some(ch) = self.periph.gdma.gdma.out_channel_for(0) else { return };
        let wanted = bits.div_ceil(8) as usize;
        let mut payload = Vec::with_capacity(wanted);
        let mut memory = CodeOut::new(&mut self.sram, &mut self.code_ver, &mut self.periph.gdma.gdma.out[ch]);
        let result = esp_periph::gdma::gather_out(&mut memory, wanted, &mut payload);
        let c = &mut self.periph.gdma.gdma.out[ch];
        c.running = false; c.buf_pos = 0;
        let complete = match result {
            Ok(next) => { c.desc = next; if next == 0 { c.int_raw |= 1 << 3; } payload.len() == wanted }
            Err(_) => false,
        };
        if !complete {
            c.int_raw |= 1 << 2;
            if self.periph.spi2.log { eprintln!("[spi2] DMA descriptor fault ({} of {} bytes)", payload.len(), wanted); }
            self.periph.spi2.fail_dma_tx();
            return;
        }
        self.periph.spi2.complete_dma_tx(&payload);
    }

    #[inline(always)]
    fn pins_active(&self) -> bool {
        self.board_edges || self.uart_pins || self.periph.rmt.rmt.is_running() || self.periph.i2c.is_active()
    }

    /// Attach controller 0 devices and reconnect board inputs after a reset.
    pub fn attach_board_devices(&mut self) {
        if self.board.uses_gpio_waveform() {
            self.board.gpio_waveform_at(self.cycles, esp_soc::pins::ChipPins::C3.routes(&self.periph.gpio, &self.periph.io_mux));
        }
        self.uart_pins = self.board.uses_uart_pins();
        self.board_edges = self.board.uses_gpio_edges();
        self.pins_active = self.pins_active();
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
        // Only a program or erase moves the flash version, and only where the MMU shows it.
        if self.periph.spi1.dirty.iter().any(|&(m, off, len)| m == esp_periph::DirtyMem::Flash && self.flash_mapped(off, len)) { self.bump_flash_code(); }
        self.periph.spi1.dirty.clear();
        self.periph.refresh_work();
    }

    /// Write straight into flash (image loaders, not the guest).
    pub fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> {
        let target = self.flash.get_mut(offset..).and_then(|tail| tail.get_mut(..data.len()))
            .ok_or("flash image too large")?;
        target.copy_from_slice(data);
        self.bump_flash_code();
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
        bump_code(&mut self.code_ver, addr, data.len());
        true
    }

    /// AES through GDMA (peripheral 6): the OUT chain is the input, the cipher is the shared
    /// model's, the result goes into the IN chain, all in the round that set the trigger. Both
    /// chains are read through the shared walker (`gather_out`, `GdmaInCh::receive`), so
    /// CHECK_OWNER and the descriptor budget apply as for SHA and SPI2; the OUT data comes from
    /// data SRAM, the IN data never lands on MMIO. Each channel stops after the transaction.
    /// A malformed OUT chain ends the input where it is and raises OUT_DSCR_ERR (no
    /// OUT_TOTAL_EOF); a malformed or short IN chain keeps what fits and raises IN_DSCR_ERR
    /// (with IN_DSCR_EMPTY when it ran out). The AES block reports done either way, as before.
    fn aes_dma_step(&mut self) {
        let (out_ch, in_ch) = { let g = &self.periph.gdma.gdma; (g.out_channel_for(6), g.in_channel_for(6)) };
        if let (Some(out_ch), Some(in_ch)) = (out_ch, in_ch) {
            self.periph.aes.dma_pending = false;
            let mut input = Vec::new();
            let mut memory = CodeOut::new(&mut self.sram, &mut self.code_ver, &mut self.periph.gdma.gdma.out[out_ch]);
            let result = esp_periph::gdma::gather_out(&mut memory, usize::MAX, &mut input);
            let c = &mut self.periph.gdma.gdma.out[out_ch];
            c.running = false; c.buf_pos = 0;
            match result {
                Ok(next) => { c.desc = next; c.int_raw |= 1 << 3; }       // OUT_TOTAL_EOF
                Err(_) => c.int_raw |= 1 << 2,                          // OUT_DSCR_ERR
            }

            let output = self.periph.aes.transform_blocks(&input);
            let mut channel = self.periph.gdma.gdma.inp[in_ch];
            let mut irq_changed = false;
            if channel.receive(self, &output, Some(output.len() as u32), false, Self::is_periph, &mut irq_changed) {
                channel.running = false; channel.desc = 0;
            }
            self.periph.gdma.gdma.inp[in_ch] = channel;
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

    /// The receive descriptor at `desc` (a masked pointer from `rx_next`) as the DMA would use it for a
    /// `total`-byte frame, if software armed it: owned and not yet filled, with a buffer, and with room
    /// for the frame. `None` means the receive DMA is stopped there. Stricter than the S3, which also
    /// takes a descriptor whose `has_data` is still set. With the EX238 hand-over that difference also
    /// decides the restart: a still-filled descriptor plus a base write since the last frame sends the
    /// C3 to `rx_base`, where the S3 overwrites the descriptor in place. A driver that processes frames
    /// in order never gets there.
    fn wifi_rx_armed(&self, desc: u32, total: usize) -> Option<(u32, u32, u32, u32)> {
        if desc == 0 { return None; }
        let d = desc | esp_periph::DMA_ADDR_BASE;
        let dw0 = self.sram32(d); let buf = self.sram32(d.wrapping_add(4)); let next = self.sram32(d.wrapping_add(8));
        (dw0 & (3 << 30) == 1 << 31 && buf != 0 && (dw0 & 0xfff) as usize >= total).then_some((d, dw0, buf, next))
    }

    /// Write one received frame into the next RX descriptor (rx_ctrl header + frame + FCS) and raise the RX event.
    fn wifi_rx_deliver(&mut self, frame: &[u8], now_us: u64) {
        let total = 48 + frame.len() + 4;
        // As on the S3 (EX238): a descriptor that is not usable means the receive DMA has stopped; a
        // `BASE_RX_DSCR` write since the last frame is the hand-over of a fresh list, so try once more there.
        let mut armed = self.wifi_rx_armed(self.periph.wifi.rx_next, total);
        if armed.is_none() && self.periph.wifi.rx_restart_at_base() { armed = self.wifi_rx_armed(self.periph.wifi.rx_next, total); }
        // Neither a head nor a pending base: no receive ring yet, counted apart from lost frames (#276).
        if self.periph.wifi.rx_next == 0 { self.periph.wifi.link.drop_rx_unarmed(); return; }
        let Some((desc, dw0, buf, next)) = armed else { self.periph.wifi.link.drop_rx(); return; };
        // A network around's frames carry its own channel and signal (`VirtualAp::rx_meta`)
        let (chan, rssi, log) = { let link = &self.periph.wifi.link; let (c, r) = link.rx_meta(frame); (c as u32, r, link.rx_log()) };
        let mut b = Vec::with_capacity(total);
        // address-match bits (`StationLink::rx_match`): inherited from the S3 model, not measured on C3 silicon
        let fm = self.periph.wifi.link.rx_match(frame);
        let w0: u32 = fm | (rssi as u8 as u32);   // rssi (−40 dBm for the access point), 1 Mbps, legacy
        let w2: u32 = (chan << 16) | (chan << 20);                                        // channel, secondary
        let w5: u32 = 0xa6;                                                                // noise floor -90
        let w11: u32 = (frame.len() + 4) as u32 & 0xfff;                    // sig_len (incl. FCS), rx_state OK
        for w in [w0, 0, w2, now_us as u32, 0, w5, 0, 0, 0, 0, 0, w11] { b.extend_from_slice(&w.to_le_bytes()); }
        b.extend_from_slice(frame); b.extend_from_slice(&esp_soc::wifi::fcs(frame).to_le_bytes());
        if !self.sram_store(buf, &b) { self.periph.wifi.link.drop_rx(); return; }
        let ndw0 = (dw0 & !(0xfff << 12)) | ((total as u32) << 12) | (1 << 30) | (1 << 31);   // length; owner AND has_data set (S3-derived, checked with C3 firmware only)
        self.sram_store(desc, &ndw0.to_le_bytes());
        self.periph.wifi.rx_filled(desc, next);   // rx_last's 0x01 prefix is S3-derived
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
            self.note_code_store(a, 1);
        }
        Ok(())
    }

    /// Bus cycles at which the current peripheral clock started.
    pub(crate) fn peripheral_clock_offset(&self) -> u64 { self.cycles - self.periph.clock_cycles() }

    // ESP-IDF v5.5.5 components/soc/esp32c3/include/soc/gdma_channel.h:13, I2S0 trigger 3.
    fn i2s_rx_step(&mut self, cycles: u64) {
        let Some(ch) = self.periph.gdma.gdma.in_channel_for(3) else { return };
        let bytes = self.i2s_receive(cycles);
        let eof = self.periph.i2s0.read(0x64);
        let mut channel = self.periph.gdma.gdma.inp[ch];
        let mut irq_changed = false;
        let _ = channel.receive(self, &bytes, Some(eof), false, Self::is_periph, &mut irq_changed);
        self.periph.i2s0.recycle_rx_buffer(bytes);
        self.irq_dirty |= irq_changed;
        self.periph.gdma.gdma.inp[ch] = channel;
    }

    /// PCM for one RX interval: pin-routed sources when a bank is attached, else controller input.
    /// Kept out of line so the RX DMA pump keeps the EX214 call shape.
    #[inline(never)]
    fn i2s_receive(&mut self, cycles: u64) -> Vec<u8> {
        let signals = esp_periph::i2s::RxSignals { data: 15, clock: [16, 17], input_select_bit: 6, output_mask: 0x1ff };
        let mut bank = self.periph.i2s0.take_pcm_bank();
        let bytes = self.periph.i2s0.receive(cycles, self.cycles, false, &self.periph.gpio, signals, bank.as_deref_mut());
        self.periph.i2s0.restore_pcm_bank(bank);
        bytes
    }

    #[inline(always)]
    fn devices(&mut self, cycles: u32) {
        if self.periph.work_pending { self.pending_work(cycles); }
        self.periph.tick(cycles as u64);
        if self.uart_busy { self.pace_uart_input(cycles); }
    }

    /// Each UART's configured baud rate, for pacing queued host input.
    /// CLK_CONF (0x78) is not seeded with its reset value, so a ROM or stub that never writes it reads source 0 there, and
    /// the route has no baud. Pacing then assumes what the autobaud model does (`Uart::autobaud_pulse`): APB, 80 MHz,
    /// divider 1. So esptool's baud change, which writes only CLKDIV, speeds up the paced input as it does on silicon.
    fn uart_bauds(&self) -> [Option<u32>; 2] {
        std::array::from_fn(|port| {
            let u = &self.periph.uart[port];
            if (u.clock_config() >> 20) & 3 == 0 { u.baud(1 << 20, 17_500_000) } else { self.periph.uart_route(port).baud }
        })
    }
    /// Release host bytes queued by `uart_input` at the configured baud rate.
    #[cold]
    #[inline(never)]
    fn pace_uart_input(&mut self, cycles: u32) {
        let bauds = self.uart_bauds();
        self.irq_dirty |= esp_periph::uart::pace_input(self.periph.uart.iter_mut().zip(bauds), cycles as u64, crate::periph::CPU_HZ);
        self.uart_busy = self.ena_latency.pending() || self.periph.uart.iter().any(|u| u.has_queued());
    }
    /// Cycles an idle core may skip before queued UART input or a pending INT_ENA level is due.
    #[cold]
    #[inline(never)]
    pub(crate) fn uart_deadline(&self) -> Option<u64> {
        if !self.uart_busy { return None; }
        let input = if self.periph.uart.iter().any(|u| u.has_queued()) {
            esp_periph::uart::input_deadline(self.periph.uart.iter().zip(self.uart_bauds()), crate::periph::CPU_HZ)
        } else { None };
        input.into_iter().chain(self.ena_latency.deadline()).min()
    }

    #[inline(never)]
    fn pending_work(&mut self, cycles: u32) {
        if self.periph.i2s_rx { self.i2s_rx_step(u64::from(cycles)); }
        if self.periph.ble_lc.enabled() {
            self.periph.ble_lc.service(&mut self.sram);
            self.note_ble_writes();
            esp_periph::Dispatch::refresh_optional(&mut self.periph, 0x31);
        }
        if self.periph.spi_exec { self.run_spi(); }
        if self.periph.aes.dma_pending { self.aes_dma_step(); }
        if !self.periph.wifi.tx_pending.is_empty() { self.wifi_tx_step(); }
        if self.periph.wifi.link.on_air() { self.wifi_air_step(); let now_us = self.now_us(); self.periph.wifi.link.net_step(now_us); }
        self.periph.refresh_work();
    }

    /// The link controller's exchange-memory stores move the code versions of the SRAM they span.
    fn note_ble_writes(&mut self) {
        if let Some((lo, hi)) = self.periph.ble_lc.take_written() { bump_code(&mut self.code_ver, IRAM_LOW + lo as u32, hi + 1 - lo); }
    }

    /// Every routed pin of a completed transmitter receives its frame (mirrored strips).
    #[cold]
    #[inline(never)]
    fn deliver_rmt_frames(&mut self) {
        for (ch, bits) in std::mem::take(&mut self.periph.rmt.rmt.done) {
            for pin in esp_soc::pins::ChipPins::C3.routes(&self.periph.gpio, &self.periph.io_mux).output_pins(51 + ch as u32) { self.board.rmt_frame(pin, &bits); }
            self.irq_dirty = true;
        }
    }

    #[inline(never)]
    fn tick_with_pins(&mut self, cycles: u32) -> u32 {
        let advance_board = self.periph.i2c.is_active()
            || (self.board_edges && self.board.next_deadline().is_some_and(|cycle| cycle <= self.cycles));
        if advance_board { self.board.advance_to(self.cycles); }
        self.devices(cycles);
        if self.periph.rmt.rmt.is_running() { self.periph.rmt.rmt.tick(cycles as u64); }
        if !self.periph.rmt.rmt.done.is_empty() { self.deliver_rmt_frames(); }
        self.pins_active = self.pins_active();
        if self.board_edges && advance_board {
            self.irq_dirty |= esp_soc::gpio::drain_board_inputs(&mut *self.board, &mut self.periph.gpio, &mut self.gpio_events, self.cycles);
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
    fn note_code_page(&mut self, _vidx: u32) {} // Every write that can change code moves its version (`CODE_PAGES`).
    #[inline(always)]
    fn page_versions(&self) -> &[u32] { &self.code_ver }
    /// `CODE_PAGES`; `u32::MAX` (no version) where nothing can be fetched.
    #[inline(always)]
    fn code_page(&mut self, pc: u32) -> u32 {
        if let Some((base, off, _)) = ram_code(pc) { return (base + (off >> CODE_PAGE_SHIFT)) as u32; }
        match pc {
            IROM_MASK_LOW..=0x4005_FFFF | DROM_MASK_LOW..=0x3FF1_FFFF => ROM_CODE as u32,
            DBUS_LOW..=0x3C7F_FFFF | IBUS_LOW..=0x427F_FFFF => FLASH_CODE as u32,
            _ => u32::MAX,
        }
    }
    /// `fetch` without its fault record: the same memories, MMU and zero fill past an end.
    #[inline(always)]
    fn fetch_ahead(&mut self, pc: u32) -> Option<[u8; 4]> {
        let (b, o, _) = self.resolve(pc)?;
        if o >= b.len() { return None; }
        let mut r = [0u8; 4];
        for (i, byte) in r.iter_mut().enumerate() { if o + i < b.len() { *byte = b[o + i]; } }
        Some(r)
    }
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
            Some((b, o, true)) if o < b.len() => b[o] = v,
            _ => { self.last_fault = Some((addr, true)); return Err(Fault::Prohibited); }
        }
        bump_code(&mut self.code_ver, addr, 1);
        Ok(())
    }
    fn write16(&mut self, addr: u32, v: u16) -> Result<(), Fault> {
        if Self::is_periph(addr) { self.periph_write(addr, v as u32, 2); return Ok(()); }
        match self.resolve(addr) {
            Some((b, o, true)) if o + 2 <= b.len() => b[o..o + 2].copy_from_slice(&v.to_le_bytes()),
            _ => { self.last_fault = Some((addr, true)); return Err(Fault::Prohibited); }
        }
        bump_code(&mut self.code_ver, addr, 2);
        Ok(())
    }
    fn write32(&mut self, addr: u32, v: u32) -> Result<(), Fault> {
        if Self::is_periph(addr) { self.periph_write(addr, v, 4); return Ok(()); }
        match self.resolve(addr) {
            Some((b, o, true)) if o + 4 <= b.len() => b[o..o + 4].copy_from_slice(&v.to_le_bytes()),
            _ => { self.last_fault = Some((addr, true)); return Err(Fault::Prohibited); }
        }
        bump_code(&mut self.code_ver, addr, 4);
        Ok(())
    }
    fn fetch(&mut self, pc: u32) -> Result<[u8; 4], Fault> {
        match self.fetch_ahead(pc) {
            Some(r) => Ok(r),
            None => { self.last_fault = Some((pc, false)); Err(Fault::Unmapped) }
        }
    }
    fn tick(&mut self, cycles: u32) -> u32 {
        self.cycles += cycles as u64;
        if self.uart_busy && self.ena_latency.tick(cycles as u64) { self.irq_dirty = true; }
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

    /// The ROM's uart_buff_switch: INT_ENA is written and the next instructions store which UART the handler serves.
    /// The level must not reach the CPU at the tick ending the writing block, however many cycles that tick carries.
    #[test]
    fn uart_int_ena_is_presented_after_a_latency_counted_from_the_next_tick() {
        let mut bus = SocBus::new(4 << 20, [0; 6]);
        bus.write32(0x6000_0024, 1).unwrap();
        esp_soc::SocBus::uart_input(&mut bus, 0, b"x");
        bus.irq_dirty = false;
        bus.write32(0x6000_000c, 1).unwrap();
        assert!(!bus.irq_dirty, "not on the next instruction");
        bus.tick(4096);
        assert!(!bus.irq_dirty, "not at the tick that ends the block holding the write");
        assert_eq!(esp_soc::SocBus::next_deadline(&bus), Some(esp_periph::uart::EnaLatency::CYCLES), "an idle core stops for it");
        bus.tick(1);
        assert!(!bus.irq_dirty);
        bus.tick(esp_periph::uart::EnaLatency::CYCLES as u32);
        assert!(bus.irq_dirty);
    }

    /// With CLK_CONF never written, pacing follows CLKDIV at the APB clock the autobaud model assumes: esptool's change
    /// to 921600 (CLKDIV = 80 MHz / 921600) paces at 921600, not at the 115200 fallback.
    #[test]
    fn uart_pacing_follows_clkdiv_when_clk_conf_was_never_written() {
        let mut bus = SocBus::new(4 << 20, [0; 6]);
        assert_eq!(bus.uart_bauds()[0], None, "no divider yet: the 115200 fallback");
        let div = 80_000_000 * 16 / 921_600;
        bus.write32(0x6000_0014, (div / 16) | (div % 16) << 20).unwrap();
        let b = bus.uart_bauds()[0].unwrap();
        assert!((920_000..=923_000).contains(&b), "{b}");
    }

    /// Masking a UART interrupt takes effect at once: only newly enabled sources wait for the latency.
    #[test]
    fn uart_int_ena_disable_is_presented_at_once() {
        let mut bus = SocBus::new(4 << 20, [0; 6]);
        bus.write32(0x6000_000c, 1).unwrap();
        bus.tick(4096); bus.tick(esp_periph::uart::EnaLatency::CYCLES as u32 + 1);
        bus.irq_dirty = false;
        bus.write32(0x6000_000c, 0).unwrap();
        assert!(bus.irq_dirty, "a mask is presented on the next instruction");
        assert_eq!(esp_soc::SocBus::next_deadline(&bus), None, "and arms no latency");
        bus.irq_dirty = false;
        bus.write32(0x6000_000c, 1).unwrap();
        assert!(!bus.irq_dirty, "a new enable still waits");
    }

    /// Queued host input bounds the idle deadline at the next byte time, and a byte store to the TX FIFO keeps RX bytes.
    #[test]
    fn queued_uart_input_bounds_the_idle_deadline_and_byte_stores_do_not_pop_rx() {
        let mut bus = SocBus::new(4 << 20, [0; 6]);
        esp_soc::SocBus::uart_input(&mut bus, 0, &[7; 200]);
        assert_eq!(esp_soc::SocBus::next_deadline(&bus), None, "full FIFO: nothing can move until a read");
        assert_eq!(bus.read8(0x6000_0000), Ok(7));
        let per_byte = crate::periph::CPU_HZ * 10 / 115_200;
        assert_eq!(esp_soc::SocBus::next_deadline(&bus), Some(per_byte));
        bus.write8(0x6000_0000, b'Z').unwrap();
        assert_eq!(bus.periph.uart[0].rx_pending(), 127, "the byte store transmitted without reading the FIFO");
        assert_eq!(bus.periph.uart[0].tx_out.last(), Some(&b'Z'));
    }

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

    #[test]
    fn pin_clock_participation_tracks_i2c_transfers() {
        let mut bus = SocBus::new(4 << 20, [0; 6]);
        bus.write32(0x6001_3058, 2 << 11).unwrap();
        assert!(!bus.pins_active, "an idle controller stays off the pin path");
        bus.write32(0x6001_3004, 1 << 5).unwrap();
        assert!(bus.pins_active, "TRANS_START enters pin service");
        assert_eq!(esp_soc::SocBus::next_deadline(&bus), Some(6), "STOP deadline in CPU cycles");
        assert_eq!(bus.tick(64), 1);
        assert!(!bus.periph.i2c.is_active());
        assert!(!bus.pins_active, "completion leaves pin service");
    }
    /// As on the S3: a frame from a network around reaches the RX descriptor with that network's
    /// channel and RSSI (`VirtualAp::rx_meta`; `wifi_pkt_rx_ctrl_t`, IDF v5.5.5
    /// components/esp_wifi/include/local/esp_wifi_types_native.h:36, :59-60); a frame from the
    /// access point keeps its channel and -40 dBm.
    #[test]
    fn wifi_rx_ctrl_carries_a_nearby_networks_channel_and_rssi() {
        use esp_soc::wifi::{ApConfig, BeaconAp, VirtualAp};
        const DESC: u32 = 0x3fc9_0100;
        const BUF: u32 = 0x3fc9_0200;
        let mut bus = SocBus::new(4 << 20, [0; 6]);
        bus.periph.wifi.link.attach(Some(VirtualAp::new(ApConfig::parse("ssid=demo,chan=6").unwrap(), false)), None);
        let home = [0x02, 1, 2, 3, 4, 5];
        let left_out = bus.periph.wifi.link.set_beacons(vec![BeaconAp { ssid: "Home".into(), bssid: home, channel: 1, rssi: -60, secure: true }]);
        assert_eq!(left_out, Some(vec![]));
        let ap = bus.periph.wifi.link.ap().unwrap().cfg.bssid;
        for (from, chan, rssi) in [(home, 1u32, -60i8), (ap, 6, -40)] {
            bus.write32(DESC, (1 << 31) | 0xfff).unwrap();                     // owner DMA, size 4095
            bus.write32(DESC + 4, BUF).unwrap();
            bus.write32(DESC + 8, 0).unwrap();
            bus.periph.wifi.rx_next = DESC & 0xf_ffff;
            let mut frame = vec![0u8; 24];
            frame[0] = 0x80;                                                     // beacon
            frame[4..10].fill(0xff);
            frame[10..16].copy_from_slice(&from);
            bus.wifi_rx_deliver(&frame, 1234);
            assert_eq!(bus.read32(BUF).unwrap() & 0xff, u32::from(rssi as u8), "RSSI of {from:02x?}");
            assert_eq!(bus.read32(BUF + 8).unwrap(), (chan << 16) | (chan << 20), "channel of {from:02x?}");
        }
    }

    /// A WiFi bus with one open virtual access point and `frames` of 24 bytes queued on the air, the
    /// first byte of each a marker so the test can tell them apart in the receive buffer.
    fn wifi_bus_with_air_frames(frames: &[u8]) -> SocBus {
        use esp_soc::wifi::{AirFrame, ApConfig, VirtualAp};
        let mut bus = SocBus::new(4 << 20, [0; 6]);
        bus.periph.wifi.link.attach(Some(VirtualAp::new(ApConfig {
            ssid: "test".into(), bssid: [2, 0, 0, 0, 0, 1], channel: 1, psk: None, epoch_s: None,
        }, false)), None);
        for &marker in frames {
            bus.periph.wifi.link.ap_mut().unwrap().queue.push(AirFrame { at_us: 0, frame: vec![marker; 24] });
        }
        bus.periph.refresh_work();
        bus
    }

    /// One RX descriptor: `512` bytes of buffer at `buf`, owned by the DMA, pointing at `next`.
    fn arm_rx_desc(bus: &mut SocBus, at: u32, buf: u32, next: u32) {
        bus.write32(at, 512 | (1 << 31)).unwrap();
        bus.write32(at + 4, buf).unwrap();
        bus.write32(at + 8, next).unwrap();
    }

    /// One scheduling round of one emulated millisecond: the air delivers at most one frame in a round.
    fn wifi_air_round(bus: &mut SocBus) { assert_eq!(bus.tick((crate::periph::CPU_HZ / 1000) as u32), 1); }

    const RX_DESC: u32 = 0x3fc9_0100;

    /// EX238 on the C3 (the S3's `air_rx_restarts_at_the_base_when_the_list_runs_dry`): a driver
    /// that arms one short list at a time restarts the stopped receive DMA with a `BASE_RX_DSCR` write.
    #[test]
    fn air_rx_restarts_at_the_base_when_the_list_runs_dry() {
        let mut bus = wifi_bus_with_air_frames(&[0x11, 0x22, 0x33]);
        let (first, dead, second) = (RX_DESC, RX_DESC + 0x40, RX_DESC + 0x80);
        let dead_base = dead & 0xf_ffff;
        arm_rx_desc(&mut bus, first, first + 0x200, dead);          // the armed list: one descriptor, then a null word
        bus.write32(dead, 0).unwrap();
        bus.write32(0x6003_3088, first & 0xf_ffff).unwrap();        // BASE_RX_DSCR
        wifi_air_round(&mut bus);
        assert_eq!((bus.periph.wifi.link.tx_frames(), bus.periph.wifi.link.rx_frames()), (0, 1));
        assert_eq!(bus.read8(first + 0x200 + 48).unwrap(), 0x11, "frame after the 48-byte rx_ctrl header");
        assert_eq!(bus.periph.wifi.rx_next, dead_base);

        // Software recycled the frame and left the next descriptor alone: the DMA stops there.
        arm_rx_desc(&mut bus, first, first + 0x200, dead);
        wifi_air_round(&mut bus);
        assert_eq!(bus.periph.wifi.link.rx_dropped(), 1, "a frame that finds the DMA stopped is lost");

        // The driver hands over the next list, base first and the base write on its own.
        arm_rx_desc(&mut bus, second, second + 0x200, 0);
        bus.write32(0x6003_3088, second & 0xf_ffff).unwrap();
        wifi_air_round(&mut bus);
        assert_eq!(bus.periph.wifi.link.rx_frames(), 2, "the base write restarts the stopped DMA");
        assert_eq!(bus.periph.wifi.rx_next, 0, "the new list ends after one descriptor");
        assert_eq!(bus.read8(second + 0x200 + 48).unwrap(), 0x33);
    }

    /// The other half of the rule (the S3's `air_rx_base_write_does_not_rewind_a_running_list`):
    /// software rewrites `BASE_RX_DSCR` on every recycle, and a running DMA keeps its pointer.
    #[test]
    fn air_rx_base_write_does_not_rewind_a_running_list() {
        let mut bus = wifi_bus_with_air_frames(&[0x11, 0x22]);
        let (first, second) = (RX_DESC, RX_DESC + 0x40);
        arm_rx_desc(&mut bus, first, first + 0x200, second);
        arm_rx_desc(&mut bus, second, second + 0x200, 0);
        bus.write32(0x6003_3088, first & 0xf_ffff).unwrap();
        wifi_air_round(&mut bus);
        assert_eq!((bus.periph.wifi.link.rx_frames(), bus.periph.wifi.rx_next), (1, (second & 0xf_ffff)));

        arm_rx_desc(&mut bus, first, first + 0x200, second);        // recycle descriptor 0
        bus.write32(0x6003_3088, first & 0xf_ffff).unwrap();        // and rewrite the base, as software does
        wifi_air_round(&mut bus);
        assert_eq!(bus.periph.wifi.link.rx_frames(), 2);
        assert_eq!(bus.read8(second + 0x200 + 48).unwrap(), 0x22, "landed in the next descriptor, not back at the base");
        assert_eq!(bus.read32(first).unwrap() >> 30 & 1, 0, "descriptor 0 was not filled a second time");
    }

    /// EX265: reading ahead returns `fetch`'s bytes wherever it succeeds, `None` where it faults,
    /// and never records the fault.
    #[test]
    fn fetch_ahead_is_fetch_without_its_fault_records() {
        let mut b = SocBus::new(1 << 20, [0; 6]);
        b.mmu[0] = 0; b.mmu[1] = 15; b.mmu[2] = 16;                          // 0x4200_0000: flash page 0; 0x3C02_0000: beyond the 1 MB flash
        b.write_flash(0xfffe, &[0x13, 0]).unwrap(); b.write_flash(0x2_0000 - 2, &[0x13, 0x05]).unwrap();
        b.load_bytes(IROM_MASK_LOW + 0x40, &[0x13, 0, 0, 0]).unwrap();
        b.write32(IRAM_LOW + 0x4000, 0x0050_0513).unwrap();
        b.write16(DRAM_HIGH - 2, 0x4505).unwrap();
        b.write32(RTC_SLOW_LOW + 8, 0x0050_0513).unwrap();
        for pc in [IRAM_LOW, IRAM_LOW + 0x4000, IRAM_LOW + 0x4002, DRAM_LOW, DRAM_LOW + 2, IRAM_HIGH - 2, IRAM_HIGH, DRAM_HIGH - 2, DRAM_HIGH,
                   RTC_SLOW_LOW + 8, RTC_SLOW_HIGH - 2, RTC_SLOW_HIGH, IROM_MASK_LOW + 0x40, IROM_MASK_HIGH - 2, DROM_MASK_LOW, DROM_MASK_HIGH - 1,
                   IBUS_LOW, IBUS_LOW + 0xfffe, IBUS_LOW + 0x1_fffe, DBUS_LOW + 0x1_fffe, DBUS_LOW + 0x2_0000, IBUS_LOW + 0x3_0000, IBUS_HIGH - 2,
                   0x6000_0000, MMU_TABLE, 0, u32::MAX - 1] {
            b.last_fault = None;
            let ahead = b.fetch_ahead(pc);
            assert!(b.last_fault.is_none(), "{pc:#x}: fetch_ahead recorded a fault");
            assert_eq!(ahead, b.fetch(pc).ok(), "{pc:#x}");
        }
        assert_eq!(b.fetch_ahead(IRAM_HIGH - 2), Some([0x05, 0x45, 0, 0]), "zeros past the end, as `fetch`");
    }

    /// EX265: what moves the code versions (`Bus::page_versions`) and what leaves them alone.
    #[test]
    fn code_versions_move_with_every_write_that_can_change_fetched_code() {
        let mut b = SocBus::new(1 << 20, [0; 6]);
        let ver = |b: &mut SocBus, pc: u32| { let i = b.code_page(pc); b.page_versions()[i as usize] };
        let moves = |b: &mut SocBus, pc: u32, f: &mut dyn FnMut(&mut SocBus)| { let v = ver(b, pc); f(b); ver(b, pc) != v };
        let pc = IRAM_LOW + 0x5000;
        let alias = pc - IRAM_LOW - DRAM_IN_SRAM as u32 + DRAM_LOW;
        assert_eq!(b.code_page(pc), b.code_page(alias), "both views of SRAM1 share a version");
        assert_ne!(b.code_page(pc), b.code_page(pc + 0x100));
        assert!(moves(&mut b, pc, &mut |b| b.write32(pc + 8, 1).unwrap()), "a store on the page");
        assert!(moves(&mut b, pc, &mut |b| b.write8(alias + 8, 1).unwrap()), "a store through the data view");
        assert!(moves(&mut b, pc, &mut |b| b.write16(alias + 0x100, 1).unwrap()), "a store in the first bytes of the next page");
        assert!(!moves(&mut b, pc, &mut |b| b.write32(pc + 0x104, 1).unwrap()), "a store further into the next page");
        assert!(!moves(&mut b, pc, &mut |b| b.write32(pc + 0x400, 1).unwrap()), "a store elsewhere");
        assert!(moves(&mut b, RTC_SLOW_LOW + 0x40, &mut |b| b.write32(RTC_SLOW_LOW + 0x44, 1).unwrap()), "an RTC memory store");
        assert!(!moves(&mut b, pc, &mut |b| b.write32(RTC_SLOW_LOW + 0x44, 1).unwrap()), "RTC memory is not SRAM");
        // what no guest store reaches: DMA, the radio models, loaders
        assert!(moves(&mut b, pc, &mut |b| b.write32_unpriced(alias + 0x10, 1).unwrap()), "a GDMA IN scatter (`GdmaInCh::receive`)");
        assert!(moves(&mut b, pc, &mut |b| {
            let mut ch = esp_periph::gdma::GdmaOutCh::default();
            esp_periph::gdma::OutMemory::writeback(&mut CodeOut::new(&mut b.sram, &mut b.code_ver, &mut ch), alias + 0x20, 0).unwrap();
        }), "a GDMA OUT descriptor write-back (SHA, AES, SPI2)");
        assert!(moves(&mut b, pc, &mut |b| assert!(b.sram_store(alias + 0x30, &[1, 2, 3]))), "a WiFi RX frame");
        assert!(moves(&mut b, pc, &mut |b| { b.periph.ble_lc.put(&mut b.sram, (pc - IRAM_LOW) as usize + 0x40, &[1, 2]); b.note_ble_writes(); }), "a BLE exchange-memory store");
        assert!(moves(&mut b, pc, &mut |b| b.load_bytes(pc, &[0x13]).unwrap()), "loading");
        assert!(moves(&mut b, IROM_MASK_LOW + 0x40, &mut |b| b.load_bytes(DROM_MASK_LOW, &[0x13]).unwrap()), "loading the ROM");
        assert!(moves(&mut b, pc, &mut |b| { esp_soc::SocBus::reboot(b, [0; 6]); }), "a chip reset");
        // the cache windows: the MMU and the flash array behind it
        let flash = IBUS_LOW + 0x10;
        assert!(moves(&mut b, flash, &mut |b| b.write32(MMU_TABLE, 0).unwrap()), "an MMU entry");
        assert!(!moves(&mut b, flash, &mut |b| b.write32(MMU_TABLE, 0).unwrap()), "rewriting an MMU entry unchanged");
        assert!(moves(&mut b, DBUS_LOW, &mut |b| b.write32(MMU_TABLE + 4, 1).unwrap()), "an MMU entry for the data window");
        assert!(!moves(&mut b, flash, &mut |b| b.read32(MMU_TABLE).map(|_| ()).unwrap()), "an MMU read");
        let spi1 = |b: &mut SocBus, cmd: u32, addr: u32| { b.write32(0x6000_2004, addr).unwrap(); b.write32(0x6000_2000, cmd).unwrap(); };
        assert!(moves(&mut b, flash, &mut |b| spi1(b, 1 << 24, 0x1_0000)), "a sector erase in mapped flash (SPI1 FLASH_SE)");
        assert!(!moves(&mut b, flash, &mut |b| spi1(b, 1 << 24, 0x8_0000)), "a sector erase in flash no MMU entry shows");
        assert!(!moves(&mut b, flash, &mut |b| spi1(b, 1 << 28, 0)), "an SPI1 read of the JEDEC ID");
        assert!(moves(&mut b, flash, &mut |b| b.write_flash(0x8_0000, &[0]).unwrap()), "writing the flash image");
        assert!(moves(&mut b, flash, &mut |b| b.load_bytes(flash, &[0x13]).unwrap()), "loading through the window (the BLE trampoline)");
        assert!(moves(&mut b, flash, &mut |b| esp_soc::SocBus::set_flash_size(b, 1 << 20)), "a new flash array");
        assert!(!moves(&mut b, flash, &mut |b| b.write32(pc, 1).unwrap()), "an SRAM store");
        assert!(!moves(&mut b, IROM_MASK_LOW, &mut |b| b.write32(pc, 1).unwrap()), "an SRAM store and the ROM");
        assert!(!moves(&mut b, flash, &mut |b| b.write32(0x6000_0024, 1).unwrap()), "a peripheral register");
        for no_code in [0x6000_0000, MMU_TABLE, 0x3FC0_0000, 0x4000_0000 - 4, 0] { assert_eq!(b.code_page(no_code), u32::MAX, "{no_code:#x}"); }
    }

    /// Self-modifying code with blocks (and the JIT where it exists): a routine in IRAM is run
    /// often enough to be a block (and compiled), then rewritten through the DRAM view by a CPU
    /// store and called again, then rewritten by a DMA write (a WiFi RX frame) and called again.
    /// Each call must run the bytes as they are now, as the interpreter does.
    #[test]
    fn code_rewritten_through_the_data_view_or_by_dma_runs_new() {
        use emu_core::Core;
        const fn i(imm: i32, rs1: u32, f3: u32, rd: u32, op: u32) -> u32 { ((imm as u32) << 20) | rs1 << 15 | f3 << 12 | rd << 7 | op }
        const fn j(rd: u32, off: i32) -> u32 { let o = off as u32; (o >> 20 & 1) << 31 | (o >> 1 & 0x3ff) << 21 | (o >> 11 & 1) << 20 | (o >> 12 & 0xff) << 12 | rd << 7 | 0x6f }
        const fn bne(rs1: u32, rs2: u32, off: i32) -> u32 { let o = off as u32; (o >> 12 & 1) << 31 | (o >> 5 & 0x3f) << 25 | rs2 << 20 | rs1 << 15 | 1 << 12 | (o >> 1 & 0xf) << 8 | (o >> 11 & 1) << 7 | 0x63 }
        const fn li_a0(v: i32) -> u32 { i(v, 0, 0, 10, 0x13) }
        let (base, t) = (IRAM_LOW + 0x4000, IRAM_LOW + 0x4200);
        let t_data = DRAM_LOW + 0x200;
        let main = [
            i(100, 0, 0, 8, 0x13),                       // 0x00 li s0, 100
            j(1, 0x1fc),                                 // 0x04 jal t
            i(-1, 8, 0, 8, 0x13),                        // 0x08 addi s0, s0, -1
            bne(8, 0, -8),                               // 0x0c bnez s0, 0x04
            0x3fc8_02b7,                                 // 0x10 lui t0, 0x3fc80
            0x0020_0337,                                 // 0x14 lui t1, 0x200
            i(0x513, 6, 0, 6, 0x13),                     // 0x18 addi t1, t1, 0x513: `li a0, 2`
            0x2062_a023,                                 // 0x1c sw t1, 0x200(t0)
            j(1, 0x1e0),                                 // 0x20 jal t
            j(0, 0),                                     // 0x24 j .
            j(1, 0x1d8),                                 // 0x28 jal t
            j(0, 0),                                     // 0x2c j .
        ];
        for jit in [true, false] {
            let mut b = SocBus::new(1 << 20, [0; 6]);
            for (k, w) in main.iter().enumerate() { b.write32(base + 4 * k as u32, *w).unwrap(); }
            b.write32(t, li_a0(1)).unwrap(); b.write32(t + 4, i(0, 1, 0, 0, 0x67)).unwrap();   // t: li a0, 1; ret
            let mut cpu = riscv_rv32::Cpu::new();
            cpu.set_jit(jit);
            cpu.pc = base;
            let run_to = |cpu: &mut riscv_rv32::Cpu, b: &mut SocBus, pc: u32| {
                for _ in 0..1000 { b.irq_dirty = false; let (_, trap) = cpu.run(b, 64); assert!(trap.is_none()); if cpu.pc == pc { return; } }
                panic!("never reached {pc:#x}: pc {:#x}", cpu.pc);
            };
            run_to(&mut cpu, &mut b, base + 0x24);
            assert_eq!(cpu.x[10], 2, "the CPU store through the DRAM view (jit {jit})");
            assert!(b.sram_store(t_data, &li_a0(3).to_le_bytes()));
            cpu.pc = base + 0x28;
            run_to(&mut cpu, &mut b, base + 0x2c);
            assert_eq!(cpu.x[10], 3, "the DMA write (jit {jit})");
            let (builds, _, compiled, _) = cpu.code_cache_stats().unwrap();
            assert_eq!(builds != 0, jit, "blocks ran only with blocks on");
            let compiles = jit && riscv_rv32::jit::AVAILABLE && std::env::var_os("RV32_JIT").is_none_or(|v| v != "0");
            assert_eq!(compiled != 0, compiles, "the routine was compiled where there is a JIT");
        }
    }
}
