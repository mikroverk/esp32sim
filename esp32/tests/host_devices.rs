use esp_soc::board::BoardEdge;

use esp32::bus::SocBus;
use esp_soc::{BoardModel, SocBus as _};
use xtensa_lx7::bus::Bus;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Probe { edges: Vec<BoardEdge>, writes: Vec<u64>, uart: Vec<(esp_soc::uart::UartRoute, u8)> }
struct Board(Arc<Mutex<Probe>>);
impl BoardModel for Board {
    fn name(&self) -> &'static str { "host-test" }
    fn gpio_output_at(&mut self, cycle: u64, _: &[(u8, bool)], _: u64, _: u64) {
        let mut p = self.0.lock().unwrap(); p.writes.push(cycle);
        p.edges.push(BoardEdge { cycle, pin: 5, level: true });
    }
    fn take_edges(&mut self) -> Vec<BoardEdge> { std::mem::take(&mut self.0.lock().unwrap().edges) }
    fn uart_tx(&mut self, _: u64, route: esp_soc::uart::UartRoute, byte: u8) { self.0.lock().unwrap().uart.push((route, byte)); }
    fn uart_rx(&mut self, _: u64) -> Vec<esp_soc::uart::UartInput> { vec![esp_soc::uart::UartInput::new(5, 115200, vec![42])] }
}
fn bus() -> SocBus { SocBus::new(4 << 20, [0; 6]) }
#[test]
fn uart_apb_ahb_and_pin_receive() {
    let mut b = bus(); let probe = Arc::new(Mutex::new(Probe::default())); b.board = Box::new(Board(probe.clone())); b.attach_board_devices();
    b.write32(0x3ff4_9048, (2 << 12) | (1 << 9)).unwrap();
    b.write32(0x3ff4_906c, (2 << 12) | (1 << 9)).unwrap();
    b.write32(0x3ff4_4540, 17).unwrap(); b.write32(0x3ff4_4024, 1 << 4).unwrap();
    b.write32(0x3ff4_4174, 0x80 | 5).unwrap(); // UART1 RX signal 17
    b.write32(0x3ff5_0014, 694).unwrap();
    b.write32(0x3ff5_0000, 65).unwrap(); b.write32(0x6001_0000, 66).unwrap();
    let p = probe.lock().unwrap(); assert_eq!(p.uart.len(), 2); assert!(p.uart[0].0.transmits_on(4)); assert_eq!(p.uart[1].1, 66); drop(p);
    b.tick(1); assert_eq!(b.read32(0x6001_0000), Ok(42));
    b.write32(0x3ff5_0014, 8333).unwrap();
    assert!(b.periph.uart_route(1).matches_baud(9600));
    b.write32(0x3ff5_0020, 0).unwrap();
    b.write32(0x3ff5_0014, 104).unwrap();
    assert!(b.periph.uart_route(1).matches_baud(9600));
}
#[test]
fn uart_matrix_receive_does_not_require_gpio_output_mux() {
    struct Gps;
    impl BoardModel for Gps {
        fn name(&self) -> &'static str { "gps" }
        fn uart_rx(&mut self, _: u64) -> Vec<esp_soc::uart::UartInput> {
            vec![esp_soc::uart::UartInput::new(4, 9600, b"$G".to_vec())]
        }
    }
    let mut b = bus();
    b.board = Box::new(Gps); b.attach_board_devices();
    // Arduino HardwareSerial(1), RX GPIO4: input enabled, output mux untouched.
    b.write32(0x3ff4_9048, 1 << 9).unwrap();
    b.write32(0x3ff4_4174, 0x80 | 4).unwrap();
    b.write32(0x3ff5_0014, 0x200068).unwrap();
    b.write32(0x3ff5_0020, 0x1c).unwrap();
    b.write32(0x3ff5_0024, 0xd0000001).unwrap();
    b.write32(0x3ff5_000c, 0x195).unwrap();
    b.write32(0x3ff0_0190, 6).unwrap();
    b.tick(1);
    assert_eq!(b.periph.uart_route(1).rx_pin, Some(4));
    assert_ne!(b.periph.cpu_lines(0) & (1 << 6), 0);
    assert_ne!(b.read32(0x3ff5_0008).unwrap() & (1 << 8), 0);
    assert_eq!((b.read32(0x3ff5_0060).unwrap() >> 13) & 0x7ff, 2);
    assert_eq!(b.read32(0x3ff5_0000), Ok(b'$' as u32));
    assert_eq!(b.read32(0x3ff5_0000), Ok(b'G' as u32));
    b.write32(0x3ff5_0010, 0x195).unwrap();
    assert_eq!(b.periph.cpu_lines(0) & (1 << 6), 0);
}

