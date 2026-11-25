//! `--board socket:` against the in-repo AT peer (`support/at_peer.rs`), on a classic ESP32 guest
//! small enough to reason about: `MODEM` routes UART1 TX to GPIO27 and RX from GPIO26 through the
//! GPIO matrix at 115200 baud, sends `AT\r`, then echoes every byte it receives back out of UART1.
//! The peer answers `OK` after its latency, sends a line of its own at 5 ms and a 1500-byte burst at
//! 10 ms, so the run checks the exchange times, the receive pacing (the burst would overflow the
//! 128-byte RX FIFO if it arrived at once), that a sync is not needed per byte, guest resets, and
//! that two runs give the same session byte for byte. No ROM, no firmware, no toolchain: the
//! program's bytes are checked in.
#[path = "support/at_peer.rs"]
mod at_peer;
use at_peer::{Script, Transcript};
use esp32sim::socket_board::{Endpoint, SocketBoard};
use esp_soc::{Soc, Stop};
use std::io::BufReader;

/// `modem.S`, assembled with `xtensa-esp32-elf-as --text-section-literals` (binutils 2.43.1,
/// esp-14.2.0_20260121) and linked at 0x40080000; nine literals, then `_start` at 0x40080024:
/// ```text
/// _start: l32r a2, 0x3ff49028      # IO_MUX GPIO26
///         l32r a3, 0x2200; s32i a3, a2, 0   # GPIO26: FUN_IE, MCU_SEL 2 (GPIO)
///         l32r a3, 0x2000; s32i a3, a2, 4   # GPIO27: MCU_SEL 2
///         l32r a2, 0x3ff4459c; movi a3, 17; s32i a3, a2, 0      # FUNC27_OUT_SEL = U1TXD (17)
///         l32r a2, 0x3ff44024; l32r a3, 1 << 27; s32i a3, a2, 0 # ENABLE_W1TS GPIO27
///         l32r a2, 0x3ff44174; movi a3, 0x9a; s32i a3, a2, 0    # FUNC17_IN_SEL (U1RXD) = SIG_IN_SEL | 26
///         l32r a2, 0x3ff50000      # UART1
///         movi a3, 694; s32i a3, a2, 0x14                       # CLKDIV: 80 MHz / 694 = 115273 baud
///         l32r a3, 0x0800001c; s32i a3, a2, 0x20                # CONF0: TICK_REF_ALWAYS_ON, 8N1
///         movi a3, 'A'; s32i a3, a2, 0; movi a3, 'T'; s32i a3, a2, 0; movi a3, 13; s32i a3, a2, 0
/// loop:   l32i a4, a2, 0x1c; extui a4, a4, 0, 8; beqz a4, loop  # STATUS.RXFIFO_CNT
///         l32i a5, a2, 0; s32i a5, a2, 0; j loop                # echo one byte
/// ```
/// Register offsets: IDF v5.5.4 (as v4.4) components/soc/esp32/register/soc/io_mux_reg.h
/// (PERIPHS_IO_MUX_GPIO26_U 0x28, GPIO27_U 0x2c), gpio_reg.h (GPIO_ENABLE_W1TS_REG 0x24,
/// GPIO_FUNC0_IN_SEL_CFG_REG 0x130, GPIO_FUNC0_OUT_SEL_CFG_REG 0x530), gpio_sig_map.h
/// (U1RXD_IN_IDX / U1TXD_OUT_IDX 17) and uart_reg.h (UART_CLKDIV_REG 0x14, UART_CONF0_REG 0x20,
/// UART_STATUS_REG 0x1c).
const MODEM: [u8; 112] = [
    0x28, 0x90, 0xf4, 0x3f, 0x00, 0x22, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x9c, 0x45, 0xf4, 0x3f, 0x24, 0x40, 0xf4, 0x3f,
    0x00, 0x00, 0x00, 0x08, 0x74, 0x41, 0xf4, 0x3f, 0x00, 0x00, 0xf5, 0x3f, 0x1c, 0x00, 0x00, 0x08, 0x21, 0xf7, 0xff, 0x31,
    0xf7, 0xff, 0x39, 0x02, 0x31, 0xf7, 0xff, 0x39, 0x12, 0x21, 0xf6, 0xff, 0x1c, 0x13, 0x39, 0x02, 0x21, 0xf6, 0xff, 0x31,
    0xf6, 0xff, 0x39, 0x02, 0x21, 0xf6, 0xff, 0x32, 0xa0, 0x9a, 0x39, 0x02, 0x21, 0xf5, 0xff, 0x32, 0xa2, 0xb6, 0x39, 0x52,
    0x31, 0xf4, 0xff, 0x39, 0x82, 0x4c, 0x13, 0x39, 0x02, 0x5c, 0x43, 0x39, 0x02, 0x0c, 0xd3, 0x39, 0x02, 0x48, 0x72, 0x40,
    0x40, 0x74, 0x16, 0x74, 0xff, 0x58, 0x02, 0x59, 0x02, 0x06, 0xfc, 0xff,
];
const BASE: u32 = 0x4008_0000;
const ENTRY: u32 = 0x4008_0024;
const HZ: u64 = <esp32::Esp32 as Soc>::CPU_HZ;
const MS: u64 = 1_000_000;
/// One 8N1 character at the guest's 115273 baud, as the board paces it at the peer's 115200.
const CHAR_NS: u64 = 10 * 1_000_000_000 / 115_200;
/// The board sees time at device ticks (one 64-instruction quantum, 1.6 µs at the 40 MHz reset
/// clock) and the guest needs a few instructions to echo a byte.
const SLACK: u64 = 5_000;

