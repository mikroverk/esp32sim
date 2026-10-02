//! The ESP32-C3 as a `Soc`: one RV32IMC core on `bus::SocBus`, a bare module (no board).
use crate::bus::{SocBus, DBUS_HIGH, DBUS_LOW, IBUS_HIGH, IBUS_LOW, MMU_ENTRIES, MMU_INVALID};
use crate::periph::{self, src};
use esp_periph::Misc;
use esp_soc::{BoardModel, Soc};
use riscv_rv32::Cpu;

pub struct C3;
pub type Machine = esp_soc::Machine<C3>;

pub fn machine(mac: [u8; 6], flash_size: usize) -> Machine { let mut m = Machine::new(mac, SocBus::new(flash_size, mac)); m.set_debug(&esp_soc::DebugFlags::from_env()); m }

impl Soc for C3 {
    type Core = Cpu;
    type Bus = SocBus;
    const NAME: &'static str = "esp32c3";
    const ROM_ELF: &'static str = "esp32c3_rev3_rom.elf";
    const CPU_HZ: u64 = periph::CPU_HZ;
    const CORES: usize = 1;
    const IDLE_CHUNK: u64 = 64;
    const ROM_DATA_TABLE: &'static [&'static str] = &["_data_end_btdm_rom", "_data_start"];
    fn new_core(_i: usize) -> Cpu { Cpu::new() }
    fn reset_core(c: &mut Cpu, _i: usize) { Cpu::reset(c); }
    fn boot_core(c: &mut Cpu, entry: u32) {
        Cpu::reset(c);
        c.pc = entry;
        c.x[2] = 0x3FCD_E000;                 // a stack the bootloader would have left us
    }
    fn irqs(bus: &SocBus, out: &mut [Option<u32>]) { out[0] = bus.periph.intc.lines.pending(); }
}