#[test]
fn uart1_uart2_native_pads_use_iomux_function_4() {
    let mut b = bus(); let probe = Arc::new(Mutex::new(Probe::default())); b.board = Box::new(Board(probe.clone())); b.attach_board_devices();
    // Reset pads are at function 0 (SD_DATA3 on GPIO10, GPIO17_0 on GPIO17): no UART1/UART2 route.
    b.write32(0x3ff4_9058, 1 << 9).unwrap(); // GPIO10 function 0, input enabled
    b.write32(0x3ff4_904c, 1 << 9).unwrap(); // GPIO16 function 0, input enabled
    for port in 1..3 {
        assert_eq!(b.periph.uart_route(port).tx_pins, 0, "UART{port}");
        assert_eq!(b.periph.uart_route(port).rx_pin, None, "UART{port}");
    }
    // Arduino Serial2.begin() default pins: uart_set_pin selects U2TXD/U2RXD (function 4).
    b.write32(0x3ff4_9050, 4 << 12).unwrap();
    b.write32(0x3ff4_904c, (4 << 12) | (1 << 9)).unwrap();
    // UART1 native pads: U1TXD on SD_DATA3/GPIO10, U1RXD on SD_DATA2/GPIO9 (function 4).
    b.write32(0x3ff4_9058, 4 << 12).unwrap();
    b.write32(0x3ff4_9054, (4 << 12) | (1 << 9)).unwrap();
    assert_eq!(b.periph.uart_route(2).tx_pins, 1 << 17);
    assert_eq!(b.periph.uart_route(2).rx_pin, Some(16));
    assert_eq!(b.periph.uart_route(1).tx_pins, 1 << 10);
    assert_eq!(b.periph.uart_route(1).rx_pin, Some(9));
    b.write32(0x3ff6_e000, 0x55).unwrap();
    let p = probe.lock().unwrap();
    let (route, byte) = p.uart.last().copied().unwrap();
    assert_eq!((route.port, route.tx_pins, byte), (2, 1 << 17, 0x55));
}

#[test]
fn physical_spi_excludes_released_and_high_selects() {
    struct SpiBoard(Arc<Mutex<Vec<esp_soc::board::SpiPins>>>);
    impl BoardModel for SpiBoard {
        fn name(&self) -> &'static str { "spi-pins" }
        fn uses_spi_pins(&self) -> bool { true }
        fn spi_transfer_pins(&mut self, _: u8, pins: esp_soc::board::SpiPins, _: &[u8], len: usize) -> Vec<u8> {
            self.0.lock().unwrap().push(pins); vec![0xa5; len]
        }
    }
    let mut b = bus(); let routes = Arc::new(Mutex::new(Vec::new())); b.board = Box::new(SpiBoard(routes.clone())); b.attach_board_devices();
    // SPI2 native clock/MOSI GPIO14/13, software CS GPIO4.
    for addr in [0x3ff4_9030, 0x3ff4_9038] { b.write32(addr, (1 << 12) | (1 << 9)).unwrap(); }
    b.write32(0x3ff4_9048, (2 << 12) | (1 << 9)).unwrap();
    b.write32(0x3ff4_4024, 1 << 4).unwrap();
    for high in [false, true] {
        b.write32(if high { 0x3ff4_4008 } else { 0x3ff4_400c }, 1 << 4).unwrap();
        b.write32(0x3ff6_401c, (1 << 27) | (1 << 28) | 1).unwrap();
        b.write32(0x3ff6_4028, 7).unwrap(); b.write32(0x3ff6_402c, 7).unwrap(); b.write32(0x3ff6_4000, 1 << 18).unwrap();
        let p = *routes.lock().unwrap().last().unwrap();
        assert_ne!(p.sclk & (1 << 14), 0); assert_ne!(p.mosi & (1 << 13), 0);
        assert_eq!(p.cs & (1 << 4) != 0, !high);
    }
}

