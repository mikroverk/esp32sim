use emu_core::Bus;
use esp_periph::Device;
use esp_periph::i2c::{I2c, I2cDevice, Reg8Device, INT_END_DETECT, INT_NACK, INT_TRANS_COMPLETE};
use esp_soc::{BoardModel, SocBus};
use std::sync::{Arc, Mutex};

#[derive(Default, Debug)]
struct Seen {
    writes: Vec<u8>,
    stops: usize,
    resets: usize,
}

// VL53L1X-style 16-bit registers: address at 0x0001, identity at 0x010f.
struct Sensor {
    addr: u8,
    pins: Option<(u8, u8)>,
    general: bool,
    reject: bool,
    nack_data: bool,
    call: bool,
    phase: u8,
    reg: u16,
    seen: Arc<Mutex<Seen>>,
}
impl I2cDevice for Sensor {
    fn address(&self, _: u8) -> u8 {
        self.addr
    }
    fn start_address(&mut self, _: u8, addr: u8, read: bool) -> bool {
        if !(addr == self.addr || (self.general && addr == 0 && !read)) {
            return false;
        }
        self.call = addr == 0;
        if !read {
            self.phase = 0;
        }
        !self.reject
    }
    fn pins(&self) -> Option<(u8, u8)> {
        self.pins
    }
    fn write(&mut self, b: u8) -> bool {
        let mut seen = self.seen.lock().unwrap();
        seen.writes.push(b);
        if self.call {
            if b == 6 {
                seen.resets += 1;
            }
            return b == 6 && !self.nack_data;
        }
        match self.phase {
            0 => {
                self.reg = u16::from(b) << 8;
                self.phase = 1;
            }
            1 => {
                self.reg |= u16::from(b);
                self.phase = 2;
            }
            _ => {
                if self.reg == 1 {
                    self.addr = b & 0x7f;
                }
                self.reg += 1;
            }
        }
        true
    }
    fn read(&mut self) -> u8 {
        if self.reg == 0x010f {
            0xea
        } else {
            0xff
        }
    }
    fn stop(&mut self) {
        self.seen.lock().unwrap().stops += 1;
    }
}
fn sensor(addr: u8, pins: Option<(u8, u8)>, general: bool) -> (Sensor, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    (
        Sensor {
            addr,
            pins,
            general,
            reject: false,
            nack_data: false,
            call: false,
            phase: 0,
            reg: 0,
            seen: seen.clone(),
        },
        seen,
    )
}
fn commands(write: &mut impl FnMut(u32, u32), bytes: &[u8], ops: &[(u32, u32)]) {
    write(0x24, u32::MAX);
    write(0x18, 3 << 12);
    for &b in bytes {
        write(0x1c, u32::from(b));
    }
    for (i, &(op, n)) in ops.iter().enumerate() {
        write(0x58 + i as u32 * 4, (op << 11) | (1 << 8) | n);
    }
    write(4, 1 << 5);
}
fn send(i2c: &mut I2c, bytes: &[u8], stop: bool) -> bool {
    commands(
        &mut |o, v| i2c.write(o, v),
        bytes,
        &[
            (6, 0),
            (1, bytes.len() as u32),
            (if stop { 2 } else { 4 }, 0),
        ],
    );
    i2c.tick(100_000);
    i2c.int_raw & INT_NACK == 0
}

