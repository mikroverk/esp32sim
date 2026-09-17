//! `Machine<S>`: the cores and the bus of a `Soc`, scheduled together with device time, plus
//! everything around a run that is the same for every chip — console capture, action scripts,
//! function stubs and probes, tracing and watchpoints, the web UI protocol, real-time pacing,
//! image loading, reboot.
use crate::observe::{Ctx, Observer, Wants};
use crate::soc::{CoreState, RunUntil, Soc, SocBus, Stop};
use crate::web::WebServer;
use crate::{elf, png};
use emu_core::core::pc_bit;
use emu_core::{
    Bus, Core, CostModel, ExecutionFacts, Fault, LifecycleFacts, LifecycleKind,
    MemoryAccess, MemoryAccessKind, StepKind, Trap,
};
use std::collections::{BTreeMap, HashMap};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::VecDeque;

/// `[[r,g,b],…]` for the web protocol.
fn leds_json(leds: &[[u8; 3]]) -> String {
    leds.iter().map(|c| format!("[{},{},{}]", c[0], c[1], c[2])).collect::<Vec<_>>().join(",")
}

#[derive(Clone, Debug)]
pub enum ScriptAction { Gpio(u8, bool), Serial(String), Uart(usize, String), Stop, Touch(u16, u16, bool), Poke(u32, u32) }

/// The stop conditions that are not observers.
pub struct Debug { pub stop_on_unimplemented: bool, pub stop_after_exceptions: u64 }

/// Guest console output: everything ever printed (`all`), the per-source backlogs the web UI
/// replays to a late client, and what goes to stdout.
pub struct Console {
    pub all: Vec<u8>,
    pub usb: Vec<u8>,
    pub uart0: Vec<u8>,
    /// which consoles to mirror to stdout: bit0 = USB-CDC, bit1 = UART0, bit2 = UART1/2
    pub mask: u32,
    pub prefix: bool,
    /// keep the text instead of printing it (the WebAssembly build has no stdout worth writing to)
    pub capture: bool,
}

/// Host actions scheduled at emulated times (`--script`, and the UI's encoder detents).
pub struct Script { pub events: Vec<(u64, ScriptAction)>, pub pos: usize, pub log: bool, knob_next: u64 }

pub struct Realtime {
    pub enabled: bool,
    wall_start: Option<std::time::Instant>,
    last_check: u64,
    pub behind: f64,
    pub resyncs: u64,
    /// Emulated seconds per wall second over the last second or more; `None` until measured.
    /// Unlike `behind`, a resynchronisation does not reset it, so it shows a run that cannot keep up.
    pub speed: Option<f64>,
    speed_mark: Option<(std::time::Instant, u64)>,
    pub log: bool,
    log_last: Option<std::time::Instant>,
    log_insns: (u64, u64),
}

struct WebState { last_push_cycles: u64, audio_sent: usize, ring_updates: u64, grid_updates: Vec<u64>, px_pending: u64, px_sent: u64, px_deferred: bool, cam_pushed: u64, cam_sent: bool }

pub struct Machine<S: Soc> {
    pub mac: [u8; 6],
    pub reboots: u64,
    /// function stubs: at this pc (a function entry), return `value` immediately
    pub stubs: HashMap<u32, u32>,
    /// one bit per pc bucket for `stubs` / `fn_probes`, so the common case costs a shift and a test
    /// instead of hashing every pc (a hash lookup per instruction cost ~16% of run time)
    stub_bloom: u64, probe_bloom: u64,
    pub stub_hits: u64,
    /// function-entry tracing: pc -> name (`--trace-fn PREFIX`)
    pub fn_probes: HashMap<u32, String>,
    pub cores: Vec<S::Core>,
    /// a secondary core held in reset by its SoC registers (reset when released)
    core_held: Vec<bool>,
    pub bus: S::Bus,
    pub symbols: BTreeMap<u32, String>,
    pub dbg: Debug,
    /// analyses watching the run (`add_observer`); `probes` is the union of what they want
    pub observers: Vec<Box<dyn Observer<S>>>,
    probes: Wants,
    prev_irq: Vec<u32>,
    pub exceptions: u64,
    pub interrupts: u64,
    pub irq_hist: Vec<[u64; 32]>,
    pub script: Script,
    pub max_cycles: u64,
    pub console: Console,
    /// live web UI
    pub web: Option<WebServer>,
    /// Native raw TCP bridge attached to UART0.
    #[cfg(not(target_arch = "wasm32"))]
    pub uart_tcp: Option<crate::uart_tcp::UartTcp>,
    #[cfg(not(target_arch = "wasm32"))]
    uart_tcp_pending: VecDeque<u8>,
    #[cfg(not(target_arch = "wasm32"))]
    uart_tcp_next_rx: u64,
    ws: WebState,
    pub rt: Realtime,
    debug_rom: bool,
    cost: Option<Box<dyn CostModel>>,
    model_ready_at: Vec<u64>,
    model_stop: Option<Stop>,
    model_attach_error: Option<&'static str>,
}

const QUANTUM: u64 = 64;

/// Records only accesses made synchronously by `Core::step`. Generated direct-memory access is
/// disabled so every load and store passes through one of the typed methods below.
struct RecordingBus<'a, B> { bus: &'a mut B, accesses: Vec<MemoryAccess> }

impl<'a, B> RecordingBus<'a, B> {
    fn new(bus: &'a mut B) -> Self { Self { bus, accesses: Vec::new() } }

    fn finish(mut self, bytes: Option<[u8; 4]>, pc: u32) -> Vec<MemoryAccess> {
        if let Some(bytes) = bytes {
            self.accesses.retain(|access| access.kind != MemoryAccessKind::Fetch);
            self.accesses.insert(0, MemoryAccess {
                kind: MemoryAccessKind::Fetch, address: pc, width: 4,
                value: u32::from_le_bytes(bytes), fault: None,
            });
        }
        self.accesses
    }
}

impl<B: Bus> Bus for RecordingBus<'_, B> {
    fn read8(&mut self, address: u32) -> Result<u8, Fault> {
        let result = self.bus.read8(address);
        self.accesses.push(MemoryAccess { kind: MemoryAccessKind::Read, address, width: 1, value: result.unwrap_or(0) as u32, fault: result.err() });
        result
    }
    fn read16(&mut self, address: u32) -> Result<u16, Fault> {
        let result = self.bus.read16(address);
        self.accesses.push(MemoryAccess { kind: MemoryAccessKind::Read, address, width: 2, value: result.unwrap_or(0) as u32, fault: result.err() });
        result
    }
    fn read32(&mut self, address: u32) -> Result<u32, Fault> {
        let result = self.bus.read32(address);
        self.accesses.push(MemoryAccess { kind: MemoryAccessKind::Read, address, width: 4, value: result.unwrap_or(0), fault: result.err() });
        result
    }
    fn write8(&mut self, address: u32, value: u8) -> Result<(), Fault> {
        let result = self.bus.write8(address, value);
        self.accesses.push(MemoryAccess { kind: MemoryAccessKind::Write, address, width: 1, value: value as u32, fault: result.err() });
        result
    }
    fn write16(&mut self, address: u32, value: u16) -> Result<(), Fault> {
        let result = self.bus.write16(address, value);
        self.accesses.push(MemoryAccess { kind: MemoryAccessKind::Write, address, width: 2, value: value as u32, fault: result.err() });
        result
    }
    fn write32(&mut self, address: u32, value: u32) -> Result<(), Fault> {
        let result = self.bus.write32(address, value);
        self.accesses.push(MemoryAccess { kind: MemoryAccessKind::Write, address, width: 4, value, fault: result.err() });
        result
    }
    fn fetch(&mut self, pc: u32) -> Result<[u8; 4], Fault> {
        let result = self.bus.fetch(pc);
        self.accesses.push(MemoryAccess {
            kind: MemoryAccessKind::Fetch, address: pc, width: 4,
            value: result.map(u32::from_le_bytes).unwrap_or(0), fault: result.err(),
        });
        result
    }
    fn page_versions(&self) -> &[u32] { self.bus.page_versions() }
    fn code_page(&mut self, pc: u32) -> u32 { self.bus.code_page(pc) }
    fn note_pc(&mut self, pc: u32) { self.bus.note_pc(pc); }
    fn block_break(&self) -> bool { self.bus.block_break() }
    fn fast_mem(&mut self) -> Option<emu_core::bus::FastMem> { None }
    fn tick(&mut self, cycles: u32) -> u32 { self.bus.tick(cycles) }
}