#[test]
fn timestamped_feedback_and_released_pull_survive_reboot() {
    let mut b = bus(); let probe = Arc::new(Mutex::new(Probe::default())); b.board = Box::new(Board(probe.clone())); b.attach_board_devices();
    b.write32(0x3ff4_9048, (2 << 12) | (1 << 9)).unwrap();
    b.write32(0x3ff4_906c, (2 << 12) | (1 << 9) | (1 << 8)).unwrap();
    b.cycles = 107;
    b.write32(0x3ff4_4024, 1 << 4).unwrap(); b.write32(0x3ff4_4008, 1 << 4).unwrap();
    assert_eq!(probe.lock().unwrap().writes.last(), Some(&107));
    assert_ne!(b.read32(0x3ff4_403c).unwrap() & (1 << 5), 0);
    b.gpio_set_input(5, false); b.reboot([0; 6]); assert_eq!(b.gpio_input() & (1 << 5), 0);
    b.write32(0x3ff4_906c, (2 << 12) | (1 << 9) | (1 << 8)).unwrap();
    b.gpio_release_input(5); assert_ne!(b.gpio_input() & (1 << 5), 0); assert!(b.gpio_state(5).unwrap().pull_up);
}

#[test]
fn inactive_board_skips_tick_and_mmio_callbacks() {
    struct Inactive;
    impl BoardModel for Inactive {
        fn name(&self) -> &'static str { "inactive" }
        fn uses_gpio_edges(&self) -> bool { false }
        fn advance_to(&mut self, _: u64) { panic!("inactive board advanced") }
        fn uart_rx(&mut self, _: u64) -> Vec<esp_soc::uart::UartInput> { panic!("inactive UART polled") }
        fn uart_tx(&mut self, _: u64, _: esp_soc::uart::UartRoute, _: u8) { panic!("inactive UART notified") }
        fn gpio_output_at(&mut self, _: u64, _: &[(u8, bool)], _: u64, _: u64) { panic!("inactive GPIO notified") }
        fn next_deadline(&self) -> Option<u64> { panic!("inactive deadline polled") }
    }
    let mut b = bus();
    b.board = Box::new(Inactive);
    b.attach_board_devices();
    b.tick(10);
    b.write32(0x3ff4_0000, 42).unwrap();
    b.write32(0x6000_0000, 43).unwrap();
    b.write32(0x3ff4_4024, 1 << 4).unwrap();
    b.read32(0x3ff4_0008).unwrap();
    assert_eq!(b.next_deadline(), None);
}

// Advance 240 MHz bus-reference ticks even when reset selects 40 MHz XTAL.
// The core clock conversion is part of #206; PCM/ADC source rates use bus time.
fn tick_reference(b: &mut SocBus, ticks: u32) {
    let cpu = b.cpu_cycles_for_bus(u64::from(ticks));
    b.tick(u32::try_from(cpu).unwrap());
}

#[test]
fn i2s_native_dma_packs_pcm_and_raises_eof() {
    let mut b = bus(); let base = 0x3ff4_f000;
    b.write32(0x3ff0_00c0, 1 << 4).unwrap();
    for (address, value) in [(0x3ffb_0100, 0x8000_0008), (0x3ffb_0104, 0x3ffb_0200), (0x3ffb_0108, 0)] { b.write32(address, value).unwrap(); }
    b.i2s_input(0).unwrap().push(&[[123, -456], [789, -123]]);
    b.write32(base + 0xac, 25 | (1 << 20)).unwrap();
    b.write32(base + 0xb0, (25 << 6) | (16 << 18)).unwrap();
    b.write32(base + 0x14, 1 << 9).unwrap();
    b.write32(base + 0x24, 2).unwrap();
    b.write32(base + 0x34, (1 << 29) | 0xb0100).unwrap(); b.write32(base + 8, 1 << 5).unwrap();
    tick_reference(&mut b, 60_000);
    assert_eq!(b.read32(0x3ffb_0200), Ok(123 | ((-456i16 as u16 as u32) << 16)));
    assert_eq!(b.read32(base + 0x3c), Ok(0x3ffb_0100));
    assert_ne!(b.read32(base + 0x10).unwrap() & (1 << 9), 0);
    assert_eq!(b.read32(0x3ffb_0100).unwrap() >> 31, 0);
    // IN_DONE (bit 8) and IN_SUC_EOF (bit 9) raw; no IN_DSCR_ERR or IN_DSCR_EMPTY.
    assert_eq!(b.read32(base + 0xc).unwrap() & ((1 << 8) | (1 << 9) | (1 << 13) | (1 << 15)), (1 << 8) | (1 << 9));
}
/// 16-bit stereo at 8 kHz: one 4-byte frame per 30,000 bus-reference ticks.
fn receiver(b: &mut SocBus, port: usize, size: u32, buffer: u32) -> (u32, u32) {
    let base = [0x3ff4_f000, 0x3ff6_d000][port];
    let desc = 0x3ffb_0100 + port as u32 * 12;
    for (off, value) in [(0, (1 << 31) | size), (4, buffer), (8, 0)] { b.write32(desc + off, value).unwrap(); }
    b.write32(base + 0xac, 25 | (1 << 20)).unwrap();
    b.write32(base + 0xb0, (25 << 6) | (16 << 18)).unwrap();
    b.write32(base + 0x14, (1 << 9) | (1 << 13)).unwrap();
    b.write32(base + 0x24, size / 4).unwrap();
    b.write32(base + 0x34, (1 << 29) | (desc & 0xfffff)).unwrap();
    b.write32(base + 8, 1 << 5).unwrap();
    (base, desc)
}