fn burst() -> Vec<u8> { (0..1500u32).map(|i| (i * 7 + 1) as u8).collect() }

fn script() -> Script { Script { unprompted: vec![(5 * MS, b"$TICK\r\n".to_vec()), (10 * MS, burst())], ..Script::default() } }

/// A peer on a fresh TCP port, served in a thread; the board connects to it as `--board tcp:` does.
fn peer(s: Script) -> (Endpoint, std::thread::JoinHandle<Transcript>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = Endpoint::Tcp(listener.local_addr().unwrap().to_string());
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut input = BufReader::new(stream.try_clone().unwrap());
        let mut output = stream;
        at_peer::serve(&mut input, &mut output, &s)
    });
    (endpoint, handle)
}

fn machine(board: SocketBoard) -> esp32::Machine {
    let mut m = esp32::machine([0x24, 0x6f, 0x28, 0, 0x11, 0x22], 4 << 20);
    m.bus.board = Box::new(board);
    m.bus.attach_board_devices();
    m.bus.load_bytes(BASE, &MODEM).unwrap();
    m.cores[0].pc = ENTRY;
    m
}

/// The time of the `index`th transmitted byte, if it starts or ends a `uart_tx` message.
fn byte_time(tr: &Transcript, mut index: usize) -> Option<u64> {
    for (t, t_end, _, d) in &tr.tx {
        if index < d.len() { return if index == 0 { Some(*t) } else if index + 1 == d.len() { Some(*t_end) } else { None }; }
        index -= d.len();
    }
    None
}

/// 150 ms of the guest, a chip reset, the end of the run: the peer's view and the board's report.
fn session() -> (Transcript, String) {
    let (endpoint, handle) = peer(script());
    let board = SocketBoard::connect(&endpoint, "esp32", HZ).unwrap();
    let mut m = machine(board);
    m.max_cycles = HZ * 150 / 1000;
    assert!(matches!(m.run(u64::MAX), Stop::Halted));
    m.reboot();
    let report = m.bus.board.report();
    drop(m);
    (handle.join().unwrap(), report)
}

