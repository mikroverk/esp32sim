//! The shared peripheral models through the `Device` trait, and the `device_set!` table
//! mechanics from outside the crate: dispatch by block and range, `delta`, `alias`, the generic
//! fallback, interrupt source mapping, tick delivery per clock domain, the timer-deadline query.
use emu_core::{ClockDomain, ClockTree};
use esp_periph::{device_set, mmio, Device, DeviceSet, Dispatch, Gpio, I2s, Misc, RegRam, Systimer, TimerGroup, UsbSerialJtag, WriteEffect, NO_SOURCE};

// ------------------------------------------------------------------ systimer
#[test]
fn systimer_one_shot_fires_on_its_tick_and_says_when() {
    let mut s = Systimer::new();
    Device::write(&mut s, 0x0, (1 << 30) | (1 << 24));      // unit 0 counting, comparator 0 enabled
    Device::write(&mut s, 0x20, 100);                        // target 0 = 100
    Device::write(&mut s, 0x34, 0);                          // unit 0, one-shot
    Device::write(&mut s, 0x50, 1);                          // COMP0_LOAD: arm
    Device::write(&mut s, 0x64, 1);
    assert!(Device::has_deadline(&s));
    assert_eq!(Device::next_deadline(&s), Some(100));
    Device::tick(&mut s, 99);
    assert_eq!(Device::irq_sources(&s), 0); assert_eq!(Device::next_deadline(&s), Some(1));
    Device::tick(&mut s, 1);
    assert_eq!(Device::irq_sources(&s), 1, "comparator 0 raised its source");
    assert_eq!(Device::read(&mut s, 0x70), 1, "INT_ST");
    Device::write(&mut s, 0x6c, 1);
    assert_eq!(Device::irq_sources(&s), 0);
    assert_eq!(Device::next_deadline(&s), None, "one-shot: nothing armed any more");
}

#[test]
fn systimer_periodic_repeats() {
    let mut s = Systimer::new();
    Device::write(&mut s, 0x0, (1 << 30) | (1 << 25));      // unit 0, comparator 1
    Device::write(&mut s, 0x38, (1 << 30) | 50);             // periodic, 50 ticks
    Device::write(&mut s, 0x54, 1);                          // COMP1_LOAD
    Device::write(&mut s, 0x64, 2);
    Device::tick(&mut s, 49); assert_eq!(Device::irq_sources(&s), 0);
    Device::tick(&mut s, 1); assert_eq!(Device::irq_sources(&s), 2);
    Device::write(&mut s, 0x6c, 2);
    Device::tick(&mut s, 50); assert_eq!(Device::irq_sources(&s), 2, "and again a period later");
}

// ------------------------------------------------------------------ timer group
#[test]
fn timer_group_alarm_autoreload_and_deadline() {
    let mut g = TimerGroup::new();
    let cfg = (1 << 31) | (1 << 30) | (1 << 10) | (2 << 13);   // enabled, up, alarm, divider 2
    Device::write(&mut g, 0x0, cfg);
    Device::write(&mut g, 0x10, 10);                         // alarm = 10 -> 20 APB ticks
    Device::write(&mut g, 0x70, 1);                          // int_ena T0
    assert_eq!(Device::clock(&g), Some(ClockDomain::Apb));
    assert_eq!(Device::next_deadline(&g), Some(20));
    Device::tick(&mut g, 19);
    assert_eq!(Device::irq_sources(&g), 0); assert_eq!(Device::next_deadline(&g), Some(1));
    Device::tick(&mut g, 1);
    assert_eq!(Device::irq_sources(&g), 1);
    assert_eq!(Device::read(&mut g, 0x0) & (1 << 10), 0, "one-shot: alarm disarmed");
    // autoreload from LOAD = 0: the count restarts and the alarm stays armed
    Device::write(&mut g, 0x7c, 1);
    Device::write(&mut g, 0x0, cfg | (1 << 29));
    Device::write(&mut g, 0x18, 0); Device::write(&mut g, 0x20, 1);   // load, reload count
    Device::tick(&mut g, 20);
    assert_eq!(Device::irq_sources(&g), 1);
    assert_eq!(Device::read(&mut g, 0x0) & (1 << 10), 1 << 10, "autoreload keeps the alarm");
}