#[test]
fn shared_address_change_replacement_and_pin_matching() {
    let mut i2c = I2c::new();
    let (a, seen) = sensor(0x29, Some((8, 9)), false);
    let (other, untouched) = sensor(0x29, Some((6, 7)), false);
    i2c.attach(0x29, Box::new(a));
    i2c.attach(0x29, Box::new(other));
    i2c.set_pins(Some((8, 9)));
    assert!(send(&mut i2c, &[0x52, 0, 1, 0x30, 0xaa], true));
    assert_eq!(seen.lock().unwrap().writes, [0, 1, 0x30, 0xaa]);
    assert!(untouched.lock().unwrap().writes.is_empty());
    assert!(!send(&mut i2c, &[0x52], true));
    assert!(send(&mut i2c, &[0x60], true));
    assert!(i2c.has_device(0x30));
    // Replacement at the current address and same pins must leave only the new device.
    let (replacement, replacement_seen) = sensor(0x30, Some((8, 9)), false);
    i2c.attach(0x30, Box::new(replacement));
    assert!(send(&mut i2c, &[0x60, 1, 15], true));
    assert_eq!(replacement_seen.lock().unwrap().writes, [1, 15]);
    assert_eq!(seen.lock().unwrap().writes, [0, 1, 0x30, 0xaa]);
    assert!(i2c.detach(0x30).is_some());
    assert!(!send(&mut i2c, &[0x60], true));
    i2c.set_pins(Some((6, 7)));
    assert!(send(&mut i2c, &[0x52], true));
}

#[test]
fn shared_general_call_fanout_ack_lifetime_and_filtering() {
    let mut i2c = I2c::new();
    let (mut a, first) = sensor(0x58, Some((8, 9)), true);
    a.nack_data = true;
    let (b, second) = sensor(0x59, None, true);
    let (wrong, wrong_seen) = sensor(0x58, Some((6, 7)), true);
    let (mut rejecting, rejected) = sensor(0x5a, None, true);
    rejecting.reject = true;
    i2c.attach(0x58, Box::new(a));
    i2c.attach(0x59, Box::new(b));
    i2c.attach(0x58, Box::new(wrong));
    i2c.attach(0x5a, Box::new(rejecting));
    i2c.attach(0x60, Box::new(Reg8Device::new("ordinary", &[])));
    i2c.set_pins(Some((8, 9)));
    assert!(send(&mut i2c, &[0, 6], true));
    for seen in [&first, &second] {
        let seen = seen.lock().unwrap();
        assert_eq!((seen.resets, seen.stops), (1, 1));
    }
    assert!(wrong_seen.lock().unwrap().writes.is_empty());
    assert!(rejected.lock().unwrap().writes.is_empty());
    assert!(!send(&mut i2c, &[1], true)); // General-call reads are not accepted.
    assert!(!send(&mut i2c, &[0, 7], true)); // No data ACK.
    assert!(send(&mut i2c, &[0], false)); // END retains both recipients.
    i2c.detach(0x58);
    commands(&mut |o, v| i2c.write(o, v), &[6], &[(1, 1), (2, 0)]);
    i2c.tick(100_000);
    assert_eq!(second.lock().unwrap().resets, 2);
    assert_eq!(first.lock().unwrap().resets, 1);
    assert!(send(&mut i2c, &[0], false));
    i2c.set_pins(Some((6, 7)));
    commands(&mut |o, v| i2c.write(o, v), &[6], &[(1, 1), (2, 0)]);
    i2c.tick(100_000);
    assert_ne!(i2c.int_raw & INT_NACK, 0);
    i2c.clear_devices();
    assert!(!send(&mut i2c, &[0, 6], true));
}