impl esp_soc::SocBus for SocBus {
    fn cycles(&self) -> u64 { self.cycles }
    fn next_deadline(&self) -> Option<u64> {
        match self.periph.cycles_until_timer() { u32::MAX => None, cycles => Some(cycles.max(1) as u64) }
    }
    fn irq_dirty(&mut self) -> &mut bool { &mut self.irq_dirty }
    fn refresh_irq(&mut self) -> bool { self.periph.refresh_lines(); true }
    fn misc(&mut self) -> &mut Misc { &mut self.periph.misc }
    fn load_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), String> { SocBus::load_bytes(self, addr, data) }
    fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> { SocBus::write_flash(self, offset, data) }
    /// Copy the RAM segments, map the flash-resident ones through the MMU, as the 2nd-stage bootloader would.
    fn boot_app(&mut self, app_off: usize) -> Result<u32, String> {
        let image = self.flash.get(app_off..).ok_or("app offset beyond flash")?;
        let img = esp_soc::image::parse(image)?;
        self.periph.system.preset_after_bootloader();
        self.periph.rtc.preset_after_bootloader();
        for s in &img.segments {
            let start = app_off + s.file_off as usize;
            let end = start + s.len as usize;
            if end > self.flash.len() { return Err("segment beyond flash".into()); }
            let mapped = (DBUS_LOW..DBUS_HIGH).contains(&s.load_addr) || (IBUS_LOW..IBUS_HIGH).contains(&s.load_addr);
            if mapped {
                // The C3 has ONE 128-entry table for both buses, and software keeps the DROM and
                // IROM page ranges disjoint (`CACHE_DROM_MMU_START = CACHE_IROM_MMU_END`). Naively
                // indexing by `vaddr & 0x7FFFFF` makes 0x3C01_0000 and 0x4201_0000 collide, so a
                // direct app boot needs the bootloader's split, which we do not model yet.
                if (DBUS_LOW..DBUS_HIGH).contains(&s.load_addr) {
                    return Err("--boot app is not supported on the C3 yet: its DROM and IROM share \
                                one MMU table and need the bootloader's page split. Boot from the \
                                mask ROM instead (--boot rom with --bootloader/--ptable/--app)".into());
                }
                if (s.load_addr & 0xffff) != (start as u32 & 0xffff) {
                    return Err(format!("segment {:#x} not page-aligned with flash offset {:#x}", s.load_addr, start));
                }
                let first_page = (start as u32) >> 16;
                if !s.load_addr.checked_add(s.len).is_some_and(|end| end <= IBUS_HIGH) {
                    return Err("segment beyond the flash window".into());
                }
                let npages = ((s.load_addr & 0xffff) + s.len + 0xffff) >> 16;
                let first_entry = ((s.load_addr & 0x7F_FFFF) >> 16) as usize;
                for i in 0..npages {
                    self.mmu[first_entry + i as usize] = first_page + i;
                }
            } else {
                let data = self.flash[start..end].to_vec();
                SocBus::load_bytes(self, s.load_addr, &data)?;
            }
        }
        Ok(img.entry)
    }
    /// Digital peripherals re-created, SRAM kept.
    fn reboot(&mut self, mac: [u8; 6]) -> u32 {
        let cause = self.periph.rtc.reset_cause;
        let old = std::mem::replace(&mut self.periph, periph::Peripherals::new(mac));
        let p = &mut self.periph;
        p.efuse = old.efuse;
        p.misc.log_unknown = old.misc.log_unknown;
        p.usb.connected = old.usb.connected;
        p.rtc.reset_cause = cause;
        // The flash chip is on the board, not in the chip: its JEDEC capacity survives a reset.
        // (Real silicon reported 4 MB on every boot; without this the emulator re-detected the
        // default 8 MB from the second boot onward.)
        p.spi0.jedec = old.spi0.jedec;
        p.spi1.jedec = old.spi1.jedec;
        p.gpio.strap = old.gpio.strap;      // strapping pins are board wiring, not chip state
        // Publish the cause where the ROM reads it, so the boot banner says RTC_SW_CPU_RST like
        // real silicon rather than POWERON.
        p.rtc.ram.write(0x38, cause | (cause << 6));
        self.mmu = [MMU_INVALID; MMU_ENTRIES];
        cause
    }
    fn sw_reset(&self) -> bool { self.periph.rtc.sw_reset }
    fn request_reset(&mut self, cause: u32) { self.periph.rtc.sw_reset = true; self.periph.rtc.reset_cause = cause; }
    fn reset_cause(&self) -> u32 { self.periph.rtc.reset_cause }
    fn last_fault(&self) -> Option<(u32, bool)> { self.last_fault }
    fn console_take(&mut self) -> [Vec<u8>; 4] { [std::mem::take(&mut self.periph.usb.tx_out), std::mem::take(&mut self.periph.uart[0].tx_out), std::mem::take(&mut self.periph.uart[1].tx_out), Vec::new()] }
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
    fn gpio_set_input(&mut self, pin: u8, level: bool) {
        let before = self.periph.gpio.input;
        self.periph.gpio.set_input(pin, level);
        self.irq_dirty |= before != self.periph.gpio.input;
        if let Some(ev) = &mut self.gpio_events { ev.push((self.cycles, pin, level)); }
    }
    fn set_flash_size(&mut self, bytes: usize) {
        self.flash = vec![0xff; bytes];
        let cap = bytes.trailing_zeros() as u8; self.periph.spi1.jedec[2] = cap; self.periph.spi0.jedec[2] = cap;
    }
    fn set_strap(&mut self, v: u32) { self.periph.gpio.strap = v; }
    fn set_reset_cause(&mut self, c: u32) { self.periph.rtc.ram.write(0x38, c | (c << 6)); self.periph.rtc.reset_cause = c; }
    fn set_debug(&mut self, f: &esp_soc::DebugFlags) {
        self.debug = f.clone();
        for area in f.iter() { esp_periph::Dispatch::debug(&mut self.periph, area, true); }
        self.periph.misc.log_all = f.has("mmio");
    }
    fn observe_gpio(&mut self, on: bool) { self.gpio_events = if on { Some(Vec::new()) } else { None }; }
    fn take_gpio_events(&mut self) -> Vec<(u64, u8, bool)> { self.gpio_events.as_mut().map(std::mem::take).unwrap_or_default() }
    fn gpio_input(&self) -> u64 { self.periph.gpio.input }
    fn pwm_output(&self, pin: u32) -> Option<(f64, u32)> { self.periph.ledc.output(&self.periph.gpio, pin) }
    fn board(&mut self) -> &mut dyn BoardModel { &mut *self.board }
    fn board_ref(&self) -> &dyn BoardModel { &*self.board }
    fn audio(&self) -> (&[i16], u32) { (&[], 44100) }
    fn irq_sources_of(&self, _core: usize, line: u32) -> Vec<usize> { (0..src::COUNT).filter(|&s| self.periph.intc.map[s] == line).collect() }
}
