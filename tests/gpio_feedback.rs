use emu_core::Bus;
use esp_soc::board::{BoardEdge, BoardModel};

#[derive(Default)]
struct Feedback {
    now: u64,
    edges: Vec<BoardEdge>,
}
impl BoardModel for Feedback {
    fn name(&self) -> &'static str {
        "same-cycle-feedback"
    }
    fn gpio_output_at(&mut self, cycle: u64, changes: &[(u8, bool)], _: u64, _: u64) {
        for &(pin, level) in changes {
            if pin == 4 {
                self.edges.push(BoardEdge {
                    cycle,
                    pin: 5,
                    level: !level,
                });
                self.edges.push(BoardEdge {
                    cycle: cycle + 100,
                    pin: 5,
                    level,
                });
            }
        }
        self.edges.sort_by_key(|e| e.cycle);
    }
    fn advance_to(&mut self, cycle: u64) {
        self.now = cycle;
    }
    fn next_deadline(&self) -> Option<u64> {
        self.edges.first().map(|e| e.cycle)
    }
    fn take_edges(&mut self) -> Vec<BoardEdge> {
        let n = self.edges.partition_point(|e| e.cycle <= self.now);
        self.edges.drain(..n).collect()
    }
}

#[test]
fn gpio_reads_deliver_same_cycle_feedback_and_preserve_future_edges() {
    let mut m = machine!();
    let b = &mut m.bus;
    b.board = Box::<Feedback>::default();
    b.attach_board_devices();
    b.gpio_events = Some(Vec::new());
    b.write32(GPIO + 0x24, 1 << 4).unwrap();
    b.write32(GPIO + 0x74 + 4 * 5, 3 << 7 | 1 << 13).unwrap();
    b.write32(GPIO + 8, 1 << 4).unwrap();
    b.irq_dirty = false;
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & 32, 0);
    assert!(b.irq_dirty);
    assert_ne!(b.periph.gpio.status & 32, 0);
    assert_eq!(b.cycles, 0);
    if CPU_HZ == 160_000_000 {
        assert_eq!(b.read8(GPIO + 0x3c).unwrap() & 32, 0);
    }
    b.tick(99);
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & 32, 0);
    if CPU_HZ == 160_000_000 {
        assert_eq!(b.read16(GPIO + 0x3c).unwrap() & 32, 0);
    }
    b.tick(1);
    assert_ne!(b.read32(GPIO + 0x3c).unwrap() & 32, 0);
    b.write32(GPIO + 0xc, 1 << 4).unwrap();
    assert_ne!(b.read32(GPIO + 0x3c).unwrap() & 32, 0);
    b.tick(100);
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & 32, 0);
    let inputs: Vec<_> = b
        .gpio_events
        .as_ref()
        .unwrap()
        .iter()
        .copied()
        .filter(|&(_, pin, _)| pin == 5)
        .collect();
    assert_eq!(
        inputs,
        [
            (0, 5, false),
            (100, 5, true),
            (100, 5, true),
            (200, 5, false)
        ]
    );
}


#[test]
fn released_inputs_resolve_pulls_outputs_and_irq() {
    use esp_soc::SocBus;
    let mut m = machine!();
    let b = &mut m.bus;
    let mux = if GPIO == 0x6009_1000 { 0x6009_0000 } else { 0x6000_9000 };
    let bit = 1 << 4;
    b.gpio_events = Some(Vec::new());
    b.write32(GPIO + 0x74 + 4 * 4, 3 << 7 | 1 << 13).unwrap();
    b.gpio_set_input(4, false);
    b.write32(mux + 20, 1 << 8).unwrap();
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & bit, 0, "host overrides pull-up");
    b.write32(GPIO + 0x4c, bit).unwrap();
    b.irq_dirty = false;
    b.gpio_release_input(4);
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & bit, bit);
    assert!(b.irq_dirty);
    assert_eq!(b.periph.gpio.status & bit as u64, bit as u64);
    assert_eq!(b.gpio_events.as_ref().unwrap().last(), Some(&(0, 4, true)));
    let n = b.gpio_events.as_ref().unwrap().len();
    b.gpio_release_input(4);
    assert_eq!(b.gpio_events.as_ref().unwrap().len(), n, "idempotent release");
    b.write32(mux + 20, 1 << 7).unwrap();
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & bit, 0);
    b.write32(mux + 20, (1 << 7) | (1 << 8)).unwrap();
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & bit, bit, "simultaneous pulls use high");
    b.write32(GPIO + 0x24, bit).unwrap();
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & bit, 0, "output overrides pulls");
    b.write32(GPIO + 0x4c, bit).unwrap();
    b.irq_dirty = false;
    b.write32(GPIO + 8, bit).unwrap();
    assert!(b.irq_dirty, "output rising edge invalidates IRQ cache");
    assert_ne!(b.periph.gpio.status & bit as u64, 0);
    b.gpio_set_input(4, false);
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & bit, 0, "host overrides output");
    b.gpio_release_input(4);
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & bit, bit);
    b.write32(mux + 20, 1 << 7).unwrap();
    b.write32(GPIO + 0x28, bit).unwrap();
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & bit, 0, "disabled output uses pull");
    b.write32(mux + 20, 0).unwrap();
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & bit, bit, "floating model is high");
}

struct ReleaseBoard { now: u64 }
impl BoardModel for ReleaseBoard {
    fn name(&self) -> &'static str { "release" }
    fn next_deadline(&self) -> Option<u64> { (self.now < 20).then_some(20) }
    fn advance_to(&mut self, cycle: u64) { self.now = cycle; }
    fn input_levels(&self) -> Vec<(u8, bool)> { vec![(4, false), (5, false)] }
    fn released_inputs(&mut self) -> Vec<u8> {
        if self.now >= 20 { vec![4, 5] } else { vec![4] }
    }
}