struct Board;
impl BoardModel for Board {
    fn name(&self) -> &'static str {
        "i2c-address-test"
    }
    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn I2cDevice>)> {
        vec![
            (0, 0x29, Box::new(sensor(0x29, None, false).0)),
            (0, 0x58, Box::new(sensor(0x58, None, true).0)),
        ]
    }
}
fn chip(bus: &mut impl SocBus, base: u32) {
    for (bytes, ack) in [
        (&[0x52, 0, 1, 0x30][..], true),
        (&[0x52][..], false),
        (&[0x60, 1, 15][..], true),
        (&[0, 6][..], true),
        (&[1][..], false),
    ] {
        commands(
            &mut |o, v| bus.write32(base + o, v).unwrap(),
            bytes,
            &[(6, 0), (1, bytes.len() as u32), (2, 0)],
        );
        bus.tick(100_000);
        assert_eq!(bus.read32(base + 0x20).unwrap() & INT_NACK == 0, ack);
    }
    commands(
        &mut |o, v| bus.write32(base + o, v).unwrap(),
        &[0x61],
        &[(6, 0), (1, 1), (3, 1), (2, 0)],
    );
    bus.tick(100_000);
    assert_eq!(bus.read32(base + 0x1c).unwrap(), 0xea);
}
#[test]
fn s3_address_and_general_call() {
    let mut m = esp32s3::machine([0; 6]);
    m.bus.board = Box::new(Board);
    m.bus.attach_board_devices();
    chip(&mut m.bus, 0x60013000);
}
#[test]
fn c3_address_and_general_call() {
    let mut m = esp32c3::machine([0; 6], 4 << 20);
    m.bus.board = Box::new(Board);
    m.bus.attach_board_devices();
    chip(&mut m.bus, 0x60013000);
}
#[test]
fn c6_address_and_general_call() {
    let mut m = esp32c6::machine([0; 6], 4 << 20);
    m.bus.board = Box::new(Board);
    m.bus.attach_board_devices();
    chip(&mut m.bus, 0x60004000);
}
// Register-level PCA9685 fixture; PWM timing and output pins are outside this bus test.
struct Pca9685 {
    regs: [u8; 256],
    starts: Vec<(u8, bool)>,
    reject: bool,
    resets: usize,
}
impl Default for Pca9685 {
    fn default() -> Self {
        let mut regs = [0; 256];
        regs[0] = 0x11;
        regs[1] = 4;
        regs[2..6].copy_from_slice(&[0xe2, 0xe4, 0xe8, 0xe0]);
        regs[0xfe] = 30;
        Self {
            regs,
            starts: Vec::new(),
            reject: false,
            resets: 0,
        }
    }
}
struct PcaDevice {
    state: Arc<Mutex<Pca9685>>,
    pins: (u8, u8),
    pointer: u8,
    first: bool,
    reset: bool,
}
impl I2cDevice for PcaDevice {
    fn pins(&self) -> Option<(u8, u8)> {
        Some(self.pins)
    }
    fn start_address(&mut self, configured: u8, addr: u8, read: bool) -> bool {
        let mut s = self.state.lock().unwrap();
        if !(addr == configured
            || (addr == 0 && !read)
            || (0..4).any(|n| {
                s.regs[0] & (1 << n) != 0 && addr == s.regs[if n == 0 { 5 } else { 5 - n }] >> 1
            }))
        {
            return false;
        }
        s.starts.push((addr, read));
        self.first = !read;
        self.reset = addr == 0;
        !s.reject
    }
    fn write(&mut self, byte: u8) -> bool {
        let mut s = self.state.lock().unwrap();
        if self.reset {
            if byte != 6 {
                return false;
            }
            s.regs = Pca9685::default().regs;
            s.resets += 1;
        } else if self.first {
            self.pointer = byte;
            self.first = false;
        } else {
            s.regs[self.pointer as usize] = byte;
            if s.regs[0] & 0x20 != 0 {
                self.pointer = self.pointer.wrapping_add(1);
            }
        }
        true
    }
    fn read(&mut self) -> u8 {
        let s = self.state.lock().unwrap();
        let byte = s.regs[self.pointer as usize];
        if s.regs[0] & 0x20 != 0 {
            self.pointer = self.pointer.wrapping_add(1);
        }
        byte
    }
}
fn pca(state: &Arc<Mutex<Pca9685>>, pins: (u8, u8)) -> Box<dyn I2cDevice> {
    Box::new(PcaDevice {
        state: state.clone(),
        pins,
        pointer: 0,
        first: false,
        reset: false,
    })
}

