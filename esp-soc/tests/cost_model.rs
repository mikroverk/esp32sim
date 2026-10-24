use emu_core::{Bus, CacheOperation, ControlEvent, ControlEventKind, Core, CostModel, ExecutionFacts, Fault, LifecycleFacts, LifecycleKind, MemoryAccess, MemoryAccessKind, StepKind, StepOutcome, Trap};
use esp_periph::Misc;
use esp_soc::{BoardModel, CoreState, Ctx, DebugFlags, Machine, NoBoard, Observer, Soc, SocBus, Stop, Wants};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
struct OwnedFacts {
    core: usize,
    outcome: StepOutcome,
    accesses: Vec<MemoryAccess>,
}

#[derive(Default)]
struct ModelState {
    facts: Vec<OwnedFacts>,
    lifecycle: Vec<LifecycleKind>,
    costs: [u32; 2],
    refusal: Option<String>,
    lifecycle_refusal: Option<LifecycleKind>,
}

struct TestModel(Arc<Mutex<ModelState>>);
impl CostModel for TestModel {
    fn lifecycle(&mut self, facts: &LifecycleFacts) -> Result<(), String> {
        let mut state = self.0.lock().unwrap();
        state.lifecycle.push(facts.kind);
        if state.lifecycle_refusal == Some(facts.kind) {
            Err(format!("refused {:?}", facts.kind))
        } else {
            Ok(())
        }
    }
    fn cycles(&mut self, facts: &ExecutionFacts<'_>) -> Result<u32, String> {
        let mut state = self.0.lock().unwrap();
        state.facts.push(OwnedFacts { core: facts.core, outcome: facts.outcome, accesses: facts.accesses.to_vec() });
        if let Some(reason) = &state.refusal {
            Err(reason.clone())
        } else {
            Ok(state.costs[facts.core])
        }
    }
}

fn new_model(costs: [u32; 2]) -> (Box<dyn CostModel>, Arc<Mutex<ModelState>>) {
    let state = Arc::new(Mutex::new(ModelState { costs, ..ModelState::default() }));
    (Box::new(TestModel(state.clone())), state)
}

struct TestCore {
    id: usize,
    pc: u32,
    insns: u64,
    cycles: u64,
    waiting: bool,
    irq: bool,
}

impl TestCore {
    fn new(id: usize) -> Self { Self { id, pc: id as u32 * 0x20, insns: 0, cycles: 0, waiting: false, irq: false } }
}