impl<S: Soc> Machine<S> {
    pub fn new(mac: [u8; 6], bus: S::Bus) -> Self {
        Machine {
            mac, reboots: 0, stubs: HashMap::new(), stub_bloom: 0, probe_bloom: 0, stub_hits: 0, fn_probes: HashMap::new(),
            cores: (0..S::CORES).map(S::new_core).collect(), core_held: (0..S::CORES).map(|i| i > 0).collect(),
            bus, symbols: BTreeMap::new(),
            dbg: Debug { stop_on_unimplemented: true, stop_after_exceptions: u64::MAX },
            observers: Vec::new(), probes: Wants::NONE, prev_irq: vec![0; S::CORES],
            exceptions: 0, interrupts: 0, irq_hist: vec![[0; 32]; S::CORES],
            script: Script { events: Vec::new(), pos: 0, log: true, knob_next: 0 }, max_cycles: u64::MAX,
            console: Console { all: Vec::new(), usb: Vec::new(), uart0: Vec::new(), mask: 3, prefix: false, capture: false },
            web: None, ws: WebState { last_push_cycles: 0, audio_sent: 0, ring_updates: 0, grid_updates: Vec::new(), px_pending: 0, px_sent: 0, px_deferred: false, cam_pushed: u64::MAX, cam_sent: false },
            #[cfg(not(target_arch = "wasm32"))]
            uart_tcp: None,
            #[cfg(not(target_arch = "wasm32"))]
            uart_tcp_pending: VecDeque::new(),
            #[cfg(not(target_arch = "wasm32"))]
            uart_tcp_next_rx: 0,
            rt: Realtime { enabled: false, wall_start: None, last_check: 0, behind: 0.0, resyncs: 0, speed: None, speed_mark: None, log: false, log_last: None, log_insns: (0, 0) },
            debug_rom: false, cost: None, model_ready_at: vec![0; S::CORES], model_stop: None, model_attach_error: None,
        }
    }

    /// Which parts of the model print what they do (`--debug`, `ESP_EMU_DEBUG`).
    pub fn set_debug(&mut self, f: &crate::debug::DebugFlags) { self.rt.log = f.has("rt"); self.debug_rom = f.has("rom"); self.bus.set_debug(f); }
    pub fn seconds(&self) -> f64 { self.bus.cycles() as f64 / S::CPU_HZ as f64 }
    pub fn insns(&self) -> u64 { self.cores.iter().map(|c| c.insn_count()).sum() }