#[test]
fn at_session_is_paced_exact_and_reproducible() {
    let (tr, report) = session();
    let (again, _) = session();
    if std::env::var("SOCKET_TEST_DUMP").is_ok() { for l in &tr.lines { eprintln!("{l:.200}"); } }
    assert_eq!(tr.lines, again.lines, "the same peer must see the same session twice");
    assert_eq!(tr.lines[0], "{\"type\":\"hello\",\"version\":1,\"chip\":\"esp32\"}");

    // What left UART1: the command, then the echo of everything the peer sent, nothing lost.
    let sent: Vec<u8> = tr.tx.iter().flat_map(|(_, _, port, d)| { assert_eq!(*port, 1, "only the port the peer asked for"); d.clone() }).collect();
    let mut expect = b"AT\r\r\nOK\r\n$TICK\r\n".to_vec();
    expect.extend(burst());
    assert_eq!(sent.len(), expect.len(), "{:?}", String::from_utf8_lossy(&sent[..sent.len().min(40)]));
    assert!(sent == expect, "the echo differs from what the peer sent");
    assert_eq!(tr.commands, ["AT"]);

    // Byte times, at the FIFO write: the echo of byte k of an answer comes (k + 1) characters
    // after the answer's time. A message carries its first and last byte's times.
    let at = |offset: usize| byte_time(&tr, offset).unwrap_or_else(|| panic!("byte {offset} is inside a message"));
    let ok = 3;                                  // index of the first echoed byte of "\r\nOK\r\n"
    assert!(at(2) < 1_000, "AT\\r goes out at once: {}", at(2));
    let answer = at(2) + MS;                     // the peer answers one latency after the \r
    assert!((answer + CHAR_NS..answer + CHAR_NS + SLACK).contains(&at(ok)), "first OK byte echoed at {} ns", at(ok));
    let tick = ok + 6;
    assert!((5 * MS + CHAR_NS..5 * MS + CHAR_NS + SLACK).contains(&at(tick)), "first $TICK byte echoed at {} ns", at(tick));
    let last = sent.len() - 1;
    assert!((10 * MS + 1500 * CHAR_NS..10 * MS + 1500 * CHAR_NS + 1500 + SLACK).contains(&at(last)), "last burst byte echoed at {} ns", at(last));

    // Not a round trip per byte: one sync per latency window of echo, plus the peer's own times.
    assert!(tr.syncs < 200, "{} syncs for {} bytes", tr.syncs, sent.len());
    assert!(report.contains(&format!("{} syncs, {} bytes to it, {} from it, 1 guest resets", tr.syncs, sent.len(), 6 + 7 + 1500)), "{report}");

    // The reset reaches the peer at the time it happened, then the run's end.
    let end = 150 * MS;
    assert_eq!(tr.resets.len(), 1);
    assert!(tr.resets[0].0 >= end && tr.resets[0].0 < end + 10_000, "{:?}", tr.resets);
    assert_eq!(tr.stop, Some(tr.resets[0].0));
    assert!(tr.lines.last().unwrap().starts_with("{\"type\":\"stop\""), "{:?}", tr.lines.last());
}

/// A peer that hangs up: the board gives up on it, names it, and the guest is not harmed. The CLI
/// ends the run there instead (`exit_on_failure`).
#[test]
fn a_peer_that_hangs_up_is_named_in_the_error() {
    let (endpoint, handle) = peer(Script { hang_up: true, ..script() });
    let board = SocketBoard::connect(&endpoint, "esp32", HZ).unwrap();
    let mut m = machine(board);
    handle.join().unwrap();
    m.max_cycles = HZ * 3 / 1000;
    assert!(matches!(m.run(u64::MAX), Stop::Halted));
    let report = m.bus.board.report();
    assert!(report.contains(&format!("[socket] peer {endpoint}: closed the connection while the emulator waited for done after sync t=")), "{report}");
}