impl Core for TestCore {
    type Irq = bool;
    fn reset(&mut self) { *self = Self::new(self.id); }
    fn pc(&self) -> u32 { self.pc }
    fn set_pc(&mut self, pc: u32) { self.pc = pc; }
    fn waiting(&self) -> bool { self.waiting }
    fn insn_count(&self) -> u64 { self.insns }
    fn set_irq(&mut self, irq: bool) { self.irq = irq; }
    fn irq_pending(&self) -> bool { self.irq }
    fn irq_bits(irq: &bool) -> u32 { *irq as u32 }
    fn advance_cycles(&mut self, cycles: u32) { self.cycles += cycles as u64; }
    fn cycles_until_wake(&self) -> Option<u64> { None }
    fn step<B: Bus>(&mut self, bus: &mut B) -> StepOutcome {
        let pc = self.pc;
        self.insns += 1;
        self.cycles += 1;
        if self.waiting && self.irq {
            self.waiting = false;
            self.irq = false;
            return StepOutcome { pc, next_pc: pc, bytes: None, length: 0, kind: StepKind::TrapBefore(Trap::Interrupt(1)), control: None };
        }
        if self.waiting {
            return StepOutcome { pc, next_pc: pc, bytes: None, length: 0, kind: StepKind::Idle, control: None };
        }
        let bytes = match bus.fetch(pc) {
            Ok(bytes) => bytes,
            Err(_) => return StepOutcome { pc, next_pc: pc, bytes: None, length: 0, kind: StepKind::TrapBefore(Trap::Exception(2)), control: None },
        };
        let mut control = None;
        let kind = match bytes[0] {
            1 => {
                let _ = bus.read32(0x100);
                let _ = bus.write16(0x104, 0x55aa);
                StepKind::Retired
            }
            2 => {
                let _ = bus.write8(0x1f0 + self.id as u32, self.id as u8);
                StepKind::Retired
            }
            3 => {
                let _ = bus.write32(0x108, 0xdead_beef);
                let _ = bus.write32(0x300, 0xcafe_babe);
                StepKind::Retired
            }
            4 => {
                self.waiting = true;
                StepKind::Retired
            }
            5 => StepKind::TrapDuring(Trap::Simcall),
            6 => {
                let _ = bus.read16(0x400);
                StepKind::TrapDuring(Trap::Exception(3))
            }
            7 => StepKind::TrapDuring(Trap::Unimplemented(pc, 7)),
            8 => StepKind::TrapDuring(Trap::Ebreak(pc)),
            9 => {
                control = Some(ControlEvent { kind: ControlEventKind::Cache(CacheOperation::DataHitInvalidate), address: 0x1234 });
                StepKind::Retired
            }
            10 => {
                let _ = bus.write32(0x304, 0x1357_2468);
                StepKind::Retired
            }
            11 => {
                let _ = bus.write8(0x1f0 + self.id as u32, self.id as u8);
                self.waiting = true;
                StepKind::Retired
            }
            12 => {
                let _ = bus.write32(0x120, 1);
                StepKind::Retired
            }
            13 => {
                let _ = bus.read32(0x120);
                StepKind::Retired
            }
            14 => {
                control = Some(ControlEvent { kind: ControlEventKind::Cache(CacheOperation::DataHitInvalidate), address: 0x5678 });
                StepKind::TrapDuring(Trap::Exception(4))
            }
            _ => StepKind::Retired,
        };
        self.pc = if matches!(bytes[0], 2 | 4) { pc } else { pc + 4 };
        StepOutcome { pc, next_pc: self.pc, bytes: Some(bytes), length: 1, kind, control }
    }
    fn regs(&self, _out: &mut Vec<(&'static str, u32)>) {}
    fn arg<B: Bus>(&self, _bus: &mut B, _n: usize) -> u32 { 0 }
    fn return_from_stub<B: Bus>(&mut self, _bus: &mut B, _v: u32) {}
    fn disasm(&self, _pc: u32, _bytes: [u8; 4]) -> String { String::new() }
    fn insn_len(_bytes: [u8; 4]) -> u32 { 1 }
    const TRACE_WIDTH: usize = 1;
    fn trace_regs(&self) -> String { String::new() }
    fn regtrace_line(&self, _pc: u32) -> String { String::new() }
    fn dump(&self, _core: usize, _sym: &dyn Fn(u32) -> String) -> String { String::new() }
    fn has_trap_handler(&self) -> bool { false }
    fn probe_args<B: Bus>(&self, _bus: &mut B) -> String { String::new() }
    fn return_address<B: Bus>(&self, _bus: &mut B) -> u32 { 0 }
}

struct TestBus {
    memory: Vec<u8>,
    cycles: u64,
    approximate_penalties: [u32; 2],
    approximate_pending: u32,
    irq: bool,
    dirty: bool,
    secondary: CoreState,
    irq_at: Option<u64>,
    release_at: Option<u64>,
    mmio: u32,
    mmu: u32,
    starts: Arc<Mutex<Vec<(usize, u64, bool)>>>,
    ticks: Arc<Mutex<Vec<u64>>>,
    misc: Misc,
    board: Box<dyn BoardModel>,
    gpio_events: Option<Vec<(u64, u8, bool)>>,
}

impl TestBus {
    fn new() -> Self {
        Self {
            memory: vec![0; 0x200],
            cycles: 0,
            approximate_penalties: [0; 2],
            approximate_pending: 0,
            irq: false,
            dirty: true,
            secondary: CoreState::Held,
            irq_at: None,
            release_at: None,
            mmio: 0,
            mmu: 0,
            starts: Arc::new(Mutex::new(Vec::new())),
            ticks: Arc::new(Mutex::new(Vec::new())),
            misc: Misc::new(),
            board: Box::new(NoBoard),
            gpio_events: None,
        }
    }
    fn range(&self, address: u32, width: usize) -> Result<usize, Fault> {
        let start = address as usize;
        if start.checked_add(width).is_some_and(|end| end <= self.memory.len()) {
            Ok(start)
        } else {
            Err(Fault::Unmapped)
        }
    }
}

impl Bus for TestBus {
    fn note_code_page(&mut self, _vidx: u32) {} // All writes already update versions, or this bus has no decode cache.
    fn read8(&mut self, address: u32) -> Result<u8, Fault> { Ok(self.memory[self.range(address, 1)?]) }
    fn read16(&mut self, address: u32) -> Result<u16, Fault> {
        let start = self.range(address, 2)?;
        Ok(u16::from_le_bytes([self.memory[start], self.memory[start + 1]]))
    }
    fn read32(&mut self, address: u32) -> Result<u32, Fault> {
        if address == 0x300 {
            return Ok(self.mmio);
        }
        if address == 0x304 {
            return Ok(self.mmu);
        }
        let start = self.range(address, 4)?;
        Ok(u32::from_le_bytes(self.memory[start..start + 4].try_into().unwrap()))
    }
    fn write8(&mut self, address: u32, value: u8) -> Result<(), Fault> {
        if (0x1f0..=0x1f1).contains(&address) {
            self.approximate_pending += self.approximate_penalties[(address - 0x1f0) as usize];
            self.starts.lock().unwrap().push(((address - 0x1f0) as usize, self.cycles, self.irq));
            if let Some(log) = &mut self.misc.mmio_log {
                log.push((self.misc.cur_pc, address, value as u32, true));
            }
            if let Some(events) = &mut self.gpio_events {
                events.push((self.cycles, 7, value != 0));
            }
        }
        let start = self.range(address, 1)?;
        self.memory[start] = value;
        Ok(())
    }
    fn take_timing_penalty(&mut self) -> u32 { std::mem::take(&mut self.approximate_pending) }
    fn write16(&mut self, address: u32, value: u16) -> Result<(), Fault> {
        let start = self.range(address, 2)?;
        self.memory[start..start + 2].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }
    fn write32(&mut self, address: u32, value: u32) -> Result<(), Fault> {
        if address == 0x300 {
            self.mmio = value;
            return Ok(());
        }
        if address == 0x304 {
            self.mmu = value;
            return Ok(());
        }
        let start = self.range(address, 4)?;
        self.memory[start..start + 4].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }
    fn fetch(&mut self, pc: u32) -> Result<[u8; 4], Fault> {
        let start = self.range(pc, 4)?;
        Ok(self.memory[start..start + 4].try_into().unwrap())
    }
    fn note_pc(&mut self, pc: u32) { self.misc.cur_pc = pc; }
    fn tick(&mut self, cycles: u32) -> u32 {
        self.cycles += cycles as u64;
        self.ticks.lock().unwrap().push(self.cycles);
        if self.irq_at.is_some_and(|at| self.cycles >= at) {
            self.irq = true;
            self.dirty = true;
        }
        if self.release_at.is_some_and(|at| self.cycles >= at) {
            self.secondary = CoreState::Running;
        }
        1
    }
}

impl SocBus for TestBus {
    fn cycles(&self) -> u64 { self.cycles }
    fn next_deadline(&self) -> Option<u64> { [self.irq_at.filter(|at| *at > self.cycles), self.release_at.filter(|at| *at > self.cycles)].into_iter().flatten().min().map(|at| at - self.cycles) }
    fn irq_dirty(&mut self) -> &mut bool { &mut self.dirty }
    fn refresh_irq(&mut self) -> bool { true }
    fn misc(&mut self) -> &mut Misc { &mut self.misc }
    fn load_bytes(&mut self, address: u32, data: &[u8]) -> Result<(), String> {
        let start = self.range(address, data.len()).map_err(|fault| format!("{fault:?}"))?;
        self.memory[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn write_flash(&mut self, _offset: usize, _data: &[u8]) -> Result<(), String> { Ok(()) }
    fn read_flash(&self, _offset: usize, _len: usize) -> Option<&[u8]> { None }
    fn flash_len(&self) -> usize { 0 }
    fn boot_app(&mut self, _app_off: usize) -> Result<u32, String> { Ok(0) }
    fn reboot(&mut self, _mac: [u8; 6]) -> u32 {
        self.irq = false;
        self.dirty = true;
        0
    }
    fn sw_reset(&self) -> bool { false }
    fn request_reset(&mut self, _cause: u32) {}
    fn reset_cause(&self) -> u32 { 0 }
    fn last_fault(&self) -> Option<(u32, bool)> { None }
    fn console_take(&mut self) -> [Vec<u8>; 4] { std::array::from_fn(|_| Vec::new()) }
    fn serial_input(&mut self, _data: &[u8]) {}
    fn uart_input(&mut self, _n: usize, _data: &[u8]) {}
    fn gpio_set_input(&mut self, _pin: u8, _level: bool) {}
    fn gpio_input(&self) -> u64 { 0 }
    fn observe_gpio(&mut self, on: bool) { self.gpio_events = on.then(Vec::new); }
    fn take_gpio_events(&mut self) -> Vec<(u64, u8, bool)> { self.gpio_events.as_mut().map(std::mem::take).unwrap_or_default() }
    fn board(&mut self) -> &mut dyn BoardModel { &mut *self.board }
    fn board_ref(&self) -> &dyn BoardModel { &*self.board }
    fn audio(&self) -> (&[i16], u32) { (&[], 44_100) }
    fn irq_sources_of(&self, _core: usize, _line: u32) -> Vec<usize> { Vec::new() }
    fn set_debug(&mut self, _flags: &DebugFlags) {}
    fn set_flash_size(&mut self, _bytes: usize) {}
    fn set_strap(&mut self, _value: u32) {}
    fn set_reset_cause(&mut self, _cause: u32) {}
}

struct TestSoc;
impl Soc for TestSoc {
    type Core = TestCore;
    type Bus = TestBus;
    const NAME: &'static str = "test";
    const ROM_ELF: &'static str = "test.elf";
    const CPU_HZ: u64 = 100;
    const CORES: usize = 2;
    const IDLE_CHUNK: u64 = 64;
    const ROM_DATA_TABLE: &'static [&'static str] = &[];
    fn new_core(i: usize) -> TestCore { TestCore::new(i) }
    fn reset_core(core: &mut TestCore, i: usize) { *core = TestCore::new(i); }
    fn boot_core(core: &mut TestCore, entry: u32) {
        core.reset();
        core.pc = entry;
    }
    fn irqs(bus: &TestBus, out: &mut [bool]) { out.fill(bus.irq); }
    fn core_state(bus: &TestBus, core: usize) -> CoreState {
        if core == 0 {
            CoreState::Running
        } else {
            bus.secondary
        }
    }
}

fn new_machine(op0: u8, op1: u8) -> Machine<TestSoc> {
    let mut bus = TestBus::new();
    bus.memory[0] = op0;
    bus.memory[0x20] = op1;
    bus.memory[0x100..0x104].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    Machine::new([0; 6], bus)
}

#[test]
fn approximate_frontiers_allow_other_core_to_run_during_memory_stall() {
    let mut machine = new_machine(2, 2);
    machine.bus.secondary = CoreState::Running;
    machine.bus.approximate_penalties = [4, 0];
    machine.set_approximate_jit_timing(1, 1).unwrap();
    machine.set_approximate_jit_frontiers(true).unwrap();
    machine.max_cycles = 6;
    assert!(matches!(machine.run(100), Stop::Halted));
    assert_eq!(*machine.bus.starts.lock().unwrap(), vec![
        (0, 0, false), (1, 0, false), (1, 1, false), (1, 2, false),
        (1, 3, false), (1, 4, false), (0, 5, false), (1, 5, false),
    ]);
}

#[test]
fn captures_conceptual_fetch_and_ordered_cpu_accesses() {
    let mut machine = new_machine(1, 0);
    let (model, state) = new_model([1, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::MaxInsns));
    let state = state.lock().unwrap();
    let facts = &state.facts[0];
    assert_eq!(facts.outcome.pc, 0);
    assert_eq!(facts.outcome.next_pc, 4);
    assert_eq!(
        facts.accesses,
        vec![
            MemoryAccess { kind: MemoryAccessKind::Fetch, address: 0, width: 4, value: 1, fault: None },
            MemoryAccess { kind: MemoryAccessKind::Read, address: 0x100, width: 4, value: 0x1234_5678, fault: None },
            MemoryAccess { kind: MemoryAccessKind::Write, address: 0x104, width: 2, value: 0x55aa, fault: None },
        ]
    );
}

#[test]
fn interrupt_before_fetch_has_no_invented_access() {
    let mut machine = new_machine(0, 0);
    machine.cores[0].waiting = true;
    machine.bus.irq = true;
    let (model, state) = new_model([1, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::MaxInsns));
    let state = state.lock().unwrap();
    assert!(matches!(state.facts[0].outcome.kind, StepKind::TrapBefore(Trap::Interrupt(1))));
    assert!(state.facts[0].accesses.is_empty());
}

#[test]
fn refusal_keeps_effects_and_does_not_advance_devices() {
    let mut machine = new_machine(3, 0);
    let (model, state) = new_model([1, 1]);
    state.lock().unwrap().refusal = Some("unpriced store".into());
    let ticks = machine.bus.ticks.clone();
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::CostModel { core: 0, pc: 0, ref reason } if reason == "unpriced store"));
    assert_eq!(machine.bus.read32(0x108), Ok(0xdead_beef));
    assert_eq!(machine.bus.read32(0x300), Ok(0xcafe_babe));
    assert_eq!(machine.bus.cycles, 0);
    assert!(ticks.lock().unwrap().is_empty());
    assert!(matches!(machine.run(1), Stop::CostModel { ref reason, .. } if reason == "unpriced store"));
    assert_eq!(state.lock().unwrap().facts.len(), 1);
}

#[test]
fn zero_cost_is_refused() {
    let mut machine = new_machine(0, 0);
    let (model, _) = new_model([0, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::CostModel { core: 0, pc: 0, ref reason } if reason == "cost model returned zero cycles"));
}

#[test]
fn shared_scheduler_uses_ready_time_and_core_index_ties() {
    let mut machine = new_machine(2, 2);
    machine.bus.secondary = CoreState::Running;
    let starts = machine.bus.starts.clone();
    let (model, _) = new_model([10, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(12), Stop::MaxInsns));
    let expected = std::iter::once((0, 0, false)).chain((0..10).map(|cycle| (1, cycle, false))).chain(std::iter::once((0, 10, false))).collect::<Vec<_>>();
    assert_eq!(*starts.lock().unwrap(), expected);
    assert_eq!((machine.cores[0].cycles, machine.cores[1].cycles), (20, 10));
    assert_eq!(machine.bus.cycles, 10);
}

#[test]
fn equal_timestamp_effects_are_visible_in_core_index_order() {
    let mut machine = new_machine(12, 13);
    machine.bus.secondary = CoreState::Running;
    let (model, state) = new_model([10, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(2), Stop::MaxInsns));
    let state = state.lock().unwrap();
    assert_eq!((state.facts[0].core, state.facts[1].core), (0, 1));
    assert_eq!(state.facts[1].accesses[1], MemoryAccess { kind: MemoryAccessKind::Read, address: 0x120, width: 4, value: 1, fault: None });
}

#[test]
fn deadline_is_visible_at_its_exact_cycle() {
    let mut machine = new_machine(2, 2);
    machine.bus.secondary = CoreState::Running;
    machine.bus.irq_at = Some(5);
    let starts = machine.bus.starts.clone();
    let (model, _) = new_model([10, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(7), Stop::MaxInsns));
    let starts = starts.lock().unwrap();
    assert!(!starts.iter().find(|entry| entry.1 == 4).unwrap().2);
    assert!(starts.iter().find(|entry| entry.1 == 5).unwrap().2);
}

#[test]
fn released_core_starts_at_current_horizon() {
    let mut machine = new_machine(2, 2);
    machine.bus.release_at = Some(5);
    let starts = machine.bus.starts.clone();
    let (model, state) = new_model([10, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(2), Stop::MaxInsns));
    assert_eq!(*starts.lock().unwrap(), vec![(0, 0, false), (1, 5, false)]);
    assert_eq!(state.lock().unwrap().lifecycle, vec![LifecycleKind::Attach, LifecycleKind::CoreReset(1)]);
}

#[test]
fn lifecycle_refusals_stop_before_execution() {
    let mut machine = new_machine(2, 0);
    let (model, state) = new_model([1, 1]);
    state.lock().unwrap().lifecycle_refusal = Some(LifecycleKind::ChipReset);
    machine.set_cost_model(model).unwrap();
    machine.reboot();
    assert!(matches!(machine.run(1), Stop::CostModelLifecycle { kind: LifecycleKind::ChipReset, .. }));
    assert!(state.lock().unwrap().facts.is_empty());
    assert!(matches!(machine.run(1), Stop::CostModelLifecycle { kind: LifecycleKind::ChipReset, .. }));

    let mut machine = new_machine(2, 2);
    machine.bus.secondary = CoreState::Running;
    let (model, state) = new_model([1, 1]);
    state.lock().unwrap().lifecycle_refusal = Some(LifecycleKind::CoreReset(1));
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::CostModelLifecycle { kind: LifecycleKind::CoreReset(1), .. }));
    assert!(state.lock().unwrap().facts.is_empty());
}