    // ------------------------------------------------------------------ observers
    pub fn add_observer(&mut self, o: Box<dyn Observer<S>>) {
        self.probes = self.probes | o.wants();
        self.observers.push(o);
        self.bus.misc().mmio_log = if self.probes.contains(Wants::MMIO) { Some(Vec::new()) } else { None };
        self.bus.observe_gpio(self.probes.contains(Wants::GPIO));
    }
    /// Attach a timing model before the machine has executed or reset.
    pub fn set_cost_model(&mut self, mut model: Box<dyn CostModel>) -> Result<(), String> {
        if self.cost.is_some() { return Err("a cost model is already attached".into()); }
        if let Some(reason) = self.model_attach_error { return Err(reason.into()); }
        if self.bus.cycles() != 0 || self.reboots != 0 || self.cores.iter().any(|core| core.insn_count() != 0) {
            return Err("cost model attachment requires a pristine machine with no execution or reset".into());
        }
        model.lifecycle(&LifecycleFacts { kind: LifecycleKind::Attach, chip: S::NAME, cores: S::CORES, cpu_hz: S::CPU_HZ })?;
        self.model_ready_at.fill(0);
        self.cost = Some(model);
        Ok(())
    }
    pub fn has_observer(&self, name: &str) -> bool { self.observers.iter().any(|o| o.name() == name) }
    /// Every observer's end-of-run report, in the order they were added (files are written now).
    pub fn reports(&mut self) -> String {
        let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ };
        self.observers.iter_mut().map(|o| o.report(&cx)).filter(|r| !r.is_empty()).collect::<Vec<_>>().join("\n")
    }
    /// Deliver the MMIO and GPIO events the bus recorded since the last call.
    fn deliver_events(&mut self) {
        if self.probes.contains(Wants::MMIO) {
            let log = self.bus.misc().mmio_log.as_mut().map(std::mem::take).unwrap_or_default();
            if !log.is_empty() { let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ }; for o in &mut self.observers { if o.wants().contains(Wants::MMIO) { for &(pc, a, v, w) in &log { o.on_mmio(&cx, pc, a, v, w); } } } }
        }
        if self.probes.contains(Wants::GPIO) {
            let ev = self.bus.take_gpio_events();
            if !ev.is_empty() { for o in &mut self.observers { if o.wants().contains(Wants::GPIO) { for &(c, p, l) in &ev { o.on_gpio(c, p, l); } } } }
        }
    }

    // ------------------------------------------------------------------ images
    pub fn load_rom(&mut self, rom_elf: &[u8]) -> Result<(), String> {
        let e = elf::parse(rom_elf)?;
        for s in &e.segments {
            if s.data.is_empty() { continue; }
            self.bus.load_bytes(s.vaddr, &s.data)?;
            // the mask ROM also holds the initialiser image at paddr (copied by the reset handler)
            if s.paddr != s.vaddr { let _ = self.bus.load_bytes(s.paddr, &s.data); }
        }
        // RAM initialisers live in sections without program headers (.data.interface.*, .data_*)
        let dbg = self.debug_rom;
        if dbg { eprintln!("[emu] rom: {} segments, {} alloc sections", e.segments.len(), e.sections.len()); }
        for s in &e.sections {
            if dbg { eprintln!("[emu]   section {:<36} addr {:#010x} len {:#x} bss={}", s.name, s.addr, s.data.len(), s.is_bss); }
            if s.is_bss || s.data.is_empty() { continue; }
            if let Err(err) = self.bus.load_bytes(s.addr, &s.data) { eprintln!("[emu] rom section {} @ {:#x}: {}", s.name, s.addr, err); }
        }
        // The reset handler copies RAM initialisers from ROM using a 16-byte-entry table
        // (dst_start, dst_end, rom_src, 0) between _data_start and _data_end. The ELF does
        // not carry the ROM-side copies for the W-only sections, so back-fill them from the
        // RAM contents we just loaded.
        let find = |name: &str| e.by_name.get(name).copied();
        let start = S::ROM_DATA_TABLE.iter().find_map(|n| find(n));
        let end = S::ROM_DATA_TABLE_END.iter().find_map(|n| find(n));
        if let (Some(ds), Some(de)) = (start, end) {
            let mut t = ds; let mut n = 0;
            while t + S::ROM_DATA_TABLE_STRIDE <= de {
                let (Ok(d0), Ok(d1), Ok(src)) = (self.bus.read32(t), self.bus.read32(t + 4), self.bus.read32(t + 8)) else { break };
                if d1 > d0 && d1 - d0 < 0x20000 {
                    let bytes: Vec<u8> = (d0..d1).map(|a| self.bus.read8(a).unwrap_or(0)).collect();
                    if self.bus.load_bytes(src, &bytes).is_ok() { n += 1; }
                }
                t += S::ROM_DATA_TABLE_STRIDE;
            }
            if dbg { eprintln!("[emu] rom: back-filled {} initialiser blocks into ROM from table {:#x}..{:#x}", n, ds, de); }
        }
        self.symbols.extend(e.symbols);
        Ok(())
    }

    pub fn add_symbols(&mut self, elf_bytes: &[u8]) -> Result<(), String> {
        self.symbols.extend(elf::parse(elf_bytes)?.symbols);
        Ok(())
    }

    pub fn write_flash(&mut self, offset: usize, data: &[u8]) -> Result<(), String> { self.bus.write_flash(offset, data) }

    /// Boot the application image at flash `app_off` the way the 2nd-stage bootloader would.
    pub fn boot_app(&mut self, app_off: usize) -> Result<u32, String> {
        if self.cost.is_some() { return Err("synthetic app boot is unsupported with a cost model; boot from the reset vector".into()); }
        self.model_attach_error = Some("cost model attachment after synthetic app boot is unsupported without a configuration snapshot");
        let entry = self.bus.boot_app(app_off)?;
        S::boot_core(&mut self.cores[0], entry);
        Ok(entry)
    }

    /// Cold boot from the mask ROM reset vector (needs ROM + flash image with bootloader).
    pub fn boot_rom(&mut self) { self.cores[0].reset(); }

    /// Chip reset (software / watchdog): cores back to the reset vector, digital peripherals
    /// re-initialised; SRAM, RTC memories, efuses and the RTC-domain registers survive, as on
    /// silicon. Returns the reset cause that the ROM will report.
    pub fn reboot(&mut self) -> u32 {
        // where core 0 was when the reset took effect (the C6 ROM prints it as `Saved PC`)
        let pc = self.cores[0].pc();
        self.bus.note_pc(pc);
        let cause = self.bus.reboot(self.mac);
        for (i, c) in self.cores.iter_mut().enumerate() { S::reset_core(c, i); if i > 0 { self.core_held[i] = true; } }
        self.reboots += 1;
        #[cfg(not(target_arch = "wasm32"))]
        { self.uart_tcp_next_rx = 0; }
        self.model_ready_at.fill(self.bus.cycles());
        if let Some(model) = &mut self.cost {
            let facts = LifecycleFacts { kind: LifecycleKind::ChipReset, chip: S::NAME, cores: S::CORES, cpu_hz: S::CPU_HZ };
            if let Err(reason) = model.lifecycle(&facts) {
                self.model_stop = Some(Stop::CostModelLifecycle { kind: facts.kind, reason });
            }
        }
        cause
    }

    /// Address of a symbol loaded from the ELFs.
    pub fn sym_addr(&self, name: &str) -> Option<u32> { self.symbols.iter().find(|(_, n)| n.as_str() == name).map(|(&a, _)| a) }

    pub fn sym(&self, addr: u32) -> String {
        match self.symbols.range(..=addr).next_back() {
            Some((&a, n)) if addr - a < 0x10000 => if a == addr { n.clone() } else { format!("{}+{:#x}", n, addr - a) },
            _ => String::new(),
        }
    }

    // ------------------------------------------------------------------ console
    pub fn drain_console(&mut self) {
        use std::io::Write;
        let streams = self.bus.console_take();
        #[cfg(not(target_arch = "wasm32"))]
        self.uart_tcp_output(&streams[1]);
        let mut o = std::io::stdout();
        let (mask, prefix, capture) = (self.console.mask, self.console.prefix, self.console.capture);
        let mut emit = |bit: u32, tag: &str, d: Vec<u8>, all: &mut Vec<u8>| {
            if d.is_empty() { return; }
            all.extend_from_slice(&d);
            if mask & bit == 0 || capture { return; }
            if prefix { for line in d.split_inclusive(|&b| b == b'\n') { let _ = o.write_all(tag.as_bytes()); let _ = o.write_all(line); } } else { let _ = o.write_all(&d); }
            let _ = o.flush();
        };
        for (i, d) in streams.into_iter().enumerate() {
            let src = ["usb", "uart0", "uart1", "uart2"][i];
            if i < 2 {
                let backlog = if i == 0 { &mut self.console.usb } else { &mut self.console.uart0 };
                backlog.extend_from_slice(&d);
                if backlog.len() > 65536 { let cut = backlog.len() - 49152; backlog.drain(..cut); }
            }
            if let Some(w) = &self.web { if !d.is_empty() { w.send_text(&format!("{{\"t\":\"serial\",\"src\":\"{}\",\"data\":\"{}\"}}", src, crate::web::json_escape(&String::from_utf8_lossy(&d)))); } }
            let (bit, tag) = [(1, "[usb]  "), (2, "[uart0] "), (4, "[uart1] "), (4, "[uart2] ")][i];
            emit(bit, tag, d, &mut self.console.all);
        }
    }

    // ------------------------------------------------------------------ interrupts
    /// After a device change: re-derive the lines and present them to every core.
    #[inline]
    fn refresh_irq(&mut self) {
        if !*self.bus.irq_dirty() { return; }
        *self.bus.irq_dirty() = false;
        if self.bus.refresh_irq() { self.present_irqs(); }
    }
    fn present_irqs(&mut self) {
        let mut irqs = [<S::Core as Core>::Irq::default(); 4];
        S::irqs(&self.bus, &mut irqs[..S::CORES]);
        for (i, c) in self.cores.iter_mut().enumerate() { c.set_irq(irqs[i]); }
        if self.probes.contains(Wants::IRQ) {
            let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ };
            for (i, irq) in irqs.iter().enumerate().take(S::CORES) {
                let now = S::Core::irq_bits(irq);
                let mut rising = now & !self.prev_irq[i];
                self.prev_irq[i] = now;
                while rising != 0 { let line = rising.trailing_zeros(); rising &= rising - 1; for o in &mut self.observers { if o.wants().contains(Wants::IRQ) { o.on_irq_raised(&cx, i, line); } } }
            }
        }
    }

    // ------------------------------------------------------------------ execution
    /// Execute up to `budget` instructions on `core` the fast way (blocks, JIT). Returns the
    /// iterations consumed (as `step_core` would have counted them) and a stop, if any.
    #[cfg_attr(all(target_arch = "wasm32", feature = "wasm-cpu-profile"), inline(never))]
    #[cfg_attr(not(all(target_arch = "wasm32", feature = "wasm-cpu-profile")), inline)]
    fn step_blocks(&mut self, core: usize, budget: u32) -> (u32, Option<Stop>) {
        // Core::run returns a trap without its faulting PC. One-instruction fragments make
        // the entry PC exact while retaining callbacks for combined BLOCK/TRAP observers.
        let budget = if self.probes.contains(Wants::TRAP_PC) { budget.min(1) } else { budget };
        let cpu = &mut self.cores[core];
        let pc = cpu.pc();
        // stubs and probes are block boundaries, so testing them at block start is exact
        if (self.stub_bloom | self.probe_bloom) & pc_bit(pc) != 0 && !cpu.waiting() {
            if let Some(name) = self.fn_probes.get(&pc) {
                let (args, ret) = (cpu.probe_args(&mut self.bus), cpu.return_address(&mut self.bus));
                eprintln!("[fn] i={} t={:.4}s c{} {}({}) ret={:#x}", cpu.insn_count(), self.bus.cycles() as f64 / S::CPU_HZ as f64, core, name, args, ret);
            }
            if let Some(&ret) = self.stubs.get(&pc) { cpu.return_from_stub(&mut self.bus, ret); self.stub_hits += 1; return (1, None); }
        }
        let (used, trap) = cpu.run(&mut self.bus, budget);
        if self.probes.contains(Wants::BLOCK | Wants::TRAP | Wants::TRAP_PC) {
            let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ };
            let cpu = &self.cores[core];
            for o in &mut self.observers {
                let w = o.wants();
                if w.contains(Wants::BLOCK) && used > 0 { o.on_block(&cx, core, pc, used); }
                if let (true, Some(t)) = (w.contains(Wants::TRAP | Wants::TRAP_PC), &trap) { o.on_trap(&cx, core, cpu, pc, t); }
            }
        }
        match trap {
            None => {}
            Some(Trap::Exception(_)) => { self.exceptions += 1; }
            Some(Trap::Interrupt(irq)) => { self.interrupts += 1; self.irq_hist[core][(irq & 31) as usize] += 1; }
            Some(Trap::Unimplemented(p, raw)) => { if self.dbg.stop_on_unimplemented { return (used, Some(Stop::Unimplemented(p, raw))); } }
            Some(Trap::Simcall) => return (used, Some(Stop::Simcall(pc))),
            Some(Trap::Ebreak(p)) => { self.exceptions += 1; if !self.cores[core].has_trap_handler() { return (used, Some(Stop::Ebreak(p))); } }
        }
        self.refresh_irq();
        if self.exceptions >= self.dbg.stop_after_exceptions { return (used, Some(Stop::Exceptions(self.exceptions))); }
        (used, None)
    }

    /// Execute one instruction on `core` with every per-instruction observer; returns Some(stop) if the run must end.
    #[inline]
    fn step_core(&mut self, core: usize) -> Option<Stop> {
        let cpu = &mut self.cores[core];
        let pc = cpu.pc();
        if self.probe_bloom & pc_bit(pc) != 0 && !cpu.waiting() {
            if let Some(name) = self.fn_probes.get(&pc) {
                let (args, ret) = (cpu.probe_args(&mut self.bus), cpu.return_address(&mut self.bus));
                eprintln!("[fn] i={} t={:.4}s c{} {}({}) ret={:#x}", cpu.insn_count(), self.bus.cycles() as f64 / S::CPU_HZ as f64, core, name, args, ret);
            }
        }
        if self.stub_bloom & pc_bit(pc) != 0 && !cpu.waiting() {
            if let Some(&ret) = self.stubs.get(&pc) { cpu.return_from_stub(&mut self.bus, ret); self.stub_hits += 1; return None; }
        }
        {
            let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ };
            for o in &mut self.observers { if o.wants().contains(Wants::INSN) { if let Some(stop) = o.on_insn(&cx, core, &self.cores[core], &mut self.bus, pc) { return Some(stop); } } }
        }
        let cpu = &mut self.cores[core];
        self.bus.note_pc(pc);
        let outcome = cpu.step(&mut self.bus);
        let r = outcome.result();
        if let (true, Err(t)) = (self.probes.contains(Wants::TRAP | Wants::TRAP_PC), &r) {
            let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ };
            let cpu = &self.cores[core];
            for o in &mut self.observers { if o.wants().contains(Wants::TRAP | Wants::TRAP_PC) { o.on_trap(&cx, core, cpu, pc, t); } }
        }
        let cpu = &self.cores[core];
        match r {
            Ok(()) => {}
            Err(Trap::Exception(_)) => { self.exceptions += 1; }
            Err(Trap::Interrupt(irq)) => { self.interrupts += 1; self.irq_hist[core][(irq & 31) as usize] += 1; }
            Err(Trap::Unimplemented(p, raw)) => { if self.dbg.stop_on_unimplemented { return Some(Stop::Unimplemented(p, raw)); } }
            Err(Trap::Simcall) => return Some(Stop::Simcall(pc)),
            Err(Trap::Ebreak(p)) => { self.exceptions += 1; if !cpu.has_trap_handler() { return Some(Stop::Ebreak(p)); } }
        }
        self.refresh_irq();
        {
            let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ };
            for o in &mut self.observers { if o.wants().contains(Wants::INSN) { if let Some(stop) = o.after_insn(&cx, core, &self.cores[core], &mut self.bus) { return Some(stop); } } }
        }
        if self.exceptions >= self.dbg.stop_after_exceptions { return Some(Stop::Exceptions(self.exceptions)); }
        None
    }

    /// Run until something stops us, for at most `max_insns` scheduling steps. The no-model path
    /// uses 64-instruction quanta; the modeled path schedules one priced event at a time.
    pub fn run(&mut self, max_insns: u64) -> Stop {
        self.web_poll_input();
        #[cfg(not(target_arch = "wasm32"))]
        self.uart_tcp_poll_input();
        self.refresh_irq();
        if self.cost.is_some() { self.run_modeled(max_insns) } else { self.run_unmodeled(max_insns) }
    }

    /// The complete unmodelled core-0 quantum a browser-side straight-line block may retire at
    /// the current scheduling boundary. External execution is deliberately limited to the
    /// single-core, unobserved case; returning a partial quantum would move device events relative
    /// to instructions. The caller still has to enforce architectural boundaries such as
    /// CCOMPARE and register-window overflow for the block it proposes.
    pub fn browser_external_block_budget(&self, requested: u32) -> Option<u32> {
        if requested == 0
            || self.cost.is_some()
            || self.probes.0 != 0
            || !self.stubs.is_empty()
            || !self.fn_probes.is_empty()
            || self.script.pos < self.script.events.len()
            || self.bus.sw_reset()
            || self.bus.block_break()
            || self.cores[0].waiting()
            || self.cores[0].irq_pending()
            || (1..S::CORES).any(|core| S::core_state(&self.bus, core) == CoreState::Running)
        {
            return None;
        }
        Some(QUANTUM as u32)
    }

    /// Advance shared device time after a quantum accepted by
    /// `browser_external_block_budget`. Architectural core state must already contain the full
    /// quantum's result.
    pub fn finish_browser_external_quantum(&mut self) -> Option<Stop> {
        self.after_round(QUANTUM);
        if self.bus.sw_reset() { self.drain_console(); return Some(Stop::SwReset); }
        if self.bus.cycles() >= self.max_cycles { self.drain_console(); return Some(Stop::Halted); }
        self.drain_console();
        None
    }

    fn run_unmodeled(&mut self, max_insns: u64) -> Stop {
        self.stub_bloom = self.stubs.keys().fold(0, |m, &pc| m | pc_bit(pc));
        self.probe_bloom = self.fn_probes.keys().fold(0, |m, &pc| m | pc_bit(pc));
        for c in &mut self.cores {
            c.set_boundaries(self.stub_bloom | self.probe_bloom);
            c.set_block_observation(self.probes.contains(Wants::BLOCK | Wants::TRAP_PC));
        }
        // Per-instruction observers need the slow hooks; only exact-PC trap observers use bounded fragments.
        let blocks = !self.probes.contains(Wants::INSN);
        let slow_path = self.probes.contains(Wants::NO_IDLE_SKIP);
        let trace = self.has_observer("trace");
        let mut n = 0u64;
        let mut on = [true; 4];
        let mut idle = [true; 4];
        loop {
            if n >= max_insns { self.drain_console(); return Stop::MaxInsns; }
            self.apply_script_events();
            self.refresh_irq();
            if self.bus.cycles() >= self.max_cycles { self.drain_console(); return Stop::Halted; }
            for (i, state) in on.iter_mut().enumerate().take(S::CORES).skip(1) {
                *state = match S::core_state(&self.bus, i) {
                    CoreState::Reset => { self.core_held[i] = true; false }
                    CoreState::Held => false,
                    CoreState::Running => {
                        if self.core_held[i] { self.core_held[i] = false; S::reset_core(&mut self.cores[i], i); if trace { eprintln!("          ** core{} released from reset", i); } }
                        true
                    }
                };
            }
            for (i, state) in idle.iter_mut().enumerate().take(S::CORES) { *state = !on[i] || (self.cores[i].waiting() && !self.cores[i].irq_pending()); }
            if idle[..S::CORES].iter().all(|&x| x) && !slow_path {
                // Stop at every known source of new work, including core-local timers. Device
                // deadlines alone do not include CCOMPARE or host-script actions.
                let limit = S::IDLE_CHUNK.min(max_insns - n).min(self.max_cycles - self.bus.cycles());
                let chunk = self.idle_budget(limit, &on);
                for (i, &enabled) in on.iter().enumerate().take(S::CORES) { if enabled { self.cores[i].idle_advance(chunk as u32); } }
                n += chunk;
                self.after_round(chunk);
                if self.bus.sw_reset() { self.drain_console(); return Stop::SwReset; }
                if self.bus.cycles() >= self.max_cycles { self.drain_console(); return Stop::Halted; }
                if n & 0xffff < chunk { self.drain_console(); }
                continue;
            }
            for i in 0..S::CORES {
                if !on[i] { continue; }
                if idle[i] && !slow_path { self.cores[i].idle_advance(QUANTUM as u32); } else if blocks {
                    let mut left = QUANTUM as u32;
                    while left > 0 {
                        let (used, stop) = self.step_blocks(i, left);
                        if let Some(stop) = stop { self.drain_console(); return stop; }
                        left -= used.min(left);
                        // a reset takes effect at the instruction that requested it: the core's
                        // run already stopped there (the register write broke the block)
                        if self.bus.sw_reset() { break; }
                    }
                } else {
                    for _ in 0..QUANTUM {
                        if let Some(stop) = self.step_core(i) { self.drain_console(); return stop; }
                        if self.bus.sw_reset() { break; }
                    }
                }
                if i == 0 { n += QUANTUM; }
            }
            self.after_round(QUANTUM);
            if self.bus.sw_reset() { self.drain_console(); return Stop::SwReset; }
            if self.bus.cycles() >= self.max_cycles { self.drain_console(); return Stop::Halted; }
            if n & 0xffff < QUANTUM { self.drain_console(); }
        }
    }

    /// Positive idle advance bounded by device work, enabled cores' wakeups and host actions.
    /// Callers settle actions already due and check their own stop bound before using this.
    fn idle_budget(&self, limit: u64, on: &[bool]) -> u64 {
        let mut budget = limit.min(u32::MAX as u64 >> 1);
        if let Some(delta) = self.bus.next_deadline() { budget = budget.min(delta.max(1)); }
        for (core, &enabled) in self.cores.iter().zip(on) {
            if enabled { if let Some(delta) = core.cycles_until_wake() { budget = budget.min(delta.max(1)); } }
        }
        if let Some((at, _)) = self.script.events.get(self.script.pos) {
            budget = budget.min(at.saturating_sub(self.bus.cycles()).max(1));
        }
        budget
    }

    fn run_modeled(&mut self, max_insns: u64) -> Stop {
        if let Some(stop) = &self.model_stop { return stop.clone(); }
        self.stub_bloom = self.stubs.keys().fold(0, |mask, &pc| mask | pc_bit(pc));
        self.probe_bloom = self.fn_probes.keys().fold(0, |mask, &pc| mask | pc_bit(pc));
        for core in &mut self.cores { core.set_boundaries(self.stub_bloom | self.probe_bloom); core.flush_caches(); }
        for observer in &mut self.observers { observer.on_modeled_run(); }

        let trace = self.has_observer("trace");
        let force_idle = self.probes.contains(Wants::NO_IDLE_SKIP);
        let mut on = [false; 4];
        let mut events = 0u64;
        loop {
            if let Some(stop) = self.refresh_modeled_core_states(&mut on, trace) {
                self.model_stop = Some(stop.clone()); self.drain_console(); return stop;
            }
            if events >= max_insns { self.drain_console(); return Stop::MaxInsns; }
            if let Err(stop) = self.settle_modeled_time(&mut on, trace, force_idle) {
                if matches!(stop, Stop::CostModelLifecycle { .. }) { self.model_stop = Some(stop.clone()); }
                self.drain_console(); return stop;
            }

            let now = self.bus.cycles();
            let Some(core) = (0..S::CORES)
                .filter(|&i| on[i] && (force_idle || !self.cores[i].waiting() || self.cores[i].irq_pending()))
                .filter(|&i| self.model_ready_at[i] <= now)
                .min_by_key(|&i| (self.model_ready_at[i], i))
            else { self.drain_console(); return Stop::Halted };

            let start = now;
            let pc = self.cores[core].pc();
            let cost = match self.step_core_modeled(core) {
                Ok(cost) => cost,
                Err(stop) => {
                    if matches!(stop, Stop::CostModel { .. } | Stop::CostModelLifecycle { .. }) { self.model_stop = Some(stop.clone()); }
                    self.drain_console(); return stop;
                }
            };
            let Some(ready) = start.checked_add(cost as u64) else {
                let stop = Stop::CostModel { core, pc, reason: "cost model cycle frontier overflow".into() };
                self.model_stop = Some(stop.clone());
                self.drain_console();
                return stop;
            };
            if cost > 1 { self.cores[core].advance_cycles(cost - 1); }
            self.model_ready_at[core] = ready;
            events += 1;

            if let Err(stop) = self.settle_modeled_time(&mut on, trace, force_idle) {
                if matches!(stop, Stop::CostModelLifecycle { .. }) { self.model_stop = Some(stop.clone()); }
                self.drain_console(); return stop;
            }
            if events & 0xffff == 0 { self.drain_console(); }
        }
    }

    fn refresh_modeled_core_states(&mut self, on: &mut [bool; 4], trace: bool) -> Option<Stop> {
        for (i, state) in on.iter_mut().enumerate().take(S::CORES) {
            *state = match S::core_state(&self.bus, i) {
                CoreState::Reset => { self.core_held[i] = true; false }
                CoreState::Held => false,
                CoreState::Running => {
                    if self.core_held[i] {
                        self.core_held[i] = false;
                        S::reset_core(&mut self.cores[i], i);
                        self.model_ready_at[i] = self.bus.cycles();
                        if trace { eprintln!("          ** core{} released from reset", i); }
                        let facts = LifecycleFacts { kind: LifecycleKind::CoreReset(i), chip: S::NAME, cores: S::CORES, cpu_hz: S::CPU_HZ };
                        if let Some(model) = &mut self.cost {
                            if let Err(reason) = model.lifecycle(&facts) { return Some(Stop::CostModelLifecycle { kind: facts.kind, reason }); }
                        }
                    }
                    true
                }
            };
        }
        None
    }

    /// Advance the shared device horizon until at least one active core can start an event.
    fn settle_modeled_time(&mut self, on: &mut [bool; 4], trace: bool, force_idle: bool) -> Result<(), Stop> {
        loop {
            if let Some(stop) = self.refresh_modeled_core_states(on, trace) { return Err(stop); }
            self.after_round_rest();
            self.refresh_irq();
            if self.bus.sw_reset() { return Err(Stop::SwReset); }
            let now = self.bus.cycles();
            if now >= self.max_cycles { return Err(Stop::Halted); }

            let next_core = (0..S::CORES)
                .filter(|&i| on[i] && (force_idle || !self.cores[i].waiting() || self.cores[i].irq_pending()))
                .map(|i| self.model_ready_at[i].max(now))
                .min();
            if next_core == Some(now) { return Ok(()); }

            let mut target = next_core;
            if let Some(delta) = self.bus.next_deadline() {
                let deadline = now.saturating_add(delta.max(1));
                target = Some(target.map_or(deadline, |current| current.min(deadline)));
            }
            if let Some(&(deadline, _)) = self.script.events.get(self.script.pos) {
                let deadline = deadline.max(now.saturating_add(1));
                target = Some(target.map_or(deadline, |current| current.min(deadline)));
            }
            for (i, &enabled) in on.iter().enumerate().take(S::CORES) {
                if enabled && self.cores[i].waiting() && !self.cores[i].irq_pending() {
                    if let Some(delta) = self.cores[i].cycles_until_wake() {
                        let deadline = self.model_ready_at[i].max(now).saturating_add(delta.max(1));
                        target = Some(target.map_or(deadline, |current| current.min(deadline)));
                    }
                }
            }
            let Some(target) = target.map(|target| target.min(self.max_cycles)) else { return Err(Stop::Halted); };
            if target <= now { return Err(Stop::Halted); }
            self.advance_modeled_time(target, on);
        }
    }

    fn advance_modeled_time(&mut self, target: u64, on: &[bool; 4]) {
        for (i, &enabled) in on.iter().enumerate().take(S::CORES) {
            if enabled && self.cores[i].waiting() && !self.cores[i].irq_pending() && self.model_ready_at[i] < target {
                let mut remaining = target - self.model_ready_at[i];
                while remaining != 0 {
                    let step = remaining.min(u32::MAX as u64) as u32;
                    self.cores[i].advance_cycles(step);
                    remaining -= step as u64;
                }
                self.model_ready_at[i] = target;
            }
        }
        while self.bus.cycles() < target {
            let step = (target - self.bus.cycles()).min(u32::MAX as u64);
            self.after_round(step);
        }
    }

    fn step_core_modeled(&mut self, core: usize) -> Result<u32, Stop> {
        let pc = self.cores[core].pc();
        if self.probe_bloom & pc_bit(pc) != 0 && !self.cores[core].waiting() {
            if let Some(name) = self.fn_probes.get(&pc) {
                let cpu = &self.cores[core];
                let (args, ret) = (cpu.probe_args(&mut self.bus), cpu.return_address(&mut self.bus));
                eprintln!("[fn] i={} t={:.4}s c{} {}({}) ret={:#x}", cpu.insn_count(), self.bus.cycles() as f64 / S::CPU_HZ as f64, core, name, args, ret);
            }
        }
        if self.stub_bloom & pc_bit(pc) != 0 && !self.cores[core].waiting() && self.stubs.contains_key(&pc) {
            return Err(Stop::CostModel { core, pc, reason: "function stubs are unsupported by modeled execution".into() });
        }
        {
            let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ };
            for observer in &mut self.observers {
                if observer.wants().contains(Wants::INSN) {
                    if let Some(stop) = observer.on_insn(&cx, core, &self.cores[core], &mut self.bus, pc) { return Err(stop); }
                }
            }
        }

        self.bus.note_pc(pc);
        let (outcome, accesses) = {
            let mut bus = RecordingBus::new(&mut self.bus);
            let mut outcome = self.cores[core].step(&mut bus);
            // A control operation is an occurrence, not a decoded intention. Current cores can
            // report it before a privilege or execution failure, so only retirement commits it.
            if !matches!(outcome.kind, StepKind::Retired) { outcome.control = None; }
            let accesses = bus.finish(outcome.bytes, outcome.pc);
            (outcome, accesses)
        };
        if let Some(trap) = outcome.trap() {
            if self.probes.contains(Wants::TRAP | Wants::TRAP_PC) {
                let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ };
                for observer in &mut self.observers {
                    if observer.wants().contains(Wants::TRAP | Wants::TRAP_PC) { observer.on_trap(&cx, core, &self.cores[core], pc, &trap); }
                }
            }
            match trap {
                Trap::Exception(_) => { self.exceptions += 1; }
                Trap::Interrupt(irq) => { self.interrupts += 1; self.irq_hist[core][(irq & 31) as usize] += 1; }
                Trap::Unimplemented(at, raw) if self.dbg.stop_on_unimplemented => return Err(Stop::Unimplemented(at, raw)),
                Trap::Simcall => return Err(Stop::Simcall(pc)),
                Trap::Ebreak(at) if !self.cores[core].has_trap_handler() => { self.exceptions += 1; return Err(Stop::Ebreak(at)); }
                Trap::Ebreak(_) => { self.exceptions += 1; }
                Trap::Unimplemented(_, _) => {}
            }
        }
        self.refresh_irq();
        {
            let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ };
            for observer in &mut self.observers {
                if observer.wants().contains(Wants::INSN) {
                    if let Some(stop) = observer.after_insn(&cx, core, &self.cores[core], &mut self.bus) { return Err(stop); }
                }
            }
        }
        if self.probes.0 != 0 { self.deliver_events(); }
        if self.bus.sw_reset() { return Err(Stop::SwReset); }
        if self.exceptions >= self.dbg.stop_after_exceptions { return Err(Stop::Exceptions(self.exceptions)); }

        let facts = ExecutionFacts { core, outcome, accesses: &accesses };
        let result = self.cost.as_mut().expect("modeled path requires an attached model").cycles(&facts);
        match result {
            Ok(0) => Err(Stop::CostModel { core, pc, reason: "cost model returned zero cycles".into() }),
            Ok(cycles) => Ok(cycles),
            Err(reason) => Err(Stop::CostModel { core, pc, reason }),
        }
    }
    /// Run core 0 until device time reaches `target` (a cycle count of `bus.cycles()`), exactly:
    /// the round is cut short at the target, so time never overshoots by a quantum, and cut
    /// short at the instruction — or the device tick — that raises a host event
    /// (`SocBus::take_host_event`), so a transmission the host must forward is seen at the cycle
    /// it started. Every round is also bounded by the bus's next device deadline
    /// (`SocBus::next_deadline`), so a device event lands at its own cycle rather than at the end
    /// of a 64-instruction quantum; a core asleep in `wfi` with nothing pending lets time jump to
    /// the target or to that deadline, whichever is first — the deadline is conservative, so an
    /// interrupt is never delivered late by the skip. Single-core chips only (the S3's second
    /// core is not scheduled here), and the unmodeled path only: a cost model is not consulted.
    /// Nothing else about a run changes: stubs, probes, observers, scripts and the console work
    /// as in `run`; `max_cycles` is not consulted.
    pub fn run_until_cycle(&mut self, target: u64) -> RunUntil {
        self.web_poll_input();
        self.refresh_irq();
        self.stub_bloom = self.stubs.keys().fold(0, |m, &pc| m | pc_bit(pc));
        self.probe_bloom = self.fn_probes.keys().fold(0, |m, &pc| m | pc_bit(pc));
        for c in &mut self.cores {
            c.set_boundaries(self.stub_bloom | self.probe_bloom);
            c.set_block_observation(self.probes.contains(Wants::BLOCK | Wants::TRAP_PC));
        }
        let blocks = !self.probes.contains(Wants::INSN);
        let no_skip = self.probes.contains(Wants::NO_IDLE_SKIP);
        loop {
            let now = self.bus.cycles();
            if now >= target { return RunUntil::Reached; }
            if self.apply_script_events() { self.drain_console(); return RunUntil::Stop(Stop::Halted); }
            self.refresh_irq();
            let left = target - now;
            let core = &self.cores[0];
            if core.waiting() && !core.irq_pending() && !no_skip {
                let chunk = self.idle_budget(left, &[true]);
                self.cores[0].idle_advance(chunk as u32);
                if self.after_round(chunk) { self.drain_console(); return RunUntil::Stop(Stop::Halted); }
                if self.bus.take_host_event() { return RunUntil::Yield; }
            } else {
                let mut deadline = self.bus.next_deadline().unwrap_or(u64::MAX).max(1);
                if let Some((at, _)) = self.script.events.get(self.script.pos) {
                    deadline = deadline.min(at.saturating_sub(now).max(1));
                }
                let mut budget = left.min(QUANTUM).min(deadline) as u32;
                let (mut used_total, mut yielded, mut stop) = (0u64, false, None);
                while budget > 0 {
                    let (used, s) = if blocks { self.step_blocks(0, budget) } else { (1, self.step_core(0)) };
                    used_total += used as u64;
                    budget -= used.min(budget);
                    if s.is_some() { stop = s; break; }
                    if self.bus.sw_reset() { break; }
                    if self.bus.take_host_event() { yielded = true; break; }
                }
                let script_stopped = self.after_round(used_total);
                if let Some(s) = stop { self.drain_console(); return RunUntil::Stop(s); }
                if script_stopped { self.drain_console(); return RunUntil::Stop(Stop::Halted); }
                if yielded || self.bus.take_host_event() { return RunUntil::Yield; }
            }
            if self.bus.sw_reset() { self.drain_console(); return RunUntil::Stop(Stop::SwReset); }
        }
    }

    /// Re-derive the interrupt lines now, after the host changed a device (a frame injected
    /// between rounds), so the next instruction sees them.
    pub fn sync_irq(&mut self) { *self.bus.irq_dirty() = true; self.refresh_irq(); }

    /// Device time, interrupt lines, scripts, web, real-time pacing after a scheduling round.
    #[inline]
    fn after_round(&mut self, cycles: u64) -> bool {
        // device models only change state when they run, so the lines are re-derived after a
        // flush or a register write and never on a fixed cadence
        let ticked = self.bus.tick(cycles as u32) != 0;
        if *self.bus.irq_dirty() || ticked {
            *self.bus.irq_dirty() = false;
            if self.bus.refresh_irq() { self.present_irqs(); }
        }
        let script_stopped = self.after_round_rest();
        if self.probes.0 != 0 {
            self.deliver_events();
            if self.probes.contains(Wants::ROUND) { let cx = Ctx { symbols: &self.symbols, cycles: self.bus.cycles(), cpu_hz: S::CPU_HZ }; for o in &mut self.observers { if o.wants().contains(Wants::ROUND) { o.on_round(&cx); } } }
        }
        script_stopped
    }

    /// Keep the common empty/not-due case in the scheduling loop without inlining action
    /// cloning, logging or device dispatch. Read the public script state at each boundary so
    /// host edits between runs and events inserted by web input are observed immediately.
    #[inline]
    fn apply_script_events(&mut self) -> bool {
        if self.script.events.get(self.script.pos).is_none_or(|(at, _)| *at > self.bus.cycles()) {
            return false;
        }
        self.apply_due_script_events()
    }

    /// Apply actions at the current boundary without advancing device time.
    #[inline(never)]
    fn apply_due_script_events(&mut self) -> bool {
        let mut stopped = false;
        while self.script.pos < self.script.events.len() && self.script.events[self.script.pos].0 <= self.bus.cycles() {
            let (t, a) = self.script.events[self.script.pos].clone(); self.script.pos += 1;
            if self.script.log { eprintln!("[script] t={:.3}s {:?}", t as f64 / S::CPU_HZ as f64, a); }
            match a {
                ScriptAction::Gpio(pin, level) => { self.bus.gpio_set_input(pin, level); *self.bus.irq_dirty() = true; }
                ScriptAction::Serial(text) => self.bus.serial_input(text.as_bytes()),
                ScriptAction::Uart(n, text) => self.bus.uart_input(n, text.as_bytes()),
                ScriptAction::Stop => { self.max_cycles = 0; stopped = true; }
                ScriptAction::Touch(x, y, d) => { self.bus.touch_input(x, y, d); }
                ScriptAction::Poke(a, v) => { let _ = self.bus.write32(a, v); }
            }
        }
        stopped
    }

    #[inline]
    fn after_round_rest(&mut self) -> bool {
        let stopped = self.apply_script_events();
        #[cfg(not(target_arch = "wasm32"))]
        self.uart_tcp_poll_input();
        if self.web.is_some() && self.bus.cycles().wrapping_sub(self.ws.last_push_cycles) >= S::CPU_HZ / 50 { self.ws.last_push_cycles = self.bus.cycles(); self.web_push(); self.web_poll_input(); }
        if self.rt.enabled && self.bus.cycles().wrapping_sub(self.rt.last_check) >= 1 << 16 {
            self.rt.last_check = self.bus.cycles();
            let start = *self.rt.wall_start.get_or_insert_with(std::time::Instant::now);
            let emulated = std::time::Duration::from_secs_f64(self.bus.cycles() as f64 / S::CPU_HZ as f64);
            let wall = start.elapsed();
            let (now, cycles) = (std::time::Instant::now(), self.bus.cycles());
            match self.rt.speed_mark {
                Some((at, from)) if cycles >= from && now.duration_since(at) >= std::time::Duration::from_secs(1) => {
                    self.rt.speed = Some((cycles - from) as f64 / S::CPU_HZ as f64 / now.duration_since(at).as_secs_f64());
                    self.rt.speed_mark = Some((now, cycles));
                }
                Some((_, from)) if cycles < from => self.rt.speed_mark = Some((now, cycles)),   // the count restarted
                None => self.rt.speed_mark = Some((now, cycles)),
                _ => {}
            }
            if emulated > wall + std::time::Duration::from_millis(2) { std::thread::sleep(emulated - wall); self.rt.behind = 0.0; }
            else if wall > emulated + std::time::Duration::from_millis(50) {
                self.rt.behind = (wall - emulated).as_secs_f64();
                // more than half a second behind: resynchronise (skip the lag) rather than flood the client while catching up
                if wall > emulated + std::time::Duration::from_millis(500) { self.rt.resyncs += 1; self.rt.wall_start = Some(std::time::Instant::now() - emulated); }
            } else { self.rt.behind = 0.0; }
        }
        stopped
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn uart_tcp_poll_input(&mut self) {
        let Some(uart_tcp) = self.uart_tcp.clone() else { return };
        let now = self.bus.cycles();
        if uart_tcp.pending_input() == 0 {
            self.uart_tcp_next_rx = now;
            return;
        }
        // TCP has no baud metadata. esptool starts at 115200 baud, with ten wire bits per byte.
        // Pace delivery in emulated time so a large TCP read cannot instantaneously overflow the
        // ROM loader's software receive ring even though the hardware FIFO itself has room.
        const BAUD: u64 = 115_200;
        let cycles_per_byte = (S::CPU_HZ * 10 / BAUD).max(1);
        if now < self.uart_tcp_next_rx { return; }
        let due = 1 + (now - self.uart_tcp_next_rx) / cycles_per_byte;
        let room = self.bus.uart_rx_capacity(0);
        if room == 0 { return; }
        let data = uart_tcp.take_input(room.min(due as usize));
        if !data.is_empty() {
            self.bus.uart_input(0, &data);
            self.uart_tcp_next_rx = self.uart_tcp_next_rx.saturating_add(data.len() as u64 * cycles_per_byte);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn uart_tcp_output(&mut self, data: &[u8]) {
        const PENDING_LIMIT: usize = 1 << 20;
        let Some(uart_tcp) = self.uart_tcp.clone() else { return };
        if !uart_tcp.connected() { self.uart_tcp_pending.clear(); return; }
        self.uart_tcp_pending.extend(data);
        if self.uart_tcp_pending.len() > PENDING_LIMIT {
            eprintln!("[emu] UART TCP client disconnected: output queue exceeded {} bytes", PENDING_LIMIT);
            uart_tcp.disconnect();
            self.uart_tcp_pending.clear();
            return;
        }
        while !self.uart_tcp_pending.is_empty() {
            let pending = self.uart_tcp_pending.make_contiguous();
            let accepted = uart_tcp.queue_output(pending);
            if accepted == 0 { break; }
            self.uart_tcp_pending.drain(..accepted);
        }
    }

    // ------------------------------------------------------------------ web UI
    /// Send display / audio / ring updates to the browser (called ~50x per emulated second).
    fn web_push(&mut self) {
        if self.rt.log {
            let now = std::time::Instant::now();
            let (i0, i1) = (self.cores[0].insn_count(), self.cores.get(1).map_or(0, |c| c.insn_count()));
            if let Some(last) = self.rt.log_last {
                let dt = now.duration_since(last).as_secs_f64() * 1e3;
                if dt > 40.0 {
                    let (p0, p1) = (self.cores[0].pc(), self.cores.get(1).map_or(0, |c| c.pc()));
                    eprintln!("[rt] t={:.2}s window took {:.0} ms: core0 {} insns (pc {:08x} {}), core1 {} insns (pc {:08x} {})", self.seconds(), dt,
                              i0 - self.rt.log_insns.0, p0, self.sym(p0), i1 - self.rt.log_insns.1, p1, self.sym(p1));
                }
            }
            self.rt.log_last = Some(now); self.rt.log_insns = (i0, i1);
        }
        let Some(w) = self.web.clone() else { return };
        self.drain_console();
        let board = self.bus.board_ref();
        let ver = board.display_version();
        // Prefer one quiet push interval for pixel streams, but never defer a changed frame
        // twice: continuous drawing must remain visible. This is a UI snapshot, not scanout.
        let changed = ver != self.ws.px_sent;
        let due = changed && (!board.display_quiet_push() || ver == self.ws.px_pending || self.ws.px_deferred);
        self.ws.px_pending = ver;
        self.ws.px_deferred = changed && !due;
        if due {
            if let Some((w_, h_, px, _)) = board.display() {
                self.ws.px_sent = ver;
                let mut b = vec![1u8, w_ as u8, (w_ >> 8) as u8, h_ as u8, (h_ >> 8) as u8];
                for p in &px { b.push(*p as u8); b.push((*p >> 8) as u8); }
                w.send_binary(&b);
            }
        }
        let board = self.bus.board_ref();
        if self.ws.cam_pushed != self.bus.camera_frames() / 20 || !self.ws.cam_sent {
            if let Some(rgb) = board.camera_preview(320, 240) {
                let mut b = vec![4u8, (320u16 & 255) as u8, (320u16 >> 8) as u8, (240u16 & 255) as u8, (240u16 >> 8) as u8]; b.extend_from_slice(&rgb); w.send_binary(&b); self.ws.cam_sent = true;
            }
            self.ws.cam_pushed = self.bus.camera_frames() / 20;
        }
        let (pcm, rate) = self.bus.audio();
        if pcm.len() > self.ws.audio_sent {
            let chunk = &pcm[self.ws.audio_sent..];
            let mut b = vec![2u8];
            b.extend_from_slice(&rate.to_le_bytes());
            for s in chunk { b.extend_from_slice(&s.to_le_bytes()); }
            w.send_binary(&b);
            self.ws.audio_sent = pcm.len();
        }
        let board = self.bus.board_ref();
        let grids: Vec<(&'static str, Vec<[u8; 3]>, u64)> =
            board.led_grids().into_iter().map(|(id, leds, updates)| (id, leds.to_vec(), updates)).collect();
        self.ws.grid_updates.resize(grids.len(), u64::MAX);
        for (i, (id, leds, updates)) in grids.iter().enumerate() {
            if self.ws.grid_updates[i] == *updates { continue; }
            self.ws.grid_updates[i] = *updates;
            w.send_text(&format!("{{\"t\":\"grid\",\"id\":\"{}\",\"leds\":[{}]}}", id, leds_json(leds)));
        }
        let board = self.bus.board_ref();
        if let Some((leds, updates)) = board.leds() { if updates != self.ws.ring_updates {
            self.ws.ring_updates = updates;
            let leds: Vec<String> = leds.iter().map(|c| format!("[{},{},{}]", c[0], c[1], c[2])).collect();
            w.send_text(&format!("{{\"t\":\"ring\",\"leds\":[{}]}}", leds.join(",")));
        } }
        // snapshot for late-joining clients: backlog, frame, ring
        if w.needs_hello() {
            use crate::web::json_escape;
            let mut hello: Vec<Vec<u8>> = Vec::new();
            let mk = |s: &str| -> Vec<u8> { let mut f = vec![0x81u8]; let n = s.len(); if n < 126 { f.push(n as u8); } else if n < 65536 { f.push(126); f.extend_from_slice(&(n as u16).to_be_bytes()); } else { f.push(127); f.extend_from_slice(&(n as u64).to_be_bytes()); } f.extend_from_slice(s.as_bytes()); f };
            let mkb = |d: &[u8]| -> Vec<u8> { let mut f = vec![0x82u8]; let n = d.len(); if n < 126 { f.push(n as u8); } else if n < 65536 { f.push(126); f.extend_from_slice(&(n as u16).to_be_bytes()); } else { f.push(127); f.extend_from_slice(&(n as u64).to_be_bytes()); } f.extend_from_slice(d); f };
            hello.push(mk(&format!("{{\"t\":\"serial\",\"src\":\"uart0\",\"data\":\"{}\"}}", json_escape(&String::from_utf8_lossy(&self.console.uart0)))));
            hello.push(mk(&format!("{{\"t\":\"serial\",\"src\":\"usb\",\"data\":\"{}\"}}", json_escape(&String::from_utf8_lossy(&self.console.usb)))));
            let controls = board.panel_controls().iter().map(|control| format!("{{\"name\":\"{}\",\"label\":\"{}\",\"x\":{},\"y\":{}}}", json_escape(control.name), json_escape(control.label), control.x, control.y)).collect::<Vec<_>>().join(",");
            hello.push(mk(&format!("{{\"t\":\"board\",\"name\":\"{}\",\"rotate\":{},\"controls\":[{}]}}", board.name(), board.display_rotation(), controls)));
            if let Some((w_, h_, px, _)) = board.display() { let mut b = vec![1u8, w_ as u8, (w_ >> 8) as u8, h_ as u8, (h_ >> 8) as u8]; for p in &px { b.push(*p as u8); b.push((*p >> 8) as u8); } hello.push(mkb(&b)); }
            if let Some(rgb) = board.camera_preview(320, 240) { let mut b = vec![4u8, (320u16 & 255) as u8, (320u16 >> 8) as u8, (240u16 & 255) as u8, (240u16 >> 8) as u8]; b.extend_from_slice(&rgb); hello.push(mkb(&b)); }
            if let Some((leds, _)) = board.leds() { let leds: Vec<String> = leds.iter().map(|c| format!("[{},{},{}]", c[0], c[1], c[2])).collect();
            hello.push(mk(&format!("{{\"t\":\"ring\",\"leds\":[{}]}}", leds.join(",")))); }
            for (id, leds, _) in board.led_grids() { hello.push(mk(&format!("{{\"t\":\"grid\",\"id\":\"{}\",\"leds\":[{}]}}", id, leds_json(leds)))); }
            w.set_hello(hello);
        }
        w.send_text(&format!("{{\"t\":\"stat\",\"time\":{:.2},\"insns\":{},\"frames\":{},\"behind\":{:.2},\"resyncs\":{},\"speed\":{},\"cam\":{},\"gpio_in\":\"{:x}\"}}", self.seconds(), self.insns(), board.display_frames(), self.rt.behind, self.rt.resyncs, self.rt.speed.map_or_else(|| "null".to_string(), |s| format!("{:.3}", s)), self.bus.camera_frames(), self.bus.gpio_input()));
    }

    // Host input is accepted at run boundaries without advancing device time. The periodic
    // poll remains necessary for native callers that run continuously rather than in slices.
    fn web_poll_input(&mut self) {
        let Some(w) = self.web.clone() else { return };
        use crate::web::json_str;
        for b in w.poll_incoming_bin() {
            // type 3: camera picture from the browser — [3][w u16 le][h u16 le][RGBA...]
            if b.len() >= 5 && b[0] == 3 {
                let (wd, ht) = (u16::from_le_bytes([b[1], b[2]]) as usize, u16::from_le_bytes([b[3], b[4]]) as usize);
                if wd > 0 && ht > 0 && b.len() >= 5 + wd * ht * 4 {
                    let mut rgb = Vec::with_capacity(wd * ht * 3);
                    for px in b[5..5 + wd * ht * 4].chunks(4) { rgb.extend_from_slice(&px[..3]); }
                    self.bus.board().set_camera_picture(crate::picture::Picture { w: wd as u32, h: ht as u32, rgb });
                }
            }
        }
        for m in w.poll_incoming() {
            let t = json_str(&m, "t").unwrap_or_default();
            match t.as_str() {
                "btn" => { let pin: u8 = json_str(&m, "pin").and_then(|x| x.parse().ok()).unwrap_or(0); let v = json_str(&m, "v").unwrap_or_default() == "1";
                           self.bus.gpio_set_input(pin, !v); *self.bus.irq_dirty() = true; }
                "knobpress" => { let v = json_str(&m, "v").unwrap_or_default() == "1"; if let Some(sw) = self.bus.board_ref().named_pin("sw") { self.bus.gpio_set_input(sw, !v); *self.bus.irq_dirty() = true; } }
                "knob" => {
                    let d: i32 = json_str(&m, "d").and_then(|x| x.parse().ok()).unwrap_or(1);
                    let Some((clk, dt)) = self.bus.board_ref().encoder() else { continue };
                    let step = S::CPU_HZ / 500;   // 2 ms per phase
                    let mut tc = (self.bus.cycles() + step).max(self.script.knob_next);   // queue detents back to back, never overlapping
                    for _ in 0..d.unsigned_abs() { for (pn, l) in Self::quadrature(clk, dt, d > 0) { self.script.events.push((tc, ScriptAction::Gpio(pn, l))); tc += step; } tc += step * 4; }
                    self.script.knob_next = tc;
                    // Keep already consumed events before the cursor; pending events at the
                    // current horizon have not necessarily run when input arrives at run entry.
                    self.script.events[self.script.pos..].sort_by_key(|e| e.0);
                }
                // a line with its newline, or `key`: bytes exactly as typed (a terminal on the console)
                "serial" | "key" => {
                    let data = if t == "key" { json_str(&m, "data").unwrap_or_default() } else { format!("{}\n", json_str(&m, "line").unwrap_or_default()) };
                    match json_str(&m, "src").as_deref() {
                        Some("uart0") => self.bus.uart_input(0, data.as_bytes()),
                        Some("uart1") => self.bus.uart_input(1, data.as_bytes()),
                        _ => self.bus.serial_input(data.as_bytes()),
                    }
                }
                "touch" => { let x: u16 = json_str(&m, "x").and_then(|v| v.parse().ok()).unwrap_or(0); let y: u16 = json_str(&m, "y").and_then(|v| v.parse().ok()).unwrap_or(0);
                             let down = json_str(&m, "down").unwrap_or_default() == "1"; self.bus.touch_input(x, y, down); }
                _ => {}
            }
        }
    }

    /// One encoder detent as (pin, level) edges, 2 ms apart. Idle is (1,1); CW: CLK falls while
    /// DT=1, then DT falls, CLK rises, DT rises. CCW: DT first.
    fn quadrature(clk: u8, dt: u8, cw: bool) -> [(u8, bool); 4] {
        if cw { [(clk, false), (dt, false), (clk, true), (dt, true)] } else { [(dt, false), (clk, false), (dt, true), (clk, true)] }
    }

    // ------------------------------------------------------------------ scripts
    /// Parse a script: one action per line, `<seconds> <cmd> [args]`.
    ///   press <pin> [ms]   release <pin>   gpio <pin> <0|1>   serial <text...>   knob <cw|ccw> [detents]   touch <x> <y> <0|1>   poke <addr> <value>   stop
    /// Pins are numbers or the board's names (`btn1`, `sw`, ...); buttons/encoder are active-low with pull-ups (release = 1).
    pub fn load_script(&mut self, text: &str) -> Result<(), String> {
        let hz = S::CPU_HZ as f64;
        let mut ev: Vec<(u64, ScriptAction)> = Vec::new();
        for (ln, line) in text.lines().enumerate() {
            let line = line.trim(); if line.is_empty() || line.starts_with('#') { continue; }
            let mut it = line.splitn(2, char::is_whitespace);
            let t: f64 = it.next().unwrap().parse().map_err(|_| format!("line {}: bad time", ln + 1))?;
            let after = it.next().unwrap_or("").trim_start();
            let cmd = after.split_whitespace().next().unwrap_or("");
            let rest = after[cmd.len()..].trim();
            let board = self.bus.board_ref();
            let pin = |s: &str| -> Result<u8, String> { board.named_pin(s).map(Ok).unwrap_or_else(|| s.parse().map_err(|_| format!("line {}: bad pin {}", ln + 1, s))) };
            let c = (t * hz) as u64;
            match cmd {
                "press" => { let mut p = rest.split_whitespace(); let pn = pin(p.next().unwrap_or(""))?; let ms: f64 = p.next().map(|x| x.parse().unwrap_or(100.0)).unwrap_or(100.0);
                             ev.push((c, ScriptAction::Gpio(pn, false))); ev.push((c + (ms / 1000.0 * hz) as u64, ScriptAction::Gpio(pn, true))); }
                "release" => ev.push((c, ScriptAction::Gpio(pin(rest)?, true))),
                "gpio" => { let mut p = rest.split_whitespace(); let pn = pin(p.next().unwrap_or(""))?; let l = p.next().unwrap_or("1") == "1"; ev.push((c, ScriptAction::Gpio(pn, l))); }
                "poke" => { let mut p = rest.split_whitespace(); let a = u32::from_str_radix(p.next().unwrap_or("0").trim_start_matches("0x"), 16).map_err(|e| e.to_string())?; let v = u32::from_str_radix(p.next().unwrap_or("0").trim_start_matches("0x"), 16).map_err(|e| e.to_string())?; ev.push((c, ScriptAction::Poke(a, v))); }
                "touch" => { let mut p = rest.split_whitespace(); let x: u16 = p.next().and_then(|v| v.parse().ok()).unwrap_or(0); let y: u16 = p.next().and_then(|v| v.parse().ok()).unwrap_or(0); let d = p.next().unwrap_or("1") == "1"; ev.push((c, ScriptAction::Touch(x, y, d))); }
                "serial" => ev.push((c, ScriptAction::Serial(format!("{}\n", rest)))),
                "uart0" | "uart1" => ev.push((c, ScriptAction::Uart(if cmd == "uart0" { 0 } else { 1 }, format!("{}\n", rest)))),
                "knob" => {
                    let mut p = rest.split_whitespace(); let dir = p.next().unwrap_or("cw"); let n: usize = p.next().map(|x| x.parse().unwrap_or(1)).unwrap_or(1);
                    let (clk, dt) = board.encoder().ok_or_else(|| format!("line {}: this board has no encoder", ln + 1))?;
                    let step = (0.002 * hz) as u64;   // 2 ms per quadrature phase
                    let mut tc = c;
                    for _ in 0..n {
                        for (pn, l) in Self::quadrature(clk, dt, dir == "cw") { ev.push((tc, ScriptAction::Gpio(pn, l))); tc += step; }
                        tc += step * 4;
                    }
                }
                "stop" => ev.push((c, ScriptAction::Stop)),
                _ => return Err(format!("line {}: unknown command {}", ln + 1, cmd)),
            }
        }
        ev.sort_by_key(|e| e.0);
        self.script.events = ev; self.script.pos = 0;
        Ok(())
    }

    // ------------------------------------------------------------------ reports and captures
    /// Write captured I2S audio (left channel) as a 16-bit mono WAV.
    pub fn write_wav(&self, path: &str) -> std::io::Result<usize> {
        let (pcm, rate) = self.bus.audio();
        let mut out = Vec::with_capacity(44 + pcm.len() * 2);
        let data_len = (pcm.len() * 2) as u32;
        out.extend_from_slice(b"RIFF"); out.extend_from_slice(&(36 + data_len).to_le_bytes()); out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes()); out.extend_from_slice(&1u16.to_le_bytes()); out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes()); out.extend_from_slice(&(rate * 2).to_le_bytes()); out.extend_from_slice(&2u16.to_le_bytes()); out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data"); out.extend_from_slice(&data_len.to_le_bytes());
        for s in pcm { out.extend_from_slice(&s.to_le_bytes()); }
        std::fs::write(path, out)?;
        Ok(pcm.len())
    }

    pub fn irq_report(&self) -> String {
        let mut s = String::from("[irq] per core, cpu-int: count (peripheral sources mapped to it)\n");
        for core in 0..S::CORES {
            for irq in 0..32 {
                let n = self.irq_hist[core][irq];
                if n == 0 { continue; }
                let srcs: Vec<String> = self.bus.irq_sources_of(core, irq as u32).iter().map(|src| src.to_string()).collect();
                s += &format!("  core{} int{:<2} {:>9}  sources [{}]\n", core, irq, n, srcs.join(","));
            }
        }
        s
    }

    /// Save the board's display (scaled) as PNG.
    pub fn write_tft_png(&self, path: &str, scale: usize) -> std::io::Result<()> {
        let Some((w, h, px, _)) = self.bus.board_ref().display() else { return Err(std::io::Error::other("this board has no display")) };
        png::write_png_rgb565(path, &px, w as usize, h as usize, if w > 200 { 1 } else { scale })
    }
    pub fn write_gram_png(&self, path: &str) -> std::io::Result<()> {
        let Some((px, cols, rows)) = self.bus.board_ref().gram() else { return Err(std::io::Error::other("this board has no TFT")) };
        png::write_png_rgb565(path, &px, cols, rows, 2)
    }

    pub fn disasm(&mut self, addr: u32, n: usize) -> String {
        let mut s = String::new(); let mut pc = addr;
        for _ in 0..n {
            let Ok(b) = self.bus.fetch(pc) else { break };
            let text = self.cores[0].disasm(pc, b);
            let len = S::Core::insn_len(b);
            s += &format!("{:08x}: {:<30} {}\n", pc, text, self.sym(pc));
            pc += len;
        }
        s
    }

    pub fn peek(&mut self, addr: u32, words: usize) -> String {
        let mut s = String::new();
        for i in 0..words { let a = addr.wrapping_add((i * 4) as u32); s += &format!("{:08x}: {}\n", a, match self.bus.read32(a) { Ok(v) => format!("{:08x}", v), Err(_) => "--------".into() }); }
        s
    }

    pub fn dump_regs(&self) -> String {
        let sym = |a: u32| self.sym(a);
        let mut out = String::new();
        for (i, c) in self.cores.iter().enumerate() { if i == 0 || !self.core_held[i] { out += &c.dump(i, &sym); } }
        out
    }
}
