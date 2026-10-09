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