#[test]
fn i2s_clock_gate_partial_dma_faults_and_interrupts() {
    for port in 0..2 {
        let mut b = bus();
        let (base, desc) = receiver(&mut b, port, 8, 0x3ffb_0300);
        b.i2s_input(port).unwrap().push(&[[1, 2], [3, 4]]);
        tick_reference(&mut b, 30_000);
        assert_eq!(b.read32(0x3ffb_0300), Ok(0));
        b.write32(0x3ff0_00c0, 1 << [4, 21][port]).unwrap();
        b.write32(0x3ff0_0104 + 4 * (32 + port as u32), 7).unwrap();
        tick_reference(&mut b, 30_000);
        assert_eq!(b.read32(0x3ffb_0300), Ok(1 | 2 << 16));
        assert_ne!(b.read32(desc).unwrap() & (1 << 31), 0);
        assert_eq!(b.periph.cpu_lines(0) & (1 << 7), 0);
        b.irq_dirty = false;
        tick_reference(&mut b, 30_000);
        assert!(b.irq_dirty);
        assert_eq!(b.read32(0x3ffb_0304), Ok(3 | 4 << 16));
        assert_ne!(b.periph.cpu_lines(0) & (1 << 7), 0);
        b.write32(base + 0x18, 1 << 9).unwrap();
        assert_eq!(b.periph.cpu_lines(0) & (1 << 7), 0);
        // DMA must reject MMIO rather than writing a UART FIFO.
        receiver(&mut b, port, 4, 0x3ff4_0000);
        tick_reference(&mut b, 30_000);
        assert_ne!(b.read32(base + 0x10).unwrap() & (1 << 13), 0);
        assert_eq!(b.read32(base + 0x10).unwrap() & (1 << 9), 0);
        assert!(b.periph.uart[0].tx_out.is_empty());
        assert_ne!(b.read32(desc).unwrap() & (1 << 31), 0);
    }
}

