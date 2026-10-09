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
    b.tick(60_000);
    assert_eq!(b.read32(0x3ffb_0200), Ok(123 | ((-456i16 as u16 as u32) << 16)));
    assert_eq!(b.read32(base + 0x3c), Ok(0x3ffb_0100));
    assert_ne!(b.read32(base + 0x10).unwrap() & (1 << 9), 0);
    assert_eq!(b.read32(0x3ffb_0100).unwrap() >> 31, 0);
    assert_eq!(b.read32(base + 0xc).unwrap() & (1 << 13), 0);
}
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
fn i2s_clock_gate_partial_dma_faults_interrupts_and_code_invalidation() {
    for port in 0..2 {
        let mut b = bus();
        let (base, desc) = receiver(&mut b, port, 8, 0x4008_0100);
        b.i2s_input(port).unwrap().push(&[[1, 2], [3, 4]]);
        b.tick(30_000);
        assert_eq!(b.read32(0x4008_0100), Ok(0));
        b.write32(0x3ff0_00c0, 1 << [4, 21][port]).unwrap();
        b.write32(0x3ff0_0104 + 4 * (32 + port as u32), 7).unwrap();
        let page = b.code_page(0x4008_0100) as usize;
        b.tick(30_000);
        assert_eq!(b.read32(0x4008_0100), Ok(1 | 2 << 16));
        assert!(b.page_versions()[page] > 0);
        assert_ne!(b.read32(desc).unwrap() & (1 << 31), 0);
        assert_eq!(b.periph.cpu_lines(0) & (1 << 7), 0);
        b.irq_dirty = false;
        b.tick(30_000);
        assert!(b.irq_dirty);
        assert_eq!(b.read32(0x4008_0104), Ok(3 | 4 << 16));
        assert_ne!(b.periph.cpu_lines(0) & (1 << 7), 0);
        b.write32(base + 0x18, 1 << 9).unwrap();
        assert_eq!(b.periph.cpu_lines(0) & (1 << 7), 0);
        // DMA must reject MMIO rather than writing a UART FIFO.
        receiver(&mut b, port, 4, 0x3ff4_0000);
        b.tick(30_000);
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
        let mut source = PcmSource::new(8000, PcmPins::I2s { bclk: 4, ws: 5, data: 18 }).unwrap();
        source.push(&[[11, 22]; 16]);
        b.pcm_sources().unwrap().inputs.push(Some(source));
        let mut second = PcmSource::new(8000, PcmPins::I2s { bclk: 4, ws: 5, data: 18 }).unwrap();
        second.push(&[[55, 66]; 16]);
        b.pcm_sources().unwrap().inputs.push(Some(second));
        b.i2s_input(port).unwrap().push(&[[33, 44]]);
        b.reboot([0; 6]);
        assert_eq!(b.pcm_sources().unwrap().inputs[0].as_ref().unwrap().queued_frames(), 16);
        b.tick(30_000);
        assert_eq!(b.pcm_sources().unwrap().inputs[0].as_ref().unwrap().queued_frames(), 15);
        b.write32(0x3ff0_00c0, 1 << [4, 21][port]).unwrap();
        for mux in [0x48, 0x6c, 0x70] { b.write32(0x3ff4_9000 + mux, (2 << 12) | (1 << 9)).unwrap(); }
        b.write32(0x3ff4_4130 + data_signal * 4, (1 << 7) | 18).unwrap();
        b.write32(0x3ff4_4530 + 4 * 4, clock_signal).unwrap();
        b.write32(0x3ff4_4530 + 5 * 4, ws_signal).unwrap();
        receiver(&mut b, port, 4, 0x3ffb_0200);
        b.tick(30_000);
        assert_eq!(b.read32(0x3ffb_0200), Ok(11 | 22 << 16));
        for (address, value) in [
            (0x3ff4_4130 + data_signal * 4, (1 << 7) | 19),
            (0x3ff4_4530 + 4 * 4, clock_signal ^ 1),
            (0x3ff4_4530 + 5 * 4, ws_signal ^ 1),
        ] {
            let old = b.read32(address).unwrap();
            b.write32(address, value).unwrap();
            receiver(&mut b, port, 4, 0x3ffb_0200);
            b.tick(30_000);
                assert_eq!(b.read32(0x3ffb_0200), Ok(0));
            b.write32(address, old).unwrap();
        }
        b.pcm_sources().unwrap().inputs[0] = None;
        b.pcm_sources().unwrap().inputs[1] = None;
        receiver(&mut b, port, 4, 0x3ffb_0200);
        b.tick(30_000);
        assert_eq!(b.read32(0x3ffb_0200), Ok(0)); // An attached empty bank remains silent.
    }
}


#[test]
fn i2s_rejects_unowned_empty_and_unwritable_descriptors() {
    for (address, control) in [(0x3ffb_0100, 4u32), (0x3ffb_0100, 1 << 31), (0x3ff9_1000, (1 << 31) | 4)] {
        let mut b = bus();
        let (base, _) = receiver(&mut b, 0, 4, 0x3ffb_0200);
        b.write32(0x3ff0_00c0, 1 << 4).unwrap();
        b.load_bytes(address, &control.to_le_bytes()).unwrap();
        b.load_bytes(address + 4, &0x3ffb_0200u32.to_le_bytes()).unwrap();
        b.load_bytes(address + 8, &0u32.to_le_bytes()).unwrap();
        b.write32(base + 0x34, (1 << 29) | (address & 0xfffff)).unwrap();
        b.i2s_input(0).unwrap().push(&[[7, 8]]);
        b.tick(30_000);
        assert_eq!(b.read32(base + 0x10).unwrap(), 1 << 13);
        assert_eq!(b.read32(address), Ok(control));
    }
}

#[test]
fn controller_pcm_queue_survives_classic_reset_without_a_source_bank() {
    let mut b = bus();
    b.i2s_input(0).unwrap().push(&[[33, 44]]);
    b.reboot([0; 6]);
    b.write32(0x3ff0_00c0, 1 << 4).unwrap();
    receiver(&mut b, 0, 4, 0x3ffb_0200);
    b.tick(30_000);
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
        b.tick((time - b.cycles) as u32);
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
        b.tick(30_000);
        assert_eq!(b.read32(base + 0x10), Ok(0));
        assert_eq!(b.read32(desc).unwrap() & (3 << 30), 0);
        b.tick(30_000);
        assert_eq!(b.read32(base + 0x3c), Ok(next));
        assert_eq!(b.read32(base + 0x10), Ok(1 << 9));
        assert_eq!(b.read32(0x3ffb_0204), Ok(3 | 4 << 16));
        b.write32(base + 0x18, u32::MAX).unwrap();
        receiver(&mut b, port, 4, 0x3ffb_0200);
        b.tick(60_000);
        assert_eq!(b.read32(base + 0x10), Ok((1 << 9) | (1 << 13)));
        for words in [0, u32::MAX] {
            b.write32(base + 0x18, u32::MAX).unwrap();
            receiver(&mut b, port, 4, 0x3ffb_0200);
            b.write32(base + 0x24, words).unwrap();
            b.tick(30_000);
            assert_eq!(b.read32(base + 0x10), Ok(1 << 13));
            assert_ne!(b.read32(desc).unwrap() & (1 << 31), 0);
        }
    }
}
