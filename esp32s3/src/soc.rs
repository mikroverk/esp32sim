//! The ESP32-S3 as a `Soc`: two LX7 cores on `bus::SocBus`, and the questions the machine asks
//! its bus (console, reset, app boot, board, audio, interrupt routing).
use crate::bus::{SocBus, DBUS_HIGH, DBUS_LOW, IBUS_HIGH, IBUS_LOW, MMU_ENTRIES, MMU_INVALID, SRC_FLASH};
use crate::periph::{self, NUM_SOURCES};
use esp_periph::Misc;
use esp_soc::{BoardModel, CoreState, Soc};
use xtensa_lx7::state::ps;
use xtensa_lx7::Cpu;

pub struct S3;
pub type Machine = esp_soc::Machine<S3>;

/// A machine with the default 8 MB flash / 2 MB PSRAM and no board (`bus.board` selects one).
pub fn machine(mac: [u8; 6]) -> Machine { let mut m = Machine::new(mac, SocBus::new(8 << 20, 2 << 20, mac)); m.set_debug(&esp_soc::DebugFlags::from_env()); m }

impl Soc for S3 {
    type Core = Cpu;
    type Bus = SocBus;
    const NAME: &'static str = "esp32s3";
    const ROM_ELF: &'static str = "esp32s3_rev0_rom.elf";
    const CPU_HZ: u64 = periph::CPU_HZ;
    const CORES: usize = 2;
    const IDLE_CHUNK: u64 = 64 * 8;
    const ROM_DATA_TABLE: &'static [&'static str] = &["_data_start"];
    fn new_core(i: usize) -> Cpu { Cpu::new(if i == 0 { 0xCDCD } else { 0xABAB }) }
    fn reset_core(c: &mut Cpu, i: usize) { Cpu::reset(c); if i == 1 { c.prid = 0xABAB; } }
    fn boot_core(c: &mut Cpu, entry: u32) {
        Cpu::reset(c);
        c.pc = entry;
        c.ps = ps::WOE | ps::UM;      // windows enabled, user vector; INTLEVEL 0
        c.vecbase = 0x4000_0000;
        c.set_ar(1, 0x3FCE_B000);     // bootloader stack (in DRAM, app treats as free)
        c.set_ar(0, 0);
    }
    fn irqs(bus: &SocBus, out: &mut [u32]) { let (l0, l1) = bus.periph.cpu_lines_both(); out[0] = l0; out[1] = l1; }
    /// Core 0 is always running. SYSTEM_CORE_1_CONTROL_0 controls core 1's clock gate, reset,
    /// and run-stall state.
    fn core_state(bus: &SocBus, core: usize) -> CoreState {
        if core == 0 { return CoreState::Running; }
        let (clk, reset, stall) = bus.periph.core1_control();
        if reset { CoreState::Reset } else if clk && !stall { CoreState::Running } else { CoreState::Held }
    }
}