/// Malformed peer output is a protocol error naming the field, not a panic.
#[cfg(unix)]
#[test]
fn bad_peer_messages_are_errors() {
    for (reply, expect) in [
        ("{\"type\":\"hello\",\"version\":2}", "speaks protocol version 2"),
        ("{\"type\":\"hello\",\"version\":1,\"latency\":-5}", "latency: expected nanoseconds or null"),
        ("{\"type\":\"hello\",\"version\":1,\"uarts\":[40]}", "uarts: bad port"),
        ("not json", "bad message"),
    ] {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        std::io::Write::write_all(&mut &b, format!("{reply}\n").as_bytes()).unwrap();
        let e = SocketBoard::over("test".into(), Box::new(a.try_clone().unwrap()), Box::new(a), "esp32", HZ).err().expect(reply);
        assert!(e.starts_with("[socket] peer test: ") && e.contains(expect), "{reply}: {e}");
    }
    for (rx, expect) in [
        ("{\"type\":\"uart_rx\",\"t\":0,\"pin\":64,\"baud\":115200,\"data\":\"41\"}", "pin: expected an integer in 0..=63"),
        ("{\"type\":\"uart_rx\",\"t\":0,\"pin\":26,\"baud\":0,\"data\":\"41\"}", "baud: expected an integer"),
        ("{\"type\":\"uart_rx\",\"t\":0,\"pin\":26,\"baud\":115200,\"data\":\"4\"}", "data must be hex"),
        ("{\"type\":\"gpio_in\",\"t\":0}", "unexpected message \"gpio_in\""),
    ] {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        std::io::Write::write_all(&mut &b, format!("{{\"type\":\"hello\",\"version\":1,\"quiet_until\":0}}\n{rx}\n").as_bytes()).unwrap();
        let board = SocketBoard::over("test".into(), Box::new(a.try_clone().unwrap()), Box::new(a), "esp32", HZ).unwrap();
        let mut m = machine(board);
        m.max_cycles = HZ / 1000;
        assert!(matches!(m.run(u64::MAX), Stop::Halted));
        let report = m.bus.board.report();
        assert!(report.contains(expect), "{rx}: {report}");
        drop(b);
    }
}

/// Every chip whose reboot path calls `guest_reset` does so once per chip reset, with the cause
/// the ROM will report, and not at power-on.
#[test]
fn guest_reset_reaches_the_board_on_the_classic_s3_and_c6() {
    use std::{cell::RefCell, rc::Rc};
    struct Probe(Rc<RefCell<Vec<(u64, u32)>>>);
    impl esp_soc::BoardModel for Probe {
        fn name(&self) -> &'static str { "reset-probe" }
        fn guest_reset(&mut self, cycle: u64, cause: u32) { self.0.borrow_mut().push((cycle, cause)); }
    }
    fn check<S: Soc>(mut m: esp_soc::Machine<S>, board: impl FnOnce(&mut esp_soc::Machine<S>, Probe)) {
        let seen = Rc::new(RefCell::new(Vec::new()));
        board(&mut m, Probe(seen.clone()));
        assert!(seen.borrow().is_empty(), "{}: attaching is not a reset", S::NAME);
        m.bus.request_reset(12);
        let cause = m.reboot();
        assert_eq!(*seen.borrow(), [(m.bus.cycles(), cause)], "{}", S::NAME);
        assert_eq!(cause, 12, "{}", S::NAME);
    }
    use esp_soc::SocBus as _;
    check(esp32::machine([0; 6], 4 << 20), |m, p| { m.bus.board = Box::new(p); m.bus.attach_board_devices(); });
    check(esp32s3::machine([0; 6]), |m, p| { m.bus.board = Box::new(p); m.bus.attach_board_devices(); });
    check(esp32c6::machine([0; 6], 4 << 20), |m, p| { m.bus.board = Box::new(p); m.bus.attach_board_devices(); });
}