#[test]
fn attach_refusal_and_nonpristine_attach_are_explicit() {
    let mut machine = new_machine(0, 0);
    let (model, state) = new_model([1, 1]);
    state.lock().unwrap().lifecycle_refusal = Some(LifecycleKind::Attach);
    assert_eq!(machine.set_cost_model(model), Err("refused Attach".into()));

    let mut machine = new_machine(0, 0);
    assert!(matches!(machine.run(1), Stop::MaxInsns));
    let (model, _) = new_model([1, 1]);
    assert_eq!(machine.set_cost_model(model), Err("cost model attachment requires a pristine machine with no execution or reset".into()));

    let mut machine = new_machine(0, 0);
    machine.boot_app(0).unwrap();
    let (model, _) = new_model([1, 1]);
    assert_eq!(machine.set_cost_model(model), Err("cost model attachment after synthetic app boot is unsupported without a configuration snapshot".into()));

    let mut machine = new_machine(0, 0);
    let (model, _) = new_model([1, 1]);
    machine.set_cost_model(model).unwrap();
    assert_eq!(machine.boot_app(0), Err("synthetic app boot is unsupported with a cost model; boot from the reset vector".into()));
}

#[test]
fn faulting_access_retains_attempted_shape() {
    let mut machine = new_machine(6, 0);
    let (model, state) = new_model([1, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::MaxInsns));
    let state = state.lock().unwrap();
    assert_eq!(state.facts[0].accesses[1], MemoryAccess { kind: MemoryAccessKind::Read, address: 0x400, width: 2, value: 0, fault: Some(Fault::Unmapped) });
    assert!(matches!(state.facts[0].outcome.kind, StepKind::TrapDuring(Trap::Exception(3))));
}