#[test]
fn shared_programmable_aliases_and_all_call() {
    let mut i2c = I2c::new();
    let a = Arc::new(Mutex::new(Pca9685::default()));
    let b = Arc::new(Mutex::new(Pca9685::default()));
    let wrong = Arc::new(Mutex::new(Pca9685::default()));
    i2c.attach(0x40, pca(&a, (8, 9)));
    i2c.attach(0x41, pca(&b, (8, 9)));
    i2c.attach(0x40, pca(&wrong, (6, 7)));
    i2c.set_pins(Some((8, 9)));
    assert!(send(&mut i2c, &[0xe0, 8, 0x55], true));
    assert_eq!(a.lock().unwrap().regs[8], 0x55);
    assert_eq!(
        b.lock().unwrap().regs[8],
        0x55,
        "all-call must reach the second device"
    );
    assert!(wrong.lock().unwrap().starts.is_empty());
    // Each programmable subaddress is consulted afresh after register writes.
    for (reg, bit, addr) in [(2, 8, 0x72), (3, 4, 0x73), (4, 2, 0x74), (5, 1, 0x75)] {
        assert!(send(&mut i2c, &[0x80, 0, 0], true));
        assert!(send(&mut i2c, &[0x80, reg, addr << 1], true));
        assert!(!send(&mut i2c, &[addr << 1], true));
        assert!(send(&mut i2c, &[0x80, 0, bit], true));
        assert!(send(&mut i2c, &[addr << 1, 8, addr], true));
        assert_eq!(a.lock().unwrap().regs[8], addr);
        assert_eq!(a.lock().unwrap().starts.last(), Some(&(addr, false)));
        assert_eq!(b.lock().unwrap().regs[8], 0x55);
    }
    assert!(send(&mut i2c, &[0, 6], true));
    assert_eq!(a.lock().unwrap().resets, 1);
    assert_eq!(b.lock().unwrap().resets, 1);
    assert!(send(&mut i2c, &[0x80, 8, 0xf0], true));
    assert!(send(&mut i2c, &[0x82, 8, 0x5a], true));
    // Repeated-start reads notify both recipients and resolve their open-drain data.
    commands(
        &mut |o, v| i2c.write(o, v),
        &[0xe0, 8, 0xe1],
        &[(6, 0), (1, 2), (6, 0), (1, 1), (3, 1), (2, 0)],
    );
    i2c.tick(100_000);
    assert_eq!(i2c.read(0x1c), 0x50);
    for state in [&a, &b] {
        assert_eq!(state.lock().unwrap().starts.last(), Some(&(0x70, true)));
    }
    // A matching device can refuse this transaction without hiding a later ACK.
    a.lock().unwrap().reject = true;
    assert!(send(&mut i2c, &[0xe0, 8, 0x33], true));
    assert_eq!(a.lock().unwrap().regs[8], 0xf0);
    assert_eq!(b.lock().unwrap().regs[8], 0x33);
    a.lock().unwrap().reject = false;
    assert!(send(&mut i2c, &[0x80, 0, 0], true));
    assert!(send(&mut i2c, &[0x82, 0, 0], true));
    assert!(!send(&mut i2c, &[0xe0], true));
    assert!(send(&mut i2c, &[0x80], true));
    assert!(!send(&mut i2c, &[1], true));
    assert!(i2c.has_device(0x40));
    assert!(!i2c.has_device(0x70)); // Aliases do not create extra attachments.
}

#[test]
fn default_address_match_tracks_device_state_and_default_start_can_nack() {
    struct Moving { address: u8 }
    impl I2cDevice for Moving {
        fn address(&self, _: u8) -> u8 { self.address }
        fn start(&mut self, read: bool) -> bool { !read }
        fn write(&mut self, value: u8) -> bool { self.address = value; true }
        fn read(&mut self) -> u8 { 0 }
    }
    let mut i2c = I2c::new();
    i2c.attach(0x20, Box::new(Moving { address: 0x30 }));
    assert!(!send(&mut i2c, &[0x40], true));
    assert!(send(&mut i2c, &[0x60, 0x31], true));
    assert!(!send(&mut i2c, &[0x60], true));
    assert!(send(&mut i2c, &[0x62], true));
    assert!(!send(&mut i2c, &[0x63], true));
    assert!(i2c.has_device(0x31));
}

const TIMING_REGISTERS: [(u32, u32); 7] = [
    (0, 199),
    (0x38, 200),
    (0x40, 199),
    (0x44, 199),
    (0x48, 199),
    (0x4c, 199),
    (0x54, 0),
];