#[test]
fn i2s_pin_source_checks_all_wires_and_survives_reboot() {
    use esp_periph::i2s::{PcmPins, PcmSource};
    for (port, data_signal, clock_signal, ws_signal) in [(0, 155, 27, 28), (1, 181, 164, 165)] {
        let mut b = bus();
        assert!(b.pcm_sources().is_none());
        let mut source = PcmSource::new(8000, PcmPins::I2s { bclk: 4, ws: 5, data: 18 }).unwrap();
        source.push(&[[11, 22]; 16]);
        b.attach_pcm_sources().unwrap().inputs.push(Some(source));
        let mut second = PcmSource::new(8000, PcmPins::I2s { bclk: 4, ws: 5, data: 18 }).unwrap();
        second.push(&[[55, 66]; 16]);
        b.attach_pcm_sources().unwrap().inputs.push(Some(second));
        b.i2s_input(port).unwrap().push(&[[33, 44]]);
        b.reboot([0; 6]);
        assert_eq!(b.periph.i2s[port].inner.rx_source(), None);
        assert_eq!(b.pcm_sources().unwrap().inputs[0].as_ref().unwrap().queued_frames(), 16);
        tick_reference(&mut b, 30_000);
        // The read-only view does not advance source time; attaching synchronizes it.
        assert_eq!(b.pcm_sources().unwrap().inputs[0].as_ref().unwrap().queued_frames(), 16);
        assert_eq!(b.attach_pcm_sources().unwrap().inputs[0].as_ref().unwrap().queued_frames(), 15);
        b.write32(0x3ff0_00c0, 1 << [4, 21][port]).unwrap();
        for mux in [0x48, 0x6c, 0x70] { b.write32(0x3ff4_9000 + mux, (2 << 12) | (1 << 9)).unwrap(); }
        b.write32(0x3ff4_4130 + data_signal * 4, (1 << 7) | 18).unwrap();
        b.write32(0x3ff4_4530 + 4 * 4, clock_signal).unwrap();
        b.write32(0x3ff4_4530 + 5 * 4, ws_signal).unwrap();
        receiver(&mut b, port, 4, 0x3ffb_0200);
        tick_reference(&mut b, 30_000);
        assert_eq!(b.read32(0x3ffb_0200), Ok(11 | 22 << 16));
        assert_eq!(b.periph.i2s[port].inner.rx_source(), Some(0));
        // A route that matches no slot reads the per-controller queue (#202's fallback): its
        // one queued frame, then silence once drained.
        for (address, value, expected) in [
            (0x3ff4_4130 + data_signal * 4, (1 << 7) | 19, 33 | 44 << 16),
            (0x3ff4_4530 + 4 * 4, clock_signal ^ 1, 0),
            (0x3ff4_4530 + 5 * 4, ws_signal ^ 1, 0),
        ] {
            let old = b.read32(address).unwrap();
            b.write32(address, value).unwrap();
            receiver(&mut b, port, 4, 0x3ffb_0200);
            tick_reference(&mut b, 30_000);
            assert_eq!(b.read32(0x3ffb_0200), Ok(expected), "signal register {address:#x}");
            assert_eq!(b.periph.i2s[port].inner.rx_source(), None);
            b.write32(address, old).unwrap();
        }
        // Matched wiring again: the second slot is selected once the first is emptied.
        b.attach_pcm_sources().unwrap().inputs[0] = None;
        receiver(&mut b, port, 4, 0x3ffb_0200);
        tick_reference(&mut b, 30_000);
        assert_eq!(b.read32(0x3ffb_0200), Ok(55 | 66 << 16));
        assert_eq!(b.periph.i2s[port].inner.rx_source(), Some(1));
        b.reboot([0; 6]);
        assert_eq!(b.periph.i2s[port].inner.rx_source(), None);
    }
}

/// IDF links its RX descriptors into a circle and sets owner = 1 once, at allocation
/// (esp_driver_i2s/i2s_common.c:534,551); it never enables CHECK_OWNER. Two descriptors of
/// one frame each must keep receiving pass after pass while their owner bits read 0.
#[test]
fn i2s_two_descriptor_ring_keeps_running_after_owner_writeback() {
    for port in 0..2 {
        let mut b = bus();
        let (base, first) = receiver(&mut b, port, 4, 0x3ffb_0200);
        let second = first + 0x40;
        let eof = 1 << 30;
        for (desc, buf, next) in [(first, 0x3ffb_0200, second), (second, 0x3ffb_0204, first)] {
            for (off, v) in [(0, (1 << 31) | eof | (4 << 12) | 4), (4, buf), (8, next)] { b.write32(desc + off, v).unwrap(); }
        }
        b.write32(0x3ff0_00c0, 1 << [4, 21][port]).unwrap();
        b.i2s_input(port).unwrap().push(&[[1, 2], [3, 4], [5, 6], [7, 8], [9, 10], [11, 12]]);
        for pass in 0..6u32 {
            tick_reference(&mut b, 30_000);
            let (desc, buf) = if pass % 2 == 0 { (first, 0x3ffb_0200) } else { (second, 0x3ffb_0204) };
            let n = 2 * pass + 1;
            assert_eq!(b.read32(buf), Ok(n | (n + 1) << 16), "pass {pass}");
            assert_eq!(b.read32(desc), Ok(eof | (4 << 12) | 4), "pass {pass}: owner cleared, length 4");
            assert_eq!(b.read32(base + 0x3c), Ok(desc));
            assert_eq!(b.read32(base + 0xc).unwrap() & ((1 << 9) | (1 << 13) | (1 << 15)), 1 << 9, "pass {pass}");
            b.write32(base + 0x18, u32::MAX).unwrap();
        }
        // With LC_CONF CHECK_OWNER (bit 12) set, the CPU-owned descriptor stops the ring.
        b.write32(base + 0x60, 1 << 12).unwrap();
        b.i2s_input(port).unwrap().push(&[[13, 14]]);
        tick_reference(&mut b, 30_000);
        assert_eq!(b.read32(base + 0xc).unwrap() & ((1 << 9) | (1 << 13)), 1 << 13);
        assert_eq!(b.read32(0x3ffb_0200), Ok(9 | 10 << 16));
    }
}