// ------------------------------------------------------------------ GPIO
#[test]
fn gpio_edges_and_interrupt_types() {
    let mut g = Gpio::new();
    Device::write(&mut g, 0x20, 1 << 5);                     // enable pin 5 as output
    Device::write(&mut g, 0x8, 1 << 5);                      // OUT_W1TS
    Device::write(&mut g, 0xc, 1 << 5);                      // OUT_W1TC
    assert_eq!(g.changes, vec![(5, true), (5, false)], "output edges in order, only for enabled pins");
    // pin 7: rising-edge interrupt, enabled for core 0
    Device::write(&mut g, 0x74 + 4 * 7, (1 << 7) | (1 << 13));
    assert!(g.set_input(7, false)); assert_eq!(Device::irq_sources(&g), 0);
    assert!(g.set_input(7, true), "a rising edge latches STATUS");
    assert_eq!(Device::irq_sources(&g), 1);
    Device::write(&mut g, 0x4c, 1 << 7);                     // STATUS_W1TC
    assert_eq!(Device::irq_sources(&g), 0);
    // pin 9: low-level interrupt follows the input
    Device::write(&mut g, 0x74 + 4 * 9, (4 << 7) | (1 << 13));
    g.set_input(9, false); assert_eq!(Device::irq_sources(&g), 1);
    g.set_input(9, true); assert_eq!(Device::irq_sources(&g), 0);
}

// ------------------------------------------------------------------ USB-Serial/JTAG
#[test]
fn usb_sof_cadence_follows_the_cpu_clock() {
    for (hz, period) in [(240_000_000u64, 60_000u64), (160_000_000, 40_000)] {
        let mut u = UsbSerialJtag::new(hz);
        Device::write(&mut u, 0x10, 1 << 1);                 // enable the SOF interrupt
        Device::tick(&mut u, period - 1); assert_eq!(Device::irq_sources(&u), 0, "{} Hz", hz);
        Device::tick(&mut u, 1); assert_eq!(Device::irq_sources(&u), 1, "{} Hz", hz);
    }
    let mut u = UsbSerialJtag::new(240_000_000);
    u.host_input(b"hi");
    assert_eq!(Device::read(&mut u, 0x0), b'h' as u32); assert_eq!(Device::read(&mut u, 0x0), b'i' as u32);
    Device::write(&mut u, 0x0, b'o' as u32); Device::write(&mut u, 0x4, 1);   // WR_DONE flushes
    assert_eq!(u.tx_out, b"o");
}

// ------------------------------------------------------------------ I2S clock tree
#[test]
#[allow(clippy::identity_op)] // Keep the slot-width field's bit position explicit in this register encoding.
fn i2s_frame_rate_from_the_clock_registers() {
    let mut i = I2s::new(240_000_000);
    assert_eq!(i.sample_rate, 44100, "until the clock is programmed");
    let clkm = |n: u32| (1 << 26) | (2 << 27) | n;           // active, PLL_F160M, integer divider n
    Device::write(&mut i, 0x2c, (15 << 0) | (3 << 7) | (15 << 13) | (15 << 18) | (15 << 24)); // 16-bit stereo, BCK divider 4
    Device::write(&mut i, 0x54, (1 << 16) | 3);              // 2 enabled slots
    Device::write(&mut i, 0x3c, 0);                          // no fraction
    Device::write(&mut i, 0x34, clkm(8));                    // MCLK = 160 MHz / 8
    assert_eq!(i.sample_rate, 160_000_000 / 8 / 4 / 32);
    Device::write(&mut i, 0x3c, 1);                          // z = 1, y = 0, x = 0: b/a = 1/1 -> divider 9
    Device::write(&mut i, 0x34, clkm(8));
    assert_eq!(i.sample_rate, (160_000_000 + 9 * 128 / 2) / (9 * 128));
    Device::write(&mut i, 0x34, 8);                          // clock off: the last rate stays
    assert_eq!(i.derive_rate(), None);
}