#[test]
fn board_releases_on_attachment_and_deadline() {
    let mut m = machine!();
    let b = &mut m.bus;
    b.board = Box::new(ReleaseBoard { now: 0 });
    b.attach_board_devices();
    assert_ne!(b.periph.gpio.input & 16, 0, "attachment applies releases after levels");
    assert_eq!(b.periph.gpio.input & 32, 0);
    b.tick(19);
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & 32, 0);
    b.tick(1);
    // A non-GPIO read flushes S3 deferred ticks without invoking the GPIO read hook.
    b.read32(GPIO + 0x38).unwrap();
    assert_ne!(b.periph.gpio.input & 32, 0, "deadline release delivered by tick");
}

#[test]
fn reboot_preserves_host_drives_but_resets_pad_state() {
    use esp_soc::SocBus;
    let mut m = machine!();
    let b = &mut m.bus;
    let mux = if GPIO == 0x6009_1000 { 0x6009_0000 } else { 0x6000_9000 };
    b.gpio_set_input(4, false);
    b.gpio_set_input(5, true);
    b.write32(mux + 24, 1 << 7).unwrap();
    b.write32(GPIO + 0x24, 1 << 6).unwrap();
    for _ in 0..2 {
        b.reboot([0; 6]);
        assert_eq!(b.read32(GPIO + 0x3c).unwrap() & 0x70, 0x60);
        assert_eq!(b.periph.gpio.enable, 0);
        assert_eq!(b.periph.gpio.status, 0);
        assert!(b.periph.gpio.input_changes.is_empty(), "reset must not invent input edges");
        b.write32(mux + 24, 1 << 7).unwrap();
        assert_ne!(b.read32(GPIO + 0x3c).unwrap() & 32, 0, "high drive also survives");
    }
    b.gpio_release_input(4);
    b.gpio_release_input(5);
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & 0x30, 0x10);
    b.reboot([0; 6]);
    assert_eq!(b.read32(GPIO + 0x3c).unwrap() & 0x30, 0x30, "released pins stay released");
}

#[test]
fn inactive_board_never_polls_inputs() {
    struct Idle;
    impl BoardModel for Idle {
        fn name(&self) -> &'static str { "idle" }
        fn uses_gpio_edges(&self) -> bool { false }
        fn advance_to(&mut self, _: u64) { panic!("inactive advance"); }
        fn take_edges(&mut self) -> Vec<BoardEdge> { panic!("inactive edges"); }
    }
    let mut m = machine!();
    m.bus.board = Box::new(Idle);
    m.bus.attach_board_devices();
    m.bus.tick(1000);
    m.bus.read32(GPIO + 0x3c).unwrap();
}

#[test]
fn first_read_delivers_feedback_for_each_width_and_bank() {
    struct OnWrite { pin: u8, pending: bool }
    impl BoardModel for OnWrite {
        fn name(&self) -> &'static str { "widths" }
        fn gpio_output_at(&mut self, _: u64, _: &[(u8, bool)], _: u64, _: u64) { self.pending = true; }
        fn take_edges(&mut self) -> Vec<BoardEdge> {
            if std::mem::take(&mut self.pending) {
                vec![BoardEdge { cycle: 0, pin: self.pin, level: false }]
            } else { Vec::new() }
        }
    }
    let cases: &[(u8, u32)] = if CPU_HZ == 240_000_000 { &[(9, 4), (40, 4)] } else { &[(9, 1), (9, 2), (9, 4)] };
    for &(pin, width) in cases {
        let mut m = machine!();
        let b = &mut m.bus;
        b.board = Box::new(OnWrite { pin, pending: false });
        b.attach_board_devices();
        let base = GPIO + if pin < 32 { 0x3c } else { 0x40 };
        b.write32(GPIO + 0x24, 1 << 4).unwrap();
        let value = match width {
            1 => u32::from(b.read8(base + 1).unwrap()) << 8,
            2 => u32::from(b.read16(base).unwrap()),
            _ => b.read32(base).unwrap(),
        };
        assert_eq!(value & (1 << (pin % 32)), 0, "pin {pin}, width {width}");
        assert_eq!(b.cycles, 0);
    }
}

#[test]
fn unchanged_board_levels_do_not_dirty_irqs() {
    struct Stable;
    impl BoardModel for Stable {
        fn name(&self) -> &'static str { "stable" }
        fn take_edges(&mut self) -> Vec<BoardEdge> {
            vec![BoardEdge { cycle: 0, pin: 4, level: true }]
        }
    }
    let mut m = machine!();
    let b = &mut m.bus;
    b.board = Box::new(Stable);
    b.attach_board_devices();
    b.irq_dirty = false;
    b.read32(GPIO + 0x3c).unwrap();
    assert!(!b.irq_dirty);
}

#[test]
fn matrix_output_queue_uses_chip_selector_width() {
    let mut m = machine!();
    let b = &mut m.bus;
    let select = b.periph.gpio.input_select;
    assert_eq!(select, if CPU_HZ == 160_000_000 && GPIO == 0x6000_4000 { 0x40 } else { 0x80 });
    b.write32(GPIO + 0x24, 1 << 4).unwrap();
    b.write32(GPIO + 0x154, select | 4).unwrap();
    b.write32(GPIO + 8, 1 << 4).unwrap();
    assert_eq!(b.periph.gpio.input_changes, [(4, true)]);
}