#[test]
fn i2s_rejects_empty_unowned_with_check_owner_and_non_dram_descriptors() {
    const CHECK: u32 = 1 << 12;
    // (descriptor, control, LC_CONF, buffer): owner checked only with CHECK_OWNER; size 0;
    // DROM, IRAM and RTC descriptors; IRAM, DROM and RTC buffers; DRAM's last word straddling.
    for (address, control, lc_conf, buffer) in [
        (0x3ffb_0100, 4u32, CHECK, 0x3ffb_0200),
        (0x3ffb_0100, 1 << 31, 0, 0x3ffb_0200u32),
        (0x3ff9_1000, (1 << 31) | 4, 0, 0x3ffb_0200),
        (0x4008_0100, (1 << 31) | 4, 0, 0x3ffb_0200),
        (0x3ff8_0100, (1 << 31) | 4, 0, 0x3ffb_0200),
        (0x3ffb_0100, (1 << 31) | 4, 0, 0x4008_0200),
        (0x3ffb_0100, (1 << 31) | 4, 0, 0x3ff9_1000),
        (0x3ffb_0100, (1 << 31) | 4, 0, 0x3ff8_0200),
        (0x3ffb_0100, (1 << 31) | 4, 0, 0x3fff_fffe),
    ] {
        let mut b = bus();
        let (base, _) = receiver(&mut b, 0, 4, 0x3ffb_0200);
        b.write32(0x3ff0_00c0, 1 << 4).unwrap();
        b.write32(base + 0x60, (1 << 8) | lc_conf).unwrap();
        b.load_bytes(address, &control.to_le_bytes()).unwrap();
        b.load_bytes(address + 4, &buffer.to_le_bytes()).unwrap();
        b.load_bytes(address + 8, &0u32.to_le_bytes()).unwrap();
        b.load_bytes(buffer, &[0xa5; 2]).unwrap();
        b.write32(base + 0x34, (1 << 29) | (address & 0xfffff)).unwrap();
        b.i2s_input(0).unwrap().push(&[[7, 8]]);
        tick_reference(&mut b, 30_000);
        assert_eq!(b.read32(base + 0x10).unwrap(), 1 << 13, "{address:#x} {buffer:#x}");
        assert_eq!(b.read32(address), Ok(control), "{address:#x} {buffer:#x}");
        assert_eq!((b.read8(buffer), b.read8(buffer + 1)), (Ok(0xa5), Ok(0xa5)), "{address:#x} {buffer:#x}");
    }
    // Owned bit clear and CHECK_OWNER at its reset value 0: IDF's ring descriptors are accepted.
    let mut b = bus();
    let (base, desc) = receiver(&mut b, 0, 4, 0x3ffb_0200);
    b.write32(desc, 4).unwrap();
    b.write32(0x3ff0_00c0, 1 << 4).unwrap();
    assert_eq!(b.read32(base + 0x60), Ok(1 << 8));
    b.i2s_input(0).unwrap().push(&[[7, 8]]);
    tick_reference(&mut b, 30_000);
    assert_eq!(b.read32(0x3ffb_0200), Ok(7 | 8 << 16));
    assert_eq!(b.read32(base + 0x10).unwrap(), 1 << 9);
}