fn configure(i2c: &mut I2c) {
    for (off, value) in TIMING_REGISTERS {
        i2c.write(off, value);
    }
}
#[test]
fn byte_deadlines_delay_fifo_callbacks_and_interrupts() {
    let mut i2c = I2c::new();
    configure(&mut i2c);
    i2c.attach(0x42, Box::new(Reg8Device::new("timed", &[(0, 0xa5)])));
    i2c.write(0x28, INT_TRANS_COMPLETE);
    commands(&mut |o, v| i2c.write(o, v), &[0x85], &[(6, 0), (1, 1), (3, 1), (2, 0)]);
    assert_eq!(
        i2c.read(8) & 16,
        16,
        "TRANS_START must leave the controller busy"
    );
    assert_eq!(i2c.int_raw, 0);
    assert!(!i2c.irq());
    assert_eq!(i2c.next_deadline(), Some(800));
    i2c.tick(799);
    assert_eq!(i2c.read(0x58) >> 31, 0);
    i2c.tick(1);
    assert_eq!(i2c.read(0x58) >> 31, 1);
    assert_eq!(i2c.next_deadline(), Some(7200));
    i2c.tick(7200);
    assert_eq!(i2c.read(8) >> 8 & 63, 0);
    i2c.tick(7199);
    assert_eq!(i2c.read(8) >> 8 & 63, 0);
    i2c.tick(1);
    assert_eq!(i2c.read(0x1c), 0xa5);
    i2c.tick(799);
    assert_eq!(i2c.int_raw, 0);
    i2c.tick(1);
    assert_eq!(i2c.int_raw, INT_TRANS_COMPLETE);
    assert!(i2c.irq());
    assert_eq!(i2c.read(8) & 16, 0);
    assert_eq!(i2c.next_deadline(), None);
}
#[test]
fn timed_nack_end_continuation_and_reset() {
    let mut i2c = I2c::new();
    configure(&mut i2c);
    commands(&mut |o, v| i2c.write(o, v), &[0x84], &[(6, 0), (1, 1), (2, 0)]);
    i2c.tick(7999);
    assert_eq!(i2c.int_raw, 0);
    i2c.tick(1);
    assert_eq!(i2c.int_raw, INT_NACK);
    i2c.attach(0x42, Box::new(Reg8Device::new("timed", &[(0, 42)])));
    commands(&mut |o, v| i2c.write(o, v), &[0x85], &[(6, 0), (1, 1), (4, 0)]);
    i2c.tick(8000);
    assert_eq!(i2c.int_raw, 0);
    i2c.tick(1);
    assert_eq!(i2c.int_raw, INT_END_DETECT);
    commands(&mut |o, v| i2c.write(o, v), &[], &[(3, 1), (2, 0)]);
    i2c.tick(8000);
    assert_eq!(i2c.read(0x1c), 42);
    assert_eq!(i2c.int_raw, INT_TRANS_COMPLETE);
    commands(&mut |o, v| i2c.write(o, v), &[0x85], &[(6, 0), (1, 1), (3, 1), (2, 0)]);
    i2c.tick(400);
    i2c.write(4, 1 << 10);
    i2c.tick(100000);
    assert_eq!(i2c.int_raw, 0);
    assert_eq!(i2c.next_deadline(), None);
    assert!(i2c.has_device(0x42));
}

#[test]
fn dividers_and_repeated_start() {
    for (clock, expected_byte) in [
        (0, 7200),
        (1 | (2 << 8) | (1 << 14), 18000),
        (1 << 20, 16458),
    ] {
        let mut i2c = I2c::new();
        configure(&mut i2c);
        i2c.write(0x54, clock);
        i2c.attach(0x42, Box::new(Reg8Device::new("timing", &[(0, 42)])));
        let start = 6;
        let stop = 2;
        commands(
            &mut |o, v| i2c.write(o, v),
            &[0x84, 0x85],
            &[(start, 0), (1, 1), (start, 0), (1, 1), (stop, 0)],
        );
        let setup = i2c.next_deadline().unwrap();
        i2c.tick(setup);
        assert_eq!(i2c.next_deadline(), Some(expected_byte));
        i2c.tick(expected_byte);
        assert_eq!(i2c.next_deadline(), Some(setup));
        i2c.tick(setup);
        i2c.tick(expected_byte);
        assert_eq!(i2c.int_raw, 0);
        i2c.tick(setup);
        assert_eq!(i2c.int_raw, INT_TRANS_COMPLETE);
    }
}