#[test]
fn fetch_fault_and_control_event_are_forwarded() {
    let mut machine = new_machine(0, 0);
    machine.cores[0].pc = 0x400;
    let (model, state) = new_model([1, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::MaxInsns));
    let state = state.lock().unwrap();
    assert_eq!(state.facts[0].accesses, vec![MemoryAccess { kind: MemoryAccessKind::Fetch, address: 0x400, width: 4, value: 0, fault: Some(Fault::Unmapped) }]);
    drop(state);

    let mut machine = new_machine(9, 0);
    let (model, state) = new_model([1, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::MaxInsns));
    assert_eq!(state.lock().unwrap().facts[0].outcome.control, Some(ControlEvent { kind: ControlEventKind::Cache(CacheOperation::DataHitInvalidate), address: 0x1234 }));

    let mut machine = new_machine(14, 0);
    let (model, state) = new_model([1, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::MaxInsns));
    assert_eq!(state.lock().unwrap().facts[0].outcome.control, None);
}

#[test]
fn mmu_table_write_keeps_its_value() {
    let mut machine = new_machine(10, 0);
    let (model, state) = new_model([1, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::MaxInsns));
    assert_eq!(machine.bus.mmu, 0x1357_2468);
    assert_eq!(state.lock().unwrap().facts[0].accesses[1], MemoryAccess { kind: MemoryAccessKind::Write, address: 0x304, width: 4, value: 0x1357_2468, fault: None });
}