/// I2S_INLINK_ADDR holds 20 bits under 0x3FF0_0000, so offset 0x40000 names UART0's FIFO.
/// The descriptor read must not pop UART input and the writeback must not transmit.
#[test]
fn i2s_inlink_offset_into_registers_does_not_touch_uart_fifo() {
    let mut b = bus();
    let (base, _) = receiver(&mut b, 0, 4, 0x3ffb_0200);
    b.write32(0x3ff0_00c0, 1 << 4).unwrap();
    b.periph.uart[0].0.host_input(b"abcd");
    b.write32(base + 0x34, (1 << 29) | 0x40000).unwrap();
    b.i2s_input(0).unwrap().push(&[[7, 8]]);
    tick_reference(&mut b, 30_000);
    assert_eq!(b.read32(base + 0x10).unwrap(), 1 << 13);
    assert_eq!(b.periph.uart[0].0.rx_pending(), 4);
    assert!(b.periph.uart[0].0.tx_out.is_empty());
}
#[test]
fn controller_pcm_queue_survives_classic_reset_without_a_source_bank() {
    let mut b = bus();
    b.i2s_input(0).unwrap().push(&[[33, 44]]);
    b.reboot([0; 6]);
    b.write32(0x3ff0_00c0, 1 << 4).unwrap();
    receiver(&mut b, 0, 4, 0x3ffb_0200);
    tick_reference(&mut b, 30_000);
    assert_eq!(b.read32(0x3ffb_0200), Ok(33 | 44 << 16));
}

#[test]
fn classic_raw_stream_uses_shared_clock_and_survives_reset() {
    use esp_periph::analog::{AnalogSource, AnalogStream};
    let mut b = bus();
    let stream = AnalogStream::new_raw(8000, 17, 0).unwrap();
    stream.push_raw(&[1234, 2345], 0, 240_000_000).unwrap();
    b.analog_set(34, AnalogSource::RawStream(stream));
    for (time, raw) in [(0, 17), (30_000, 1234), (60_000, 2345), (90_000, 2345)] {
        let elapsed = (time - b.cycles) as u32;
        tick_reference(&mut b, elapsed);
        b.reboot([0; 6]);
        b.write32(0x3ff4_8800, 1 << 28).unwrap();
        for attenuation in 0..4 {
            b.write32(0x3ff4_8834, attenuation << 12).unwrap();
            let start = (1 << 31) | (1 << 25) | (1 << 18);
            b.write32(0x3ff4_8854, start).unwrap();
            b.write32(0x3ff4_8854, start | (1 << 17)).unwrap();
            assert_eq!(b.read32(0x3ff4_8854).unwrap() & 0xffff, raw);
        }
    }
    assert_eq!(b.adc_observation(34).unwrap().generation, 16);
}

#[test]
fn i2s_eof_words_span_descriptors_and_reject_unrepresentable_counts() {
    for port in 0..2 {
        let mut b = bus();
        let base = [0x3ff4_f000, 0x3ff6_d000][port];
        assert_eq!(b.read32(base + 0x24), Ok(64));
        let (_, desc) = receiver(&mut b, port, 4, 0x3ffb_0200);
        let next = desc + 0x40;
        b.write32(desc + 8, next).unwrap();
        for (off, v) in [(0, (1 << 31) | 4), (4, 0x3ffb_0204), (8, 0)] { b.write32(next + off, v).unwrap(); }
        b.write32(base + 0x24, 2).unwrap();
        b.write32(0x3ff0_00c0, 1 << [4, 21][port]).unwrap();
        b.i2s_input(port).unwrap().push(&[[1, 2], [3, 4]]);
        tick_reference(&mut b, 30_000);
        assert_eq!(b.read32(base + 0x10), Ok(0));
        assert_eq!(b.read32(desc).unwrap() & (3 << 30), 0);
        tick_reference(&mut b, 30_000);
        assert_eq!(b.read32(base + 0x3c), Ok(next));
        assert_eq!(b.read32(base + 0x10), Ok(1 << 9));
        assert_eq!(b.read32(0x3ffb_0204), Ok(3 | 4 << 16));
        b.write32(base + 0x18, u32::MAX).unwrap();
        receiver(&mut b, port, 4, 0x3ffb_0200);
        tick_reference(&mut b, 60_000);
        assert_eq!(b.read32(base + 0x10), Ok((1 << 9) | (1 << 13)));
        // The second frame finds no descriptor left: IN_DSCR_EMPTY (bit 15) is raw-only here.
        assert_ne!(b.read32(base + 0xc).unwrap() & (1 << 15), 0);
        for words in [0, u32::MAX] {
            b.write32(base + 0x18, u32::MAX).unwrap();
            receiver(&mut b, port, 4, 0x3ffb_0200);
            b.write32(base + 0x24, words).unwrap();
            tick_reference(&mut b, 30_000);
            assert_eq!(b.read32(base + 0x10), Ok(1 << 13));
            assert_ne!(b.read32(desc).unwrap() & (1 << 31), 0);
        }
    }
}