struct ClockProbe(Arc<Mutex<(u64, Vec<u64>)>>);
impl I2cDevice for ClockProbe {
    fn start(&mut self, _: bool) -> bool {
        let mut s = self.0.lock().unwrap();
        let now = s.0;
        s.1.push(now);
        true
    }
    fn write(&mut self, _: u8) -> bool {
        true
    }
    fn read(&mut self) -> u8 {
        42
    }
}
struct ProbeBoard(Arc<Mutex<(u64, Vec<u64>)>>);
impl BoardModel for ProbeBoard {
    fn name(&self) -> &'static str {
        "i2c-clock-probe"
    }
    fn advance_to(&mut self, now: u64) {
        self.0.lock().unwrap().0 = now;
    }
    fn i2c_devices(&mut self) -> Vec<(u8, u8, Box<dyn I2cDevice>)> {
        vec![(0, 0x42, Box::new(ClockProbe(self.0.clone())))]
    }
}
fn bus_timing(
    bus: &mut impl SocBus,
    base: u32,
    address_cycles: u32,
    stop_cycles: u32,
    probe: &Arc<Mutex<(u64, Vec<u64>)>>,
) {
    for (off, value) in TIMING_REGISTERS {
        bus.write32(base + off, value).unwrap();
    }
    bus.write32(base + 0x28, INT_TRANS_COMPLETE).unwrap();
    commands(&mut |o, v| bus.write32(base + o, v).unwrap(),
        &[0x84], &[(6, 0), (1, 1), (2, 0)]);
    assert_eq!(bus.read32(base + 0x2c).unwrap(), 0);
    bus.tick(address_cycles - 1);
    assert_eq!(bus.read32(base + 0x2c).unwrap(), 0);
    assert!(probe.lock().unwrap().1.is_empty());
    bus.tick(1);
    bus.read32(base + 8).unwrap();
    assert_eq!(probe.lock().unwrap().1, [u64::from(address_cycles)]);
    bus.tick(stop_cycles - 1);
    assert_eq!(bus.read32(base + 0x2c).unwrap(), 0);
    bus.tick(1);
    assert_eq!(bus.read32(base + 0x2c).unwrap(), INT_TRANS_COMPLETE);
}
#[test]
fn timed_callbacks_and_interrupt_status_on_every_chip() {
    let probe = Arc::new(Mutex::new((0, Vec::new())));
    let mut s3 = esp32s3::machine([0; 6]);
    s3.bus.board = Box::new(ProbeBoard(probe.clone()));
    s3.bus.attach_board_devices();
    bus_timing(&mut s3.bus, 0x60013000, 24000, 2400, &probe);
    *probe.lock().unwrap() = (0, Vec::new());
    let mut c3 = esp32c3::machine([0; 6], 4 << 20);
    c3.bus.board = Box::new(ProbeBoard(probe.clone()));
    c3.bus.attach_board_devices();
    bus_timing(&mut c3.bus, 0x60013000, 16000, 1600, &probe);
    // PCR integer, fractional and RC_FAST sources; periods stay unchanged.
    for (clock, address, stop) in [
        (1 << 12, 32000, 3200),
        ((1 << 12) | 2 | (1 << 6), 40000, 4000),
        (1 << 20, 36574, 3658),
    ] {
        *probe.lock().unwrap() = (0, Vec::new());
        let mut c6 = esp32c6::machine([0; 6], 8 << 20);
        c6.bus.board = Box::new(ProbeBoard(probe.clone()));
        c6.bus.attach_board_devices();
        c6.bus.write32(0x60096024, clock).unwrap();
        bus_timing(&mut c6.bus, 0x60004000, address, stop, &probe);
    }
}

