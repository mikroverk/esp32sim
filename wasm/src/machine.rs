//! Shared machine operations and explicit chip dispatch for the browser ABI.
use super::log;
use esp_soc::observers::{BlockProfile, Coverage, IrqLatency};
use esp_soc::web::{json_escape, WebServer};
use esp_soc::{Machine, Soc, SocBus, Stop};

/// The chip-neutral operations the ABI uses.
pub(super) trait MachineApi {
    fn bus(&mut self) -> &mut dyn SocBus;
    fn load(&mut self, kind: u32, d: &[u8]) -> Result<(), String>;
    fn write_flash(&mut self, off: usize, d: &[u8]) -> Result<(), String>;
    fn boot(&mut self, app_direct: bool) -> Result<(), String>;
    fn board_name(&self) -> String;
    fn web(&self) -> Option<&WebServer>;
    fn run_slice(&mut self, cycles: u32) -> u32;
    fn cpu_hz(&self) -> f64;
    fn cycles(&self) -> f64;
    fn insns(&self) -> f64;
    fn stub(&mut self, name: &str, value: u32) -> u32;
    fn observer(&mut self, name: &str, arg: &str) -> u32;
    fn reports(&mut self) -> String;
    fn set_jit(&mut self, enabled: bool);
    fn approximate_jit_timing(&mut self, cpi: u32, quantum: u32) -> u32;
}

impl<S: Soc> MachineApi for Machine<S> {
    fn bus(&mut self) -> &mut dyn SocBus { &mut self.bus }
    fn load(&mut self, kind: u32, d: &[u8]) -> Result<(), String> {
        self.load_input(esp_soc::LoadKind::try_from(kind)?, d)
    }
    fn write_flash(&mut self, off: usize, d: &[u8]) -> Result<(), String> { Machine::write_flash(self, off, d) }
    fn boot(&mut self, app_direct: bool) -> Result<(), String> { if app_direct { self.boot_app(0x10000).map(|_| ()) } else { self.boot_rom(); Ok(()) } }
    fn board_name(&self) -> String { self.bus.board_ref().name().to_string() }
    fn web(&self) -> Option<&WebServer> { self.web.as_ref() }
    fn run_slice(&mut self, cycles: u32) -> u32 {
        self.max_cycles = self.bus.cycles() + cycles as u64;
        loop {
            match self.run(u64::MAX) {
                Stop::Halted | Stop::MaxInsns => return 0,
                Stop::SwReset => reboot(self),
                Stop::Unimplemented(pc, raw) => { log(&format!("[emu] unimplemented instruction at {:08x} {} (raw {:#x})", pc, self.sym(pc), raw)); return 2; }
                Stop::Ebreak(pc) => { log(&format!("[emu] ebreak at {:08x} {}", pc, self.sym(pc))); return 3; }
                Stop::Breakpoint(_) => return 3,
                Stop::Exceptions(_) => return 4,
                Stop::Simcall(_) => return 5,
                Stop::Watch(..) => return 6,
                Stop::CostModel { reason, .. } | Stop::CostModelLifecycle { reason, .. } => { log(&format!("[emu] cost model: {}", reason)); return 7; }
            }
        }
    }
    fn cpu_hz(&self) -> f64 { S::CPU_HZ as f64 }
    fn cycles(&self) -> f64 { self.bus.cycles() as f64 }
    fn insns(&self) -> f64 { Machine::insns(self) as f64 }
    fn stub(&mut self, name: &str, value: u32) -> u32 {
        match self.resolve_stub(name) {
            Some(addr) => { self.stubs.insert(addr, value); log(&format!("[emu] stub {} @ {:#x} -> returns {:#x}", name, addr, value)); 0 }
            None => { log(&format!("[emu] stub: no symbol '{}' (load the app ELF first)", name)); 1 }
        }
    }
    fn observer(&mut self, name: &str, arg: &str) -> u32 {
        match name {
            "profile-blocks" => { self.add_observer(Box::new(BlockProfile::new(20))); 0 }
            "coverage" => { self.add_observer(Box::new(Coverage::new(None))); 0 }
            "irq-latency" => { self.add_observer(Box::new(IrqLatency::new(S::CORES))); 0 }
            "trace-fn" => { self.trace_fns(arg); 0 }
            _ => { log(&format!("[emu] unknown observer '{}'", name)); 1 }
        }
    }
    fn reports(&mut self) -> String { Machine::reports(self) }
    fn approximate_jit_timing(&mut self, cpi: u32, quantum: u32) -> u32 {
        match self.set_approximate_jit_timing(cpi, quantum) { Ok(()) => 0, Err(reason) => { log(&reason); 1 } }
    }
    fn set_jit(&mut self, enabled: bool) { for core in &mut self.cores { xtensa_lx7::Core::set_jit(core, enabled); } }
}

/// Chip-specific ABI calls borrow an S3 directly; no runtime type erasure or downcasts.
pub(super) enum MachineKind {
    S3(Box<esp32s3::Machine>),
    C3(Box<esp32c3::Machine>),
    C6(Box<esp32c6::Machine>),
}

impl MachineKind {
    pub fn s3_mut(&mut self) -> Option<&mut esp32s3::Machine> {
        match self { Self::S3(m) => Some(m), _ => None }
    }
}

impl std::ops::Deref for MachineKind {
    type Target = dyn MachineApi;
    fn deref(&self) -> &Self::Target {
        match self { Self::S3(m) => m.as_ref(), Self::C3(m) => m.as_ref(), Self::C6(m) => m.as_ref() }
    }
}

impl std::ops::DerefMut for MachineKind {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self { Self::S3(m) => m.as_mut(), Self::C3(m) => m.as_mut(), Self::C6(m) => m.as_mut() }
    }
}

/// Report software reset once for both ordinary slices and receipt-priced sidecar commits.
pub(super) fn reboot<S: Soc>(machine: &mut Machine<S>) {
    let cause = machine.bus.reset_cause();
    let note = format!("[emu] chip reset at t={:.3}s: cause {:#x} ({})",
        machine.seconds(), cause, esp_periph::reset_cause_name(cause));
    log(&note);
    if let Some(web) = &machine.web {
        web.send_text(&format!("{{\"t\":\"emu\",\"msg\":\"{}\"}}", json_escape(&note)));
    }
    machine.reboot();
}
