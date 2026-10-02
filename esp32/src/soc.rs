use crate::bus::{SocBus, DBUS_HIGH, DBUS_LOW, IBUS_HIGH, IBUS_LOW};
use crate::periph::{self, Peripherals};
use esp_periph::Misc;
use esp_soc::{BoardModel, CoreState, Soc};
use xtensa_lx7::state::ps;
use xtensa_lx7::Cpu;

pub struct Esp32;
pub type Machine = esp_soc::Machine<Esp32>;
pub fn machine(mac: [u8; 6], flash_size: usize) -> Machine {
    let mut m = Machine::new(mac, SocBus::new(flash_size, mac));
    m.set_debug(&esp_soc::DebugFlags::from_env());
    m
}

fn core(i: usize) -> Cpu {
    let mut c = Cpu::new(if i == 0 { 0xcdcd } else { 0xabab });
    c.configid = [0xC2BC_FFFE, 0x1CC5_FE96];
    c
}
impl Soc for Esp32 {
    type Core = Cpu;
    type Bus = SocBus;
    const NAME: &'static str = "esp32";
    const ROM_ELF: &'static str = "esp32_rev300_rom.elf";
    const CPU_HZ: u64 = periph::CPU_HZ;
    const CORES: usize = 2;
    const IDLE_CHUNK: u64 = 512;
    const ROM_DATA_TABLE: &'static [&'static str] = &["_data_start"];
    fn new_core(i: usize) -> Cpu {
        core(i)
    }
    fn reset_core(c: &mut Cpu, i: usize) {
        c.reset();
        c.prid = if i == 0 { 0xcdcd } else { 0xabab };
        c.configid = [0xC2BC_FFFE, 0x1CC5_FE96];
    }
    fn boot_core(c: &mut Cpu, entry: u32) {
        c.reset();
        c.pc = entry;
        c.ps = ps::WOE | ps::UM;
        c.vecbase = 0x4000_0000;
        c.set_ar(1, 0x3ffe_0000);
    }
    fn irqs(bus: &SocBus, out: &mut [u32]) {
        for (core, lines) in out.iter_mut().enumerate() {
            *lines = bus.periph.cpu_lines(core);
        }
    }
    fn core_state(bus: &SocBus, core: usize) -> CoreState {
        if core == 0 {
            CoreState::Running
        } else {
            let (clock, reset, stall) = bus.periph.dport.core1_control();
            if reset { CoreState::Reset } else if clock && !stall { CoreState::Running } else { CoreState::Held }
        }
    }
}

