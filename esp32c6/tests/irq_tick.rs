//! C6 tick invalidation: no scheduler or guest-time changes.
use emu_core::Bus;
use esp32c6::{bus::SocBus, periph::src};
use esp_soc::SocBus as _;

fn alarm(bus: &mut SocBus) {
    // Shared Systimer model's comparator 0, ten 16-MHz ticks from reset.
    let s = &mut bus.periph.systimer;
    s.conf = (1 << 30) | (1 << 24);
    s.target[0] = 10;
    s.armed[0] = true;
    s.int_ena = 1;
    bus.periph.intmtx.map[src::SYSTIMER_T0] = 3;
    bus.periph.intc.lines.enable = 1 << 3;
    bus.periph.intc.lines.pri[3] = 1;
}

#[test]
fn quiet_tick_does_not_request_an_interrupt_refresh() {
    let mut b = SocBus::new(1024, [0; 6]);
    b.refresh_irq();
    b.irq_dirty = false;
    for cycles in [1, 9, 54, 64, 256] {
        assert_eq!(b.tick(cycles), 0);
        assert!(!b.irq_dirty);
    }
}

#[test]
fn timer_tick_requests_refresh_at_assertion_not_on_unchanged_level() {
    let mut b = SocBus::new(1024, [0; 6]);
    alarm(&mut b);
    b.refresh_irq();
    b.irq_dirty = false;
    assert_eq!(b.tick(90), 0);
    assert_eq!(b.periph.intc.lines.pending(), None);
    assert_ne!(b.tick(10), 0);
    b.refresh_irq();
    b.irq_dirty = false;
    assert_eq!(b.periph.intc.lines.pending(), Some(3));
    assert_eq!(b.tick(64), 0);
    // An MMIO write still invalidates even with no new clocked source transition.
    b.write32(0x6000_a06c, 1).unwrap();
    assert!(b.irq_dirty);
    b.refresh_irq();
    b.irq_dirty = false;
    assert_eq!(b.periph.intc.lines.pending(), None);
    assert_eq!(b.tick(64), 0);
}

#[test]
fn sleeping_core_wakes_when_the_timer_source_changes() {
    let mut m = esp32c6::machine([0; 6], 1024);
    alarm(&mut m.bus);
    let entry = esp32c6::bus::SRAM_LOW;
    // addi s0,s0,1; jal zero,-4. The trap handler is a deterministic counter.
    m.bus.load_bytes(entry, &[0x13, 0x04, 0x14, 0x00, 0x6f, 0xf0, 0xdf, 0xff]).unwrap();
    m.cores[0].pc = entry;
    m.cores[0].mtvec = entry;
    m.cores[0].mstatus |= 8;
    m.cores[0].waiting = true;
    m.max_cycles = 256;
    m.run(256);
    assert_eq!(m.cores[0].mcause, 0x8000_0003);
    assert!(!m.cores[0].waiting);
    assert!(m.cores[0].retired_count > 0);
    assert!(m.cores[0].x[8] > 0);
}