#[test]
fn busy_start_empty_commands_and_command_limit() {
    let mut i2c = I2c::new();
    assert_eq!(i2c.clock(), None);
    configure(&mut i2c);
    commands(&mut |o, v| i2c.write(o, v), &[], &[(6, 0), (1, 0), (3, 0), (4, 0)]);
    i2c.tick(400);
    i2c.write(4, 1 << 5);
    assert_eq!(i2c.transactions, 1);
    assert_eq!(i2c.next_deadline(), Some(400));
    i2c.tick(400);
    i2c.tick(3);
    assert_eq!(i2c.int_raw, INT_END_DETECT);
    assert_eq!(i2c.read(8) >> 8 & 63, 0);
    assert_eq!(i2c.clock(), None);
    commands(&mut |o, v| i2c.write(o, v), &[], &[(6, 0); 8]);
    i2c.tick(6400);
    assert_eq!(i2c.next_deadline(), None);
    assert_eq!(i2c.int_raw, 0);
    assert_eq!(i2c.read(0x74) >> 31, 1);
}

#[test]
fn wait_high_and_timing_fields_are_masked() {
    let mut i2c = I2c::new();
    configure(&mut i2c);
    i2c.write(0, 0xffff_fe00 | 199);
    i2c.write(0x38, 0xffff_0000 | (10 << 9) | 200);
    i2c.write(0x40, 0xffff_fe00 | 199);
    i2c.write(0x44, 0xffff_fe00 | 199);
    commands(&mut |o, v| i2c.write(o, v), &[0x84], &[(6, 0), (1, 1), (2, 0)]);
    assert_eq!(i2c.next_deadline(), Some(800));
    i2c.tick(800);
    assert_eq!(i2c.next_deadline(), Some(7380));
}

#[test]
fn controller_leaves_optional_dispatch_after_completion_and_reset() {
    use esp_periph::DeviceSet;
    let mut m = esp32c3::machine([0; 6], 4 << 20);
    let b = &mut m.bus;
    assert!(!b.periph.misc().active_optional.contains(&0x13));
    assert!(!b.periph.i2c.has_deadline());
    assert_eq!(b.periph.i2c.next_deadline(), None);
    for (off, value) in [(0x28, INT_TRANS_COMPLETE), (0x58, 2 << 11), (4, 1 << 5)] {
        b.write32(0x60013000 + off, value).unwrap();
    }
    assert!(b.periph.misc().active_optional.contains(&0x13));
    assert!(b.periph.i2c.has_deadline());
    assert!(b.periph.i2c.next_deadline().is_some());
    b.tick(100);
    assert!(!b.periph.misc().active_optional.contains(&0x13));
    assert!(!b.periph.i2c.has_deadline());
    assert_eq!(b.periph.i2c.next_deadline(), None);
    assert_ne!(b.periph.source_status()[0] & (1 << esp32c3::periph::src::I2C_EXT0), 0);
    b.write32(0x60013024, u32::MAX).unwrap();
    assert_eq!(b.periph.source_status()[0] & (1 << esp32c3::periph::src::I2C_EXT0), 0);
    b.write32(0x60013004, 1 << 5).unwrap();
    assert!(b.periph.misc().active_optional.contains(&0x13));
    assert!(b.periph.i2c.has_deadline());
    assert!(b.periph.i2c.next_deadline().is_some());
    b.write32(0x60013004, 1 << 10).unwrap();
    assert!(!b.periph.misc().active_optional.contains(&0x13));
    assert!(!b.periph.i2c.has_deadline());
    assert_eq!(b.periph.i2c.next_deadline(), None);
}

#[test]
fn c3_i2c_tick_delivers_edges_after_consuming_board_deadline() {
    struct EdgeBoard { pending: bool, edges: Vec<esp_soc::board::BoardEdge> }
    impl BoardModel for EdgeBoard {
        fn name(&self) -> &'static str { "i2c-edge" }
        fn input_levels(&self) -> Vec<(u8, bool)> { vec![(5, false)] }
        fn next_deadline(&self) -> Option<u64> { self.pending.then_some(100) }
        fn advance_to(&mut self, cycle: u64) {
            if self.pending && cycle >= 100 {
                self.pending = false;
                self.edges.push(esp_soc::board::BoardEdge { cycle: 100, pin: 5, level: true });
            }
        }
        fn take_edges(&mut self) -> Vec<esp_soc::board::BoardEdge> { std::mem::take(&mut self.edges) }
    }
    let mut m = esp32c3::machine([0; 6], 4 << 20);
    m.bus.board = Box::new(EdgeBoard { pending: true, edges: Vec::new() });
    m.bus.attach_board_devices();
    m.bus.write32(0x60013058, 2 << 11).unwrap();
    m.bus.write32(0x60013004, 1 << 5).unwrap();
    m.bus.tick(100);
    assert_eq!(m.bus.board.next_deadline(), None);
    assert_ne!(m.bus.gpio_input() & (1 << 5), 0);
}