impl esp_soc::SocBus for SocBus {
    fn cycles(&self) -> u64 { self.cycles }
    fn next_deadline(&self) -> Option<u64> { Some(SocBus::next_deadline(self)) }
    fn irq_dirty(&mut self) -> &mut bool { &mut self.irq_dirty }
    fn refresh_irq(&mut self) -> bool {
        let dirty = self.periph.lines_dirty() || self.periph.intmatrix_dirty;
        self.periph.intmatrix_dirty = false;
        dirty
    }
    fn flush_ticks(&mut self) { SocBus::flush_ticks(self) }
    fn touch_input(&mut self, x: u16, y: u16, down: bool) {
        self.board.touch_at(self.cycles, x, y, down);
        if self.board.next_deadline().is_some() { self.refresh_tick_budget(); }
    }
    fn misc(&mut self) -> &mut Misc { &mut self.periph.misc }
    fn load_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), String> { SocBus::load_bytes(self, addr, data) }
    fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> {
        if offset + data.len() > self.flash.len() { return Err("flash image too large".into()); }
        self.flash[offset..offset + data.len()].copy_from_slice(data);
        self.note_written(SRC_FLASH, offset, data.len());
        Ok(())
    }
    fn persistent_flash_loaded(&self) -> bool { SocBus::persistent_flash_loaded(self) }
    fn initialize_storage(&mut self) -> Result<(), String> { SocBus::initialize_storage(self) }
    fn flush_storage(&mut self) -> Result<(), String> { SocBus::flush_storage(self) }
    fn storage_generation(&self) -> u64 { SocBus::storage_generation(self) }
    fn export_storage(&self, kind: u32) -> Option<Vec<u8>> { SocBus::export_storage(self, kind) }
    fn import_storage(&mut self, kind: u32, data: &[u8]) -> Result<(), String> { SocBus::import_storage(self, kind, data) }
    /// Copy IRAM/DRAM segments, map IROM/DROM through the MMU, as the 2nd-stage bootloader would.
    fn boot_app(&mut self, app_off: usize) -> Result<u32, String> {
        self.periph.system.preset_after_bootloader();
        self.periph.rtc.preset_after_bootloader();
        let img = esp_soc::image::parse(&self.flash[app_off..])?;
        for s in &img.segments {
            let start = app_off + s.file_off as usize;
            let end = start + s.len as usize;
            if end > self.flash.len() { return Err("segment beyond flash".into()); }
            let flash_mapped = (DBUS_LOW..DBUS_HIGH).contains(&s.load_addr) || (IBUS_LOW..IBUS_HIGH).contains(&s.load_addr);
            if flash_mapped {
                // esptool aligns segments so vaddr and flash offset agree modulo 64 KiB
                if (s.load_addr & 0xffff) != (start as u32 & 0xffff) { return Err(format!("segment {:#x} not page-aligned with flash offset {:#x}", s.load_addr, start)); }
                let first_page = (start as u32) >> 16;
                let npages = ((s.load_addr & 0xffff) + s.len + 0xffff) >> 16;
                for i in 0..npages {
                    let vpage = (((s.load_addr & 0x1FF_FFFF) >> 16) + i) as usize;
                    self.mmu[vpage] = first_page + i;
                }
                self.invalidate_tlb();
            } else {
                let data = self.flash[start..end].to_vec();
                SocBus::load_bytes(self, s.load_addr, &data)?;
            }
        }
        Ok(img.entry)
    }
    /// Digital peripherals re-initialised, cache MMU invalid; SRAM, RTC memories, efuses and the
    /// RTC-domain registers survive, as on silicon. Returns the cause the ROM will report.
    fn reboot(&mut self, mac: [u8; 6]) -> u32 {
        self.flush_ticks();
        let cause = self.periph.rtc.reset_cause;
        let old = std::mem::replace(&mut self.periph, periph::Peripherals::new(mac));
        let p = &mut self.periph;
        p.efuse = old.efuse;
        p.efuse.reset_controller();
        p.gpio.strap = old.gpio.strap;
        p.misc.log_unknown = old.misc.log_unknown; p.spi1.log = old.spi1.log;
        p.spi0.jedec = old.spi0.jedec; p.spi1.jedec = old.spi1.jedec;   // the flash chip is not reset: its ID keeps the --flash-mb capacity
        p.rtc.ram = old.rtc.ram; p.rtc.slow_ticks = old.rtc.slow_ticks; p.rtc.ulp = old.rtc.ulp;
        p.rtc.ulp_adc = old.rtc.ulp_adc; p.rtc.ulp_tsens = old.rtc.ulp_tsens; p.rtc.ulp_i2c = old.rtc.ulp_i2c;
        p.rtc.ulp_adc_sample_cycle = old.rtc.ulp_adc_sample_cycle; p.rtc.ulp_adc_sample_bits = old.rtc.ulp_adc_sample_bits;
        p.rtc.ram.write(0x38, cause | (cause << 6));
        p.rtc.ram.write(0x98, 0);                       // watchdog disarmed by the reset; the ROM re-arms it
        p.i2s0.pcm = old.i2s0.pcm; p.i2s0.frames_out = old.i2s0.frames_out; p.i2s1.pcm = old.i2s1.pcm; p.i2s1.frames_out = old.i2s1.frames_out;   // keep the captured audio continuous
        self.mmu = [MMU_INVALID; MMU_ENTRIES];
        self.invalidate_tlb();
        self.attach_board_devices();
        self.refresh_tick_budget();
        self.irq_dirty = true;
        cause
    }
    fn sw_reset(&self) -> bool { self.periph.rtc.sw_reset }
    fn reset_cause(&self) -> u32 { self.periph.rtc.reset_cause }
    fn last_fault(&self) -> Option<(u32, bool)> { self.last_fault }
    fn console_take(&mut self) -> [Vec<u8>; 4] {
        [std::mem::take(&mut self.periph.usb.tx_out), std::mem::take(&mut self.periph.uart[0].tx_out), std::mem::take(&mut self.periph.uart[1].tx_out), std::mem::take(&mut self.periph.uart[2].tx_out)]
    }
    fn serial_input(&mut self, data: &[u8]) {
        let before = self.periph.usb.irq();
        self.periph.usb.host_input(data);
        self.irq_dirty |= before != self.periph.usb.irq();
    }
    fn uart_input(&mut self, n: usize, data: &[u8]) {
        let Some(u) = self.periph.uart.get_mut(n) else { return };
        let before = u.irq();
        u.host_input(data);
        self.irq_dirty |= before != u.irq();
    }
    fn uart_rx_capacity(&self, n: usize) -> usize { self.periph.uart.get(n).map_or(0, |uart| uart.rx_capacity()) }
    fn gpio_set_input(&mut self, pin: u8, level: bool) {
        let old_input = self.periph.gpio.input;
        self.periph.gpio.set_input(pin, level);
        self.irq_dirty |= old_input != self.periph.gpio.input;
        if let Some(ev) = &mut self.gpio_events { ev.push((self.cycles, pin, level)); }
    }
    fn set_flash_size(&mut self, bytes: usize) {
        self.flash = vec![0xff; bytes];
        let cap = bytes.trailing_zeros() as u8; self.periph.spi1.jedec[2] = cap; self.periph.spi0.jedec[2] = cap;
        self.rebuild_page_table();
    }
    fn set_psram_size(&mut self, bytes: usize) -> Result<(), String> { self.psram = vec![0; bytes]; self.rebuild_page_table(); Ok(()) }
    fn set_strap(&mut self, v: u32) { self.periph.gpio.strap = v; }
    fn set_reset_cause(&mut self, c: u32) { self.periph.rtc.ram.write(0x38, c | (c << 6)); }
    fn report(&self) -> String {
        let p = &self.periph;
        let mut s = format!("[emu] i2s frames out: {} (i2s0 @ {} Hz) {} (i2s1 @ {} Hz)\n", p.i2s0.frames_out, p.i2s0.sample_rate, p.i2s1.frames_out, p.i2s1.sample_rate);
        { let r = self.board.report(); if !r.is_empty() { s += &r; s += "\n"; } }
        { let w = &p.wifi;
          if w.tx_frames + w.rx_frames > 0 { s += &format!("[emu] wifi: {} frames sent by the station, {} received ({} dropped: no descriptor){}\n", w.tx_frames, w.rx_frames, w.rx_dropped, w.ap.as_ref().map_or(String::new(), |ap| format!("; AP: {} beacons, {} probe responses, {} data frames from the station, state {:?}", ap.stats.0, ap.stats.1, ap.stats.2, ap.state))); }
          if let Some(n) = &w.net { s += &format!("[emu] net: {} DHCP leases, {} ARP replies, {} DNS answers, {} NTP answers, {} TCP refused, {} pings, {} frames ignored\n", n.dhcp_acks, n.arp_replies, n.dns_answers, n.ntp_answers, n.tcp_rejects, n.pings, n.unhandled);
            if let Some(t) = &n.nat { s += &format!("[emu] nat: {} TCP connections ({} failed), {} UDP flows, {} bytes out, {} bytes in\n", t.tcp_opened, t.tcp_refused, t.udp_flows, t.bytes_to_host, t.bytes_to_guest); } } }
        { let (a, sh, r) = (&p.aes, &p.sha, &p.rsa);
          if a.blocks + sh.blocks + r.ops > 0 { s += &format!("[emu] crypto: {} AES blocks, {} SHA blocks, {} RSA/MPI operations\n", a.blocks, sh.blocks, r.ops); } }
        if let Some(r) = p.rtc.ulp.report() { s += &r; s += "\n"; }
        if self.ulp_fsm.cpu.insn_count + self.ulp_fsm.traps > 0 { s += &format!("[emu] ulp-fsm: {} instructions, {} cycles, {} traps, {} wake requests\n", self.ulp_fsm.cpu.insn_count, self.ulp_fsm.cpu.cycle_count, self.ulp_fsm.traps, self.ulp_fsm.wake_requests); }
        if self.ulp_riscv.cpu.insn_count + self.ulp_riscv.traps > 0 { s += &format!("[emu] ulp-riscv: {} instructions, {} cycles, {} traps, {} wake requests\n", self.ulp_riscv.cpu.insn_count, self.ulp_riscv.cpu.cycle_count, self.ulp_riscv.traps, self.ulp_riscv.wake_requests); }
        if p.lcd_cam.lcd_frames > 0 { s += &format!("[emu] lcd: {} RGB frames\n", p.lcd_cam.lcd_frames); }
        if p.lcd_cam.frames + p.lcd_cam.dropped > 0 { s += &format!("[emu] camera: {} frames delivered, {} dropped (no DMA/no picture)\n", p.lcd_cam.frames, p.lcd_cam.dropped); }
        if p.rmt.tx_count > 0 { s += &format!("[emu] rmt tx {}\n", p.rmt.tx_count); }
        s.trim_end().to_string()
    }
    fn set_debug(&mut self, f: &esp_soc::DebugFlags) {
        self.debug = f.clone();
        for area in f.iter() { esp_periph::Dispatch::debug(&mut self.periph, area, true); }
        if f.has("ulp") { self.periph.rtc.ulp.debug(true); }
        self.periph.misc.log_all = f.has("mmio");
    }
    fn observe_gpio(&mut self, on: bool) { self.gpio_events = if on { Some(Vec::new()) } else { None }; }
    fn take_gpio_events(&mut self) -> Vec<(u64, u8, bool)> { self.gpio_events.as_mut().map(std::mem::take).unwrap_or_default() }
    fn gpio_input(&self) -> u64 { self.periph.gpio.input }
    fn board(&mut self) -> &mut dyn BoardModel { &mut *self.board }
    fn board_ref(&self) -> &dyn BoardModel { &*self.board }
    fn audio(&self) -> (&[i16], u32) {
        if let Some(audio) = self.board.audio() { return audio; }
        let a = self.periph.audio(); (&a.pcm, a.sample_rate)
    }
    fn camera_frames(&self) -> u64 { self.periph.lcd_cam.frames }
    fn irq_sources_of(&self, core: usize, line: u32) -> Vec<usize> { (0..NUM_SOURCES).filter(|&s| self.periph.intmatrix.map[core][s] == line).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_core_is_not_controlled_by_core_one_reset_registers() {
        let bus = SocBus::new(8 << 20, 2 << 20, [0; 6]);
        assert_eq!(S3::core_state(&bus, 0), CoreState::Running);
        assert_eq!(S3::core_state(&bus, 1), CoreState::Held);
    }
}
