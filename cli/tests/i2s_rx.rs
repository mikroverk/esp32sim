use emu_core::Bus;
use esp_soc::SocBus;

fn receive(bus: &mut impl SocBus, i2s: u32, dma: u32, ram: u32, hz: u64, compact: bool, c6: bool) {
    bus.i2s_input(0).unwrap().push(&[[1234, -2345]]);
    // Host audio survives a full chip reset; peripheral/DMA state is rebuilt.
    bus.reboot([0; 6]);
    let clock = (1 << 26) | (2 << 27) | 25;
    if c6 {
        bus.write32(0x60096078, (25 << 12) | (2 << 20) | (1 << 22))
            .unwrap();
    } else {
        bus.write32(i2s + 0x30, clock).unwrap();
    }
    bus.write32(i2s + 0x28, (24 << 7) | (15 << 13) | (15 << 18) | (15 << 24))
        .unwrap();
    bus.write32(i2s + 0x50, (1 << 16) | 3).unwrap();
    bus.write32(i2s + 0x64, 4).unwrap();
    // EOF crosses two descriptors, and completes exactly at the last byte.
    for (desc, buffer, next) in [(ram, ram + 64, ram + 16), (ram + 16, ram + 66, 0)] {
        bus.write32(desc, 2 | (1 << 31)).unwrap();
        bus.write32(desc + 4, buffer).unwrap();
        bus.write32(desc + 8, next).unwrap();
    }
    let (conf1, link, select, ena, status, eof) = if compact {
        (0x74, 0x80, 0xa0, 8, 4, 0x88)
    } else {
        (4, 0x20, 0x48, 0x10, 0xc, 0x28)
    };
    bus.write32(dma + conf1, 1 << 12).unwrap();
    bus.write32(dma + select, 3).unwrap();
    bus.write32(dma + ena, 3).unwrap();
    bus.write32(dma + link, (ram & 0xfffff) | (1 << 22))
        .unwrap();
    bus.write32(i2s + 0x20, 4).unwrap();
    bus.tick((hz / 8000) as u32);
    assert_eq!(bus.read32(dma + status).unwrap() & 3, 3);
    assert_eq!(bus.read32(dma + eof).unwrap(), ram + 16);
    assert_eq!(bus.read32(ram).unwrap(), 2 | (2 << 12));
    assert_eq!(bus.read32(ram + 16).unwrap(), 2 | (2 << 12) | (1 << 30));
    assert_eq!(bus.read32(ram + 64).unwrap(), 0xf6d7_04d2);
    // CPU ownership must stop a restarted channel without overwriting its payload.
    bus.write32(dma + link, (ram & 0xfffff) | (1 << 22))
        .unwrap();
    bus.tick((hz / 8000) as u32);
    let error = if compact && !c6 { 1 << 5 } else { 1 << 3 };
    let raw = if compact { 0 } else { 8 };
    assert_ne!(bus.read32(dma + raw).unwrap() & error, 0);
    assert_eq!(bus.read32(ram + 64).unwrap(), 0xf6d7_04d2);
}

#[test]
fn pcm_to_guest_dma_on_all_chips() {
    receive(
        &mut esp32s3::machine([0; 6]).bus,
        0x6000f000,
        0x6003f000,
        0x3fc90000,
        240_000_000,
        false,
        false,
    );
    receive(
        &mut esp32c3::machine([0; 6], 4 << 20).bus,
        0x6002d000,
        0x6003f000,
        0x3fc90000,
        160_000_000,
        true,
        false,
    );
    receive(
        &mut esp32c6::machine([0; 6], 4 << 20).bus,
        0x6000c000,
        0x60080000,
        0x40810000,
        160_000_000,
        true,
        true,
    );
}