#[derive(Default)]
struct ObserverState {
    before: u32,
    after: u32,
    traps: u32,
    irqs: u32,
    mmio: u32,
    gpio: u32,
    rounds: u32,
}

struct TestObserver(Arc<Mutex<ObserverState>>);
impl Observer<TestSoc> for TestObserver {
    fn name(&self) -> &'static str { "test" }
    fn wants(&self) -> Wants { Wants::INSN | Wants::TRAP | Wants::IRQ | Wants::MMIO | Wants::GPIO | Wants::ROUND }
    fn on_insn(&mut self, _cx: &Ctx, _core: usize, _cpu: &TestCore, _bus: &mut TestBus, _pc: u32) -> Option<Stop> {
        self.0.lock().unwrap().before += 1;
        None
    }
    fn after_insn(&mut self, _cx: &Ctx, _core: usize, _cpu: &TestCore, _bus: &mut TestBus) -> Option<Stop> {
        self.0.lock().unwrap().after += 1;
        None
    }
    fn on_trap(&mut self, _cx: &Ctx, _core: usize, _cpu: &TestCore, _pc: u32, _trap: &Trap) { self.0.lock().unwrap().traps += 1; }
    fn on_irq_raised(&mut self, _cx: &Ctx, _core: usize, _line: u32) { self.0.lock().unwrap().irqs += 1; }
    fn on_mmio(&mut self, _cx: &Ctx, _pc: u32, _address: u32, _value: u32, _write: bool) { self.0.lock().unwrap().mmio += 1; }
    fn on_gpio(&mut self, _cycle: u64, _pin: u8, _level: bool) { self.0.lock().unwrap().gpio += 1; }
    fn on_round(&mut self, _cx: &Ctx) { self.0.lock().unwrap().rounds += 1; }
}