// ------------------------------------------------------------------ the table
/// A device that counts what it receives.
#[derive(Default)]
struct Probe { ticks: u64, deadline: Option<u64>, domain: Option<ClockDomain>, dbg: bool, last: (u32, u32), irq: u64, tick_irq: Option<u64> }
impl Device for Probe {
    fn read(&mut self, off: u32) -> u32 { 0xd0 | off }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { self.last = (off, v); WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { self.irq }
    fn clock(&self) -> Option<ClockDomain> { self.domain }
    fn tick(&mut self, n: u64) { self.ticks += n; if let Some(irq) = self.tick_irq { self.irq = irq; } }
    fn has_deadline(&self) -> bool { self.deadline.is_some() }
    fn next_deadline(&self) -> Option<u64> { self.deadline }
    fn debug(&mut self, on: bool) { self.dbg = on; }
}

struct Chip { a: Probe, b: Probe, wide: Probe, rom: RegRam, misc: Misc, clock: ClockTree<3> }
const BASE: u32 = 0x6000_0000;
device_set! { Chip; clock: (clock) 240_000_000, [(ClockDomain::Systimer, 15), (ClockDomain::Apb, 3), (ClockDomain::Cpu, 1)];
    0x01 "UART0" (a) => [7];
    0x02 "TIMG0" (b) => [NO_SOURCE, 40];
    0x03 "WIDE" (wide) => [];
    0x04 "WIDE" alias (wide) delta 0x1000 => [];
    0x05 "ROM" (rom) delta -0x800 @ 0x800..=0xfff => [];
    0x05 "TIMG1" alias (b) => [];
}
impl DeviceSet for Chip {
    const BASE: u32 = BASE;
    fn block_name(block: u32) -> &'static str { match block { 0x01 => "UART0", 0x02 => "TIMG0", 0x03 | 0x04 => "WIDE", 0x05 => "ROM", _ => "?" } }
    fn misc(&self) -> &Misc { &self.misc }
    fn misc_mut(&mut self) -> &mut Misc { &mut self.misc }
}
fn chip() -> Chip {
    Chip { a: Probe { domain: Some(ClockDomain::Systimer), deadline: Some(4), ..Default::default() }, b: Probe { domain: Some(ClockDomain::Apb), deadline: Some(7), ..Default::default() },
           wide: Probe { domain: Some(ClockDomain::Cpu), ..Default::default() }, rom: RegRam::new(), misc: Misc::new(), clock: Chip::new_clock() }
}

#[test]
fn table_dispatch_ranges_delta_alias_and_fallback() {
    let mut c = chip();
    assert_eq!(mmio::read32(&mut c, BASE + 0x1008), 0xd0 | 8, "block 1 -> a");
    mmio::write32(&mut c, BASE + 0x2010, 5); assert_eq!(c.b.last, (0x10, 5));
    assert_eq!(mmio::read32(&mut c, BASE + 0x4004), 0xd0 | 0x1004, "the alias adds its delta");
    mmio::write32(&mut c, BASE + 0x5900, 42);
    assert_eq!(c.rom.read(0x100), 42, "range entry with a negative delta");
    mmio::write32(&mut c, BASE + 0x5010, 9); assert_eq!(c.b.last, (0x10, 9), "the rest of the block is the next entry");
    mmio::write32(&mut c, BASE + 0x9abc, 0x1234);
    assert_eq!(mmio::read32(&mut c, BASE + 0x9abc), 0x1234, "unknown blocks read back what was written");
    assert_eq!(Chip::devices().len(), 6);
}

#[test]
#[allow(clippy::unnecessary_min_or_max)] // Keep both timer candidates visible in this deadline-selection assertion.
fn table_sources_ticks_deadlines_and_debug() {
    let mut c = chip();
    c.a.irq = 1; c.b.irq = 0b10; c.wide.irq = 1;
    let st = c.source_status();
    assert_eq!(st[0], 1 << 7, "a's source 0 -> 7; b's bit 1 -> 40 lives in word 1");
    assert_eq!(st[1], 1 << (40 - 32));
    assert_eq!(st[2] | st[3], 0, "NO_SOURCE and empty lists route nowhere");
    Dispatch::tick(&mut c, 45);
    assert_eq!((c.a.ticks, c.b.ticks, c.wide.ticks), (3, 15, 45), "each device gets its own domain's ticks");
    Dispatch::tick(&mut c, 2);
    assert_eq!((c.a.ticks, c.b.ticks, c.wide.ticks), (3, 15, 47), "the alias entry does not tick the device twice");
    // deadlines: (ticks - 1) * divider, minimum over the timers
    assert_eq!(c.cycles_until_deadline(), ((4 - 1) * 15).min((7 - 1) * 3));
    c.a.deadline = None; c.b.deadline = None;
    assert_eq!(c.cycles_until_deadline(), u32::MAX);
    Dispatch::debug(&mut c, "timg", true);
    assert!(c.b.dbg && !c.a.dbg, "TIMG0 and TIMG1 both reach b; UART0 untouched");
    Dispatch::debug(&mut c, "uart0", true); assert!(c.a.dbg);
}

#[test]
fn table_tick_reports_both_irq_edges_without_losing_another_source() {
    let mut c = chip();
    c.a.tick_irq = Some(1);
    assert!(Dispatch::tick(&mut c, 15), "rising source");
    assert!(!Dispatch::tick(&mut c, 15), "unchanged active source");
    c.a.tick_irq = Some(0);
    c.b.tick_irq = Some(2);
    assert!(Dispatch::tick(&mut c, 15), "one source falls while another rises");
    assert_eq!(c.source_status()[0], 0);
    assert_eq!(c.source_status()[1], 1 << 8);
    c.b.tick_irq = Some(0);
    assert!(Dispatch::tick(&mut c, 15), "falling last source");
    assert!(!Dispatch::tick(&mut c, 15), "unchanged inactive sources");
}

#[test]
fn gpio_output_queue_orders_each_bank_and_ignores_disabled_and_nonexistent_pins() {
    let mut g = Gpio::new();
    // Include pin 63 in the upper enable/output aliases; it is stored but has no edge.
    g.write(0x24, (1 << 0) | (1 << 5) | (1 << 31));
    g.write(0x30, (1 << 0) | (1 << 16) | (1 << 31));
    g.write(0x08, u32::MAX);
    g.write(0x14, u32::MAX);
    g.write(0x08, u32::MAX); // Repeating the same levels does not append edges.
    g.write(0x14, u32::MAX);
    g.write(0x0c, u32::MAX);
    g.write(0x18, u32::MAX);
    assert_eq!(g.changes, vec![
        (0, true), (5, true), (31, true), (32, true), (48, true),
        (0, false), (5, false), (31, false), (32, false), (48, false),
    ]);
    g.changes.clear();
    g.write(0x34, 1 << 16);
    g.write(0x14, (1 << 16) | (1 << 31));
    assert!(g.changes.is_empty());
    // Enabling an already-high output drives the line high, which the board sees as a rising
    // edge (see `gpio_enable_toggle_reports_the_edge`); the output transition is the falling one.
    g.write(0x30, 1 << 16);
    assert_eq!(g.changes, vec![(48, true)]);
    g.changes.clear();
    g.write(0x18, (1 << 16) | (1 << 31));
    assert_eq!(g.changes, vec![(48, false)]);
}

/// A level produced by toggling the output *enable* is an edge the board must see: IDF 5.5's
/// esp_lcd drives the LCD D/C line this way (level, enable, transfer, disable), and a model that
/// only compared `out` never reported it — every byte reached the panel as a command.
#[test]
fn gpio_enable_toggle_reports_the_edge() {
    let mut g = Gpio::new();
    g.write(0x8, 1 << 15);                       // OUT_W1TS: level 1, driver off — nothing visible yet
    assert!(g.changes.is_empty(), "a level with the driver disabled is not an edge");
    g.write(0x24, 1 << 15);                      // ENABLE_W1TS: the driver comes on at level 1
    assert_eq!(g.changes, vec![(15, true)], "enabling the driver with out=1 is a rising edge");
    g.changes.clear();
    g.write(0x28, 1 << 15);                      // ENABLE_W1TC: released
    assert_eq!(g.changes, vec![(15, false)], "releasing the driver is a falling edge");
    g.changes.clear();
    g.write(0xc, 1 << 15); g.write(0x24, 1 << 15);   // level 0, then enable: visible 0 -> 0
    assert!(g.changes.is_empty(), "enabling at level 0 after a release changes nothing visible");
    g.write(0x8, 1 << 15);                       // out goes to 1 while enabled: the usual edge still works
    assert_eq!(g.changes, vec![(15, true)]);
}

/// Host input is USB: packets of at most 64 bytes, one RECV_PKT interrupt each. A driver that
/// drains one packet per interrupt (Arduino's HWCDC) must be handed the rest of a longer line
/// as further packets, or the line's tail — and its newline — is stranded in the FIFO.
#[test]
fn usb_serial_host_input_arrives_as_64_byte_packets_with_an_interrupt_each() {
    use esp_periph::usb_serial_jtag::USB_PACKET;
    let mut u = UsbSerialJtag::new(240_000_000);
    let line: Vec<u8> = (0..150u8).collect();
    u.host_input(&line);
    let drain = |u: &mut UsbSerialJtag| -> Vec<u8> {
        // the driver: see RECV_PKT, clear it, read up to 64 bytes while the FIFO says data is there
        assert!(u.read(0x8) & (1 << 2) != 0, "RECV_PKT raised for this packet");
        u.write(0x14, 1 << 2);
        let mut got = Vec::new();
        while got.len() < USB_PACKET && u.read(0x4) & (1 << 2) != 0 { got.push(u.read(0x0) as u8); }
        got
    };
    let a = drain(&mut u); assert_eq!(a, &line[..64], "first packet is the first 64 bytes");
    assert!(u.read(0x8) & (1 << 2) != 0, "the next packet re-raises RECV_PKT after the first is read out");
    let b = drain(&mut u); assert_eq!(b, &line[64..128]);
    let c = drain(&mut u); assert_eq!(c, &line[128..], "the last packet is the 22-byte tail");
    assert!(u.read(0x8) & (1 << 2) == 0 && u.read(0x4) & (1 << 2) == 0, "nothing left");
    // a short line is one packet, as before
    u.host_input(b"{\"action\":\"play_sid\",\"value\":\"0\"}\n");
    assert_eq!(drain(&mut u).len(), 34);
}

// ------------------------------------------------------------------ rng
/// The RISC-V chips' `WDEV_RND_REG`: the xorshift32 sequence from the default seed is the one
/// the C3 and C6 produced before the model was shared, and the cycle count the chip stamps into
/// `now` is added to each read, not mixed into the state.
#[test]
fn rng_sequence_from_the_default_seed_plus_the_cycle_count() {
    use esp_periph::Rng;
    let mut r = Rng::new();
    let first: Vec<u32> = (0..4).map(|_| Device::read(&mut r, 0)).collect();
    assert_eq!(first, [0xe124_b63a, 0x8b9a_74ab, 0x64e1_b3ac, 0x0017_4626]);
    assert_eq!(Device::write(&mut r, 0, 0xdead_beef), WriteEffect::NONE, "writes are ignored");

    let mut stamped = Rng::new();
    stamped.now = 1000;
    assert_eq!(Device::read(&mut stamped, 0x3), 0xe124_b63au32.wrapping_add(1000));
    stamped.now = 0;
    assert_eq!(Device::read(&mut stamped, 0), 0x8b9a_74ab, "the stamp never entered the state");

    assert_eq!(Device::read(&mut Rng::with_seed(0x2545_f491), 0), 0xe124_b63a, "with_seed(DEFAULT_SEED) is new()");
    assert_eq!(Device::read(&mut Rng::default(), 0), 0xe124_b63a);
}

#[derive(Default)]
struct OptionalProbe { active: bool, stop: bool, irq: u64, polls: std::cell::Cell<u32>, ticks: u64 }
impl Device for OptionalProbe {
    fn read(&mut self, _off: u32) -> u32 { 0 }
    fn write(&mut self, _off: u32, v: u32) -> WriteEffect {
        self.active = v & 1 != 0; self.stop = v & 2 != 0; self.irq = 0; WriteEffect::NONE
    }
    fn clock(&self) -> Option<ClockDomain> {
        self.polls.set(self.polls.get() + 1);
        if self.active { Some(ClockDomain::Apb) } else { None }
    }
    fn irq_sources(&self) -> u64 { self.polls.set(self.polls.get() + 1); self.irq }
    fn tick(&mut self, ticks: u64) { self.ticks += ticks; self.irq = 1; if self.stop { self.active = false; } }
}
struct OptionalChip { pwm: OptionalProbe, other: Probe, misc: Misc, clock: ClockTree<1> }
device_set! { OptionalChip; clock: (clock) 240_000_000, [(ClockDomain::Apb, 3)];
    0x01 "PWM" optional (pwm) => [40];
    0x02 "OTHER" (other) => [7];
}
impl DeviceSet for OptionalChip {
    const BASE: u32 = BASE;
    fn block_name(_block: u32) -> &'static str { "PWM" }
    fn misc(&self) -> &Misc { &self.misc }
    fn misc_mut(&mut self) -> &mut Misc { &mut self.misc }
}

#[test]
fn optional_devices_are_not_polled_until_configured_and_keep_stopped_irqs() {
    let mut c = OptionalChip { pwm: OptionalProbe::default(), other: Probe::default(), misc: Misc::new(), clock: OptionalChip::new_clock() };
    for _ in 0..10 {
        assert!(!Dispatch::tick(&mut c, 15));
        assert_eq!(c.source_status(), [0; 4]);
        assert_eq!(c.cycles_until_deadline(), u32::MAX);
    }
    mmio::write32(&mut c, BASE + 0x2000, 1);
    c.other.irq = 1;
    assert_eq!(c.source_status(), [1 << 7, 0, 0, 0]);
    c.other.irq = 0;
    assert_eq!(c.pwm.polls.get(), 0, "ordinary writes and IRQ scans do not poll optional devices");
    assert_eq!(c.pwm.ticks, 0);
    mmio::write32(&mut c, BASE + 0x1000, 1);
    assert_eq!(c.misc.active_optional, [1]);
    c.other.domain = Some(ClockDomain::Apb);
    assert!(Dispatch::tick(&mut c, 15));
    assert_eq!((c.pwm.ticks, c.other.ticks), (5, 5), "both devices receive the same clock delta");
    assert_eq!(c.source_status(), [0, 1 << 8, 0, 0]);
    mmio::write32(&mut c, BASE + 0x1000, 3);
    assert_eq!(c.misc.active_optional, [1], "no duplicate registration");
    assert_eq!(c.source_status(), [0; 4], "write cleared the cached interrupt");
    assert!(Dispatch::tick(&mut c, 15));
    assert!(c.misc.active_optional.is_empty(), "one-shot stopped");
    let polls = c.pwm.polls.get();
    assert!(!Dispatch::tick(&mut c, 15));
    assert_eq!(c.source_status(), [0, 1 << 8, 0, 0], "stopping preserves an asserted interrupt");
    assert_eq!(c.pwm.polls.get(), polls, "stopped device is not polled");
    mmio::write32(&mut c, BASE + 0x1000, 0);
    assert_eq!(c.source_status(), [0; 4]);
}