#[test]
fn pocket_tank_amoled_transfers_complete_at_programmed_bus_rate() {
    let mut board = esp32s3::board::waveshare_amoled18::WaveshareAmoled18V2::new();
    let mut i2c = I2c::new();
    for (bus, address, device) in board.i2c_devices() {
        assert_eq!(bus, 0);
        i2c.attach(address, device);
    }
    // Pocket Tank's IDF v5.4.1 register writes: 40 MHz / 100 = 400 kHz,
    // or / 400 = 100 kHz for probes; i2c_ll.h:175-201 programs these fields.
    for (slow, bytes, received) in [
        (false, &[0x40, 0x03, 0x78][..], &[] as &[u8]),
        (false, &[0x40, 0x01, 0x80], &[]),
        (false, &[0x40, 0x01, 0x87], &[]),
        (true, &[0x2a], &[]),
        (false, &[0x2a, 0xa7, 0x2b], &[0xb7]),
        (true, &[0x68], &[]),
        (true, &[0xd6], &[]),
        (false, &[0xd6, 0x00, 0xd7], &[0x05]),
        (false, &[0xd6, 0x60, 0xb0], &[]),
        (false, &[0xd6, 0x02, 0x40], &[]),
        (false, &[0xd6, 0x03, 0x08], &[]),
        (false, &[0xd6, 0x08, 0x01], &[]),
        (false, &[0xd6, 0x02, 0xd7], &[0x40]),
        (false, &[0xd6, 0x03, 0xd7], &[0x08]),
        (false, &[0xd6, 0x08, 0xd7], &[0x01]),
        (false, &[0xa2, 0x04, 0xa3], &[0, 0, 0, 0, 0, 0, 0]),
        (false, &[0xd6, 0x35, 0xd7], &[0, 0, 0, 0, 0, 0]),
    ] {
        let values = if slow { [199, 50278, 199, 199, 199, 199, 1 << 21] }
                     else { [49, 11803, 49, 49, 49, 49, 1 << 21] };
        for ((off, _), value) in TIMING_REGISTERS.into_iter().zip(values) {
            i2c.write(off, value);
        }
        let ops = if received.is_empty() { vec![(6, 0), (1, bytes.len() as u32), (2, 0)] }
                  else { vec![(6, 0), (1, 2), (6, 0), (1, 1), (3, received.len() as u32), (2, 0)] };
        commands(&mut |o, v| i2c.write(o, v), bytes, &ops);
        let bits = 9 * (bytes.len() + received.len()) as u64;
        let edges = if received.is_empty() { 2 } else { 3 };
        let ticks = (bits + edges) * if slow { 800 } else { 200 };
        i2c.tick(ticks - 1);
        assert!(i2c.is_active(), "early completion for {bytes:x?}");
        assert_eq!(i2c.int_raw, 0, "unexpected interrupt for {bytes:x?}");
        i2c.tick(1);
        assert!(!i2c.is_active(), "late completion for {bytes:x?}");
        assert_eq!(i2c.int_raw, INT_TRANS_COMPLETE, "NACK for {bytes:x?}");
        assert_eq!(i2c.read(8) >> 8 & 63, received.len() as u32);
        for &expected in received { assert_eq!(i2c.read(0x1c), u32::from(expected)); }
        for index in 0..ops.len() { assert_ne!(i2c.read(0x58 + index as u32 * 4) & (1 << 31), 0); }
    }
}