#[test]
fn receive_faults_and_ring_progress_are_bounded() {
    let mut ram = emu_core::FlatRam::new(0, 256);
    ram.write32(16, 2 | (1 << 31)).unwrap();
    ram.write32(20, 128).unwrap();
    ram.write32(24, 16).unwrap();
    let mut ch = esp_periph::GdmaInCh {
        desc: 16,
        running: true,
        ..Default::default()
    };
    assert!(ch.scatter(&mut ram, &[1, 2, 3, 4], Some(4), false, |_| false));
    assert_eq!(ch.int_raw, 3);
    assert_eq!(&ram.mem[128..130], &[3, 4]);
    // Zero-size rings fail instead of hanging, invalid buffers report an error.
    for (size, buffer) in [(0, 128), (2, 256)] {
        ram.write32(16, size | (1 << 31)).unwrap();
        ram.write32(20, buffer).unwrap();
        ch = esp_periph::GdmaInCh {
            desc: 16,
            running: true,
            ..Default::default()
        };
        assert!(!ch.scatter(&mut ram, &[1], Some(4), false, |_| false));
    }
}

#[test]
fn absent_controllers_and_s3_second_receiver() {
    assert!(esp32c3::machine([0; 6], 4 << 20).bus.i2s_input(1).is_none());
    assert!(esp32c6::machine([0; 6], 4 << 20).bus.i2s_input(1).is_none());
    assert!(esp32s3::machine([0; 6]).bus.i2s_input(2).is_none());
    let mut bus = esp32s3::machine([0; 6]).bus;
    bus.i2s_input(1).unwrap().push(&[[77, -88]]);
    bus.reboot([0; 6]);
    bus.write32(0x6002d030, (1 << 26) | (2 << 27) | 25).unwrap();
    bus.write32(0x6002d028, (24 << 7) | (15 << 13) | (15 << 18)).unwrap();
    bus.write32(0x6002d050, (1 << 16) | 3).unwrap();
    bus.write32(0x6002d064, 4).unwrap();
    bus.write32(0x3fc90000, 4 | (1 << 31)).unwrap();
    bus.write32(0x3fc90004, 0x3fc90100).unwrap();
    bus.write32(0x3fc90008, 0).unwrap();
    bus.write32(0x6003f048, 4).unwrap();
    bus.write32(0x6003f020, 0x90000 | (1 << 22)).unwrap();
    bus.tick(30000); // DMA alone does not consume the host queue.
    assert_eq!(bus.read32(0x3fc90100).unwrap(), 0);
    bus.write32(0x6002d020, 4).unwrap();
    bus.tick(30000);
    assert_eq!(bus.read32(0x3fc90100).unwrap(), 0xffa8004d);
}

#[test]
fn active_receiver_bounds_scheduler_sleep_with_and_without_pin_work() {
    let mut c3 = esp32c3::machine([0; 6], 4 << 20).bus;
    c3.write32(0x6002d020, 4).unwrap();
    assert_eq!(c3.next_deadline(), Some(256));
    // RMT independently requires a sooner deadline, retained while I2S is active.
    c3.write32(0x60016010, 1 | 1 << 8 | 1 << 16).unwrap();
    assert!(c3.next_deadline().is_some_and(|n| n <= 31));
    c3.tick(64);
    assert_eq!(c3.next_deadline(), Some(256));
    struct UartBoard;
    impl esp_soc::BoardModel for UartBoard {
        fn name(&self) -> &'static str { "uart-test" }
        fn uses_uart_pins(&self) -> bool { true }
    }
    c3.board = Box::new(UartBoard);
    c3.attach_board_devices();
    assert_eq!(c3.next_deadline(), Some(256));
    let mut c6 = esp32c6::machine([0; 6], 4 << 20).bus;
    c6.write32(0x6000c020, 4).unwrap();
    assert_eq!(c6.next_deadline(), Some(256));
}

#[test]
fn shared_receive_reports_interrupt_changes() {
    let mut ram = emu_core::FlatRam::new(0, 256);
    ram.write32(16, 4 | (1 << 31)).unwrap();
    ram.write32(20, 128).unwrap();
    let mut ch = esp_periph::GdmaInCh { desc: 16, running: true, ..Default::default() };
    let mut changed = false;
    assert!(ch.receive(&mut ram, &[1; 4], Some(4), false, |_| false, &mut changed));
    assert!(changed);
    assert_eq!(ch.int_raw, 3);
    changed = false;
    assert!(!ch.receive(&mut ram, &[1], Some(4), false, |_| false, &mut changed));
    assert!(changed);
    assert!(!ch.running);
    assert_eq!(ch.int_raw, 3 | 8 | 16);
    changed = false;
    ch.fail_receive(&mut changed);
    assert!(!changed);
}