#[test]
fn modeled_path_delivers_slow_path_observers() {
    let mut machine = new_machine(11, 0);
    machine.bus.irq_at = Some(5);
    let observed = Arc::new(Mutex::new(ObserverState::default()));
    machine.add_observer(Box::new(TestObserver(observed.clone())));
    let (model, _) = new_model([1, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(2), Stop::MaxInsns));
    let observed = observed.lock().unwrap();
    assert_eq!((observed.before, observed.after, observed.traps), (2, 2, 1));
    assert_eq!((observed.mmio, observed.gpio), (1, 1));
    assert!(observed.irqs >= 1);
    assert!(observed.rounds >= 1);
}

#[test]
fn every_instruction_terminal_stop_wins_over_model_refusal() {
    for (opcode, expected) in [(5, "simcall"), (7, "unimplemented"), (8, "ebreak")] {
        let mut machine = new_machine(opcode, 0);
        let (model, state) = new_model([1, 1]);
        state.lock().unwrap().refusal = Some("must not replace terminal stop".into());
        machine.set_cost_model(model).unwrap();
        let stop = machine.run(1);
        assert!(match expected {
            "simcall" => matches!(stop, Stop::Simcall(0)),
            "unimplemented" => matches!(stop, Stop::Unimplemented(0, 7)),
            "ebreak" => matches!(stop, Stop::Ebreak(0)),
            _ => unreachable!(),
        });
        assert!(state.lock().unwrap().facts.is_empty());
    }
}

#[test]
fn block_profile_reports_modeled_unavailability() {
    let mut machine = new_machine(0, 0);
    machine.add_observer(Box::new(esp_soc::observers::BlockProfile::new(5)));
    let (model, _) = new_model([1, 1]);
    machine.set_cost_model(model).unwrap();
    assert!(matches!(machine.run(1), Stop::MaxInsns));
    assert_eq!(machine.reports(), "[profile-blocks] unavailable during modeled single-step execution\n");
}

#[test]
fn stub_symbols_take_precedence_and_addresses_require_prefix() {
    let mut machine = new_machine(0, 0);
    machine.symbols.insert(0x1234, "deadbeef".into());
    machine.symbols.insert(0x5678, "0xcafe".into());
    assert_eq!(machine.resolve_stub("deadbeef"), Some(0x1234));
    assert_eq!(machine.resolve_stub("0xcafe"), Some(0x5678));
    for missing in ["add", "cafe", "dead", "f", "0x0xcafe"] {
        assert_eq!(machine.resolve_stub(missing), None);
    }
    assert_eq!(machine.resolve_stub("0x123abc"), Some(0x123abc));
}