impl esp_soc::SocBus for SocBus {
    fn cycles(&self) -> u64 {
        self.cycles
    }
    fn next_deadline(&self) -> Option<u64> {
        match self.periph.cycles_until_timer() {
            u32::MAX => None,
            n => Some(n.max(1) as u64),
        }
    }
    fn irq_dirty(&mut self) -> &mut bool {
        &mut self.irq_dirty
    }
    fn refresh_irq(&mut self) -> bool {
        true
    }
    fn misc(&mut self) -> &mut Misc {
        &mut self.periph.misc
    }
    fn load_bytes(&mut self, addr: u32, data: &[u8]) -> Result<(), String> {
        SocBus::load_bytes(self, addr, data)
    }
    fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> {
        SocBus::write_flash(self, offset, data)
    }
    fn boot_app(&mut self, app_off: usize) -> Result<u32, String> {
        let img =
            esp_soc::image::parse(self.flash.get(app_off..).ok_or("app offset beyond flash")?)?;
        for s in &img.segments {
            let start = app_off + s.file_off as usize;
            let end = start + s.len as usize;
            if end > self.flash.len() {
                return Err("segment beyond flash".into());
            }
            if (DBUS_LOW..DBUS_HIGH).contains(&s.load_addr)
                || (IBUS_LOW..IBUS_HIGH).contains(&s.load_addr)
            {
                let table = if s.load_addr < DBUS_HIGH { 0 } else { 64 };
                let origin = if table == 0 { DBUS_LOW } else { 0x4000_0000 };
                let first = (start as u32) >> 16;
                let pages = ((s.load_addr & 0xffff) + s.len + 0xffff) >> 16;
                for i in 0..pages {
                    self.mmu[0][table + ((s.load_addr - origin) >> 16) as usize + i as usize] =
                        first + i;
                }
            } else {
                let data = self.flash[start..end].to_vec();
                self.load_bytes(s.load_addr, &data)?;
            }
        }
        Ok(img.entry)
    }
    fn reboot(&mut self, mac: [u8; 6]) -> u32 {
        let cause = self.periph.rtc.0.reset_cause;
        let old = std::mem::replace(&mut self.periph, Peripherals::new(mac));
        self.periph.efuse = old.efuse;
        self.periph.misc.log_unknown = old.misc.log_unknown;
        self.periph.gpio.gpio.strap = old.gpio.gpio.strap;
        self.periph.rtc.0.reset_cause = cause;
        cause
    }
    fn sw_reset(&self) -> bool {
        self.periph.rtc.0.sw_reset
    }
    fn request_reset(&mut self, cause: u32) {
        self.periph.rtc.0.sw_reset = true;
        self.periph.rtc.0.reset_cause = cause;
    }
    fn reset_cause(&self) -> u32 {
        self.periph.rtc.0.reset_cause
    }
    fn last_fault(&self) -> Option<(u32, bool)> {
        self.last_fault
    }
    fn console_take(&mut self) -> [Vec<u8>; 4] {
        [
            Vec::new(),
            std::mem::take(&mut self.periph.uart[0].tx_out),
            std::mem::take(&mut self.periph.uart[1].tx_out),
            std::mem::take(&mut self.periph.uart[2].tx_out),
        ]
    }
    fn serial_input(&mut self, data: &[u8]) {
        self.periph.uart[0].host_input(data);
        self.irq_dirty = true;
    }
    fn uart_input(&mut self, n: usize, data: &[u8]) {
        if let Some(u) = self.periph.uart.get_mut(n) {
            u.host_input(data);
            self.irq_dirty = true;
        }
    }
    fn gpio_set_input(&mut self, pin: u8, level: bool) {
        self.periph.gpio.set_input(pin, level);
        self.irq_dirty = true;
        if let Some(ev) = &mut self.gpio_events {
            ev.push((self.cycles, pin, level));
        }
    }
    fn gpio_input(&self) -> u64 {
        self.periph.gpio.gpio.input
    }
    fn observe_gpio(&mut self, on: bool) {
        self.gpio_events = on.then(Vec::new);
    }
    fn take_gpio_events(&mut self) -> Vec<(u64, u8, bool)> {
        self.gpio_events
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }
    fn board(&mut self) -> &mut dyn BoardModel {
        &mut *self.board
    }
    fn board_ref(&self) -> &dyn BoardModel {
        &*self.board
    }
    fn audio(&self) -> (&[i16], u32) {
        (&[], 44_100)
    }
    fn irq_sources_of(&self, core: usize, line: u32) -> Vec<usize> {
        (0..periph::NUM_SOURCES)
            .filter(|&source| self.periph.dport.map[core][source] == line)
            .collect()
    }
    fn set_debug(&mut self, f: &esp_soc::DebugFlags) {
        self.debug = f.clone();
        for area in f.iter() {
            esp_periph::Dispatch::debug(&mut self.periph, area, true);
        }
        self.periph.misc.log_all = f.has("mmio");
    }
    fn set_flash_size(&mut self, bytes: usize) {
        self.flash = vec![0xff; bytes];
        let cap = bytes.trailing_zeros() as u8;
        self.periph.spi0.0.jedec[2] = cap;
        self.periph.spi1.0.jedec[2] = cap;
    }
    fn set_strap(&mut self, v: u32) {
        self.periph.gpio.gpio.strap = v;
    }
    fn set_reset_cause(&mut self, cause: u32) {
        self.periph.rtc.0.ram.write(0x38, cause | cause << 6);
        self.periph.rtc.0.reset_cause = cause;
    }
}
