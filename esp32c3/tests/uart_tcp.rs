//! Native UART/TCP integration: real C3 machine, shared UART model and localhost transport.
use esp_soc::UartTcp;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !ready() {
        assert!(Instant::now() < deadline, "UART TCP integration timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn machine_feeds_uart_without_overflow_and_routes_tx_to_tcp() {
    let uart_tcp = UartTcp::start("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(uart_tcp.local_addr()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    wait_for(|| uart_tcp.connected());

    let mut machine = esp32c3::machine([0x02, 0, 0, 0, 0, 1], 4 << 20);
    machine.console.capture = true;
    machine.uart_tcp = Some(uart_tcp.clone());

    let sent: Vec<u8> = (0..600).map(|i| (i * 37) as u8).collect();
    client.write_all(&sent).unwrap();
    wait_for(|| uart_tcp.pending_input() == sent.len());

    let mut received = Vec::new();
    while received.len() < sent.len() {
        let _ = machine.run(0);
        let uart = &mut machine.bus.periph.uart[0];
        while uart.rx_pending() > 0 {
            received.push(uart.read(0) as u8);
        }
    }
    assert_eq!(received, sent);
    assert_eq!(
        machine.bus.periph.uart[0].read(0x4) & (1 << 4),
        0,
        "RX overflow"
    );

    let reply = [0xc0, 0x00, 0xdb, 0xff, 0x42];
    machine.bus.periph.uart[0].tx_out.extend(reply);
    machine.drain_console();
    let mut from_machine = [0u8; 5];
    client.read_exact(&mut from_machine).unwrap();
    assert_eq!(from_machine, reply);
}
