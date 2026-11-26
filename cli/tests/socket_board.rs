//! `--board socket:` against the in-repo AT peer (`support/at_peer.rs`), on a classic ESP32 guest
//! small enough to reason about: `MODEM` routes UART1 TX to GPIO27 and RX from GPIO26 through the
//! GPIO matrix at 115273 baud (the peer's 115200 is within the 3 % match), sends `AT\r`, then
//! `ATI\r` about 0.25 ms later (inside one latency window), then echoes every byte it receives back
//! out of UART1. The peer answers each command one latency after its own `\r`, sends a line of its
//! own at 5 ms and a 1500-byte burst at
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
/// esp-14.2.0_20260121) and linked at 0x40080000; ten literals, then `_start` at 0x40080028:
/// ```text
/// _start: l32r a2, 0x3ff49028      # IO_MUX GPIO26
///         l32r a3, 0x2200; s32i a3, a2, 0   # GPIO26: FUN_IE, MCU_SEL 2 (GPIO)
///         l32r a3, 0x2000; s32i a3, a2, 4   # GPIO27: MCU_SEL 2
///         l32r a2, 0x3ff4459c; movi a3, 17; s32i a3, a2, 0      # FUNC27_OUT_SEL = U1TXD (17)
///         l32r a2, 0x3ff44024; l32r a3, 1 << 27; s32i a3, a2, 0 # ENABLE_W1TS GPIO27
///         l32r a2, 0x3ff44174; movi a3, 0x9a; s32i a3, a2, 0    # FUNC17_IN_SEL (U1RXD) = SIG_IN_SEL | 26
///         l32r a2, 0x3ff50000      # UART1
///         movi a3, 694; s32i a3, a2, 0x14                       # CLKDIV 694: 115273 baud
///                                   # at the emulator's fixed 80 MHz APB; at the 40 MHz XTAL reset
///                                   # clock silicon would run 57636 baud
///         l32r a3, 0x0800001c; s32i a3, a2, 0x20                # CONF0: TICK_REF_ALWAYS_ON, 8N1
///         movi a3, 'A'; s32i a3, a2, 0; movi a3, 'T'; s32i a3, a2, 0; movi a3, 13; s32i a3, a2, 0
///         l32r a6, 5000                                        # about 0.25 ms at the 40 MHz reset clock
/// delay:  addi a6, a6, -1; bnez a6, delay
///         movi a3, 'A'; s32i a3, a2, 0; movi a3, 'T'; s32i a3, a2, 0
///         movi a3, 'I'; s32i a3, a2, 0; movi a3, 13; s32i a3, a2, 0
/// loop:   l32i a4, a2, 0x1c; extui a4, a4, 0, 8; beqz a4, loop  # STATUS.RXFIFO_CNT
///         l32i a5, a2, 0; s32i a5, a2, 0; j loop                # echo one byte
/// ```
/// Register offsets: IDF v5.5.4 (as v4.4) components/soc/esp32/register/soc/io_mux_reg.h
/// (PERIPHS_IO_MUX_GPIO26_U 0x28, GPIO27_U 0x2c), gpio_reg.h (GPIO_ENABLE_W1TS_REG 0x24,
/// GPIO_FUNC0_IN_SEL_CFG_REG 0x130, GPIO_FUNC0_OUT_SEL_CFG_REG 0x530), gpio_sig_map.h
/// (U1RXD_IN_IDX / U1TXD_OUT_IDX 17) and uart_reg.h (UART_CLKDIV_REG 0x14, UART_CONF0_REG 0x20,
/// UART_STATUS_REG 0x1c).
const MODEM: [u8; 140] = [
    0x28, 0x90, 0xf4, 0x3f, 0x00, 0x22, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x9c, 0x45, 0xf4, 0x3f, 0x24, 0x40, 0xf4, 0x3f,
    0x00, 0x00, 0x00, 0x08, 0x74, 0x41, 0xf4, 0x3f, 0x00, 0x00, 0xf5, 0x3f, 0x1c, 0x00, 0x00, 0x08, 0x88, 0x13, 0x00, 0x00,
    0x21, 0xf6, 0xff, 0x31, 0xf6, 0xff, 0x39, 0x02, 0x31, 0xf6, 0xff, 0x39, 0x12, 0x21, 0xf5, 0xff, 0x1c, 0x13, 0x39, 0x02,
    0x21, 0xf5, 0xff, 0x31, 0xf5, 0xff, 0x39, 0x02, 0x21, 0xf5, 0xff, 0x32, 0xa0, 0x9a, 0x39, 0x02, 0x21, 0xf4, 0xff, 0x32,
    0xa2, 0xb6, 0x39, 0x52, 0x31, 0xf3, 0xff, 0x39, 0x82, 0x4c, 0x13, 0x39, 0x02, 0x5c, 0x43, 0x39, 0x02, 0x0c, 0xd3, 0x39,
    0x02, 0x61, 0xef, 0xff, 0x0b, 0x66, 0x56, 0xa6, 0xff, 0x4c, 0x13, 0x39, 0x02, 0x5c, 0x43, 0x39, 0x02, 0x4c, 0x93, 0x39,
    0x02, 0x0c, 0xd3, 0x39, 0x02, 0x48, 0x72, 0x40, 0x40, 0x74, 0x16, 0x74, 0xff, 0x58, 0x02, 0x59, 0x02, 0x06, 0xfc, 0xff,
];
const BASE: u32 = 0x4008_0000;
const ENTRY: u32 = 0x4008_0028;
const HZ: u64 = <esp32::Esp32 as Soc>::CPU_HZ;
const MS: u64 = 1_000_000;
/// One 8N1 character at the guest's 115273 baud, as the board paces it at the peer's 115200.
const CHAR_NS: u64 = 10 * 1_000_000_000 / 115_200;
/// The board sees time at device ticks (one 64-instruction quantum, 1.6 µs at the 40 MHz reset
/// clock) and the guest needs a few instructions to echo a byte.
const SLACK: u64 = 5_000;
/// Wall time a test peer has to answer; the silent-peer test waits this long.
const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(300);

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

    // What left UART1: the commands, then the echo of everything the peer sent, nothing lost.
    let sent: Vec<u8> = tr.tx.iter().flat_map(|(_, _, port, d)| { assert_eq!(*port, 1, "only the port the peer asked for"); d.clone() }).collect();
    let ati = b"\r\nesp32sim AT peer\r\nOK\r\n";
    let mut expect = b"AT\rATI\r\r\nOK\r\n".to_vec();
    expect.extend(ati);
    expect.extend(b"$TICK\r\n");
    expect.extend(burst());
    assert_eq!(sent.len(), expect.len(), "{:?}", String::from_utf8_lossy(&sent[..sent.len().min(40)]));
    assert!(sent == expect, "the echo differs from what the peer sent");
    assert_eq!(tr.commands, ["AT", "ATI"]);
    assert!(tr.unknown.is_empty(), "{:?}", tr.unknown);

    // Byte times, at the FIFO write: a message carries its first and last byte's times, and the
    // two commands, written 0.25 ms apart, are two messages, so each answer is timed from its own
    // \r. The echo of byte k of an answer comes (k + 1) characters after the answer starts on the
    // line: its time, or the end of the answer before it on the same line.
    let at = |offset: usize| byte_time(&tr, offset).unwrap_or_else(|| panic!("byte {offset} is inside a message"));
    let (at_cr, ati_start, ati_cr, ok, ati_answer, tick) = (2, 3, 6, 7, 13, 37);
    assert!(at(at_cr) < 1_000, "AT\\r goes out at once: {}", at(at_cr));
    assert_eq!(tr.tx[0].3, b"AT\r", "one FIFO fill, one message");
    assert!(at(ati_start) > 200_000, "ATI\\r is its own message: {}", at(ati_start));
    let answer = at(at_cr) + MS;
    assert!((answer + CHAR_NS..answer + CHAR_NS + SLACK).contains(&at(ok)), "first OK byte echoed at {} ns", at(ok));
    let ati_on_line = (at(ati_cr) + MS).max(answer + 6 * CHAR_NS);
    assert!((ati_on_line + CHAR_NS..ati_on_line + CHAR_NS + SLACK).contains(&at(ati_answer)), "first ATI answer byte echoed at {} ns", at(ati_answer));
    assert!((5 * MS + CHAR_NS..5 * MS + CHAR_NS + SLACK).contains(&at(tick)), "first $TICK byte echoed at {} ns", at(tick));
    let last = sent.len() - 1;
    assert!((10 * MS + 1500 * CHAR_NS..10 * MS + 1500 * CHAR_NS + 1500 + SLACK).contains(&at(last)), "last burst byte echoed at {} ns", at(last));

    // Not a round trip per byte: one sync per latency window of echo, plus the peer's own times.
    assert!(tr.syncs < 200, "{} syncs for {} bytes", tr.syncs, sent.len());
    assert!(report.contains(&format!("{} syncs, {} bytes to it, {} from it, 1 guest reset", tr.syncs, sent.len(), 6 + ati.len() + 7 + 1500)), "{report}");

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
        ("{\"type\":\"hello\",\"version\":1,\"quiet_until\":null,\"latency\":-5}", "latency: expected nanoseconds or null"),
        ("{\"type\":\"hello\",\"version\":1,\"latency\":null}", "hello without quiet_until (null: never)"),
        ("{\"type\":\"hello\",\"version\":1,\"quiet_until\":null,\"latency\":null,\"uarts\":[40]}", "uarts: bad port"),
        ("not json", "bad message"),
    ] {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        std::io::Write::write_all(&mut &b, format!("{reply}\n").as_bytes()).unwrap();
        let e = SocketBoard::over("test".into(), Box::new(a), "esp32", HZ, TIMEOUT).err().expect(reply);
        assert!(e.starts_with("[socket] peer test: ") && e.contains(expect), "{reply}: {e}");
    }
    for (rx, expect) in [
        ("{\"type\":\"uart_rx\",\"t\":0,\"pin\":64,\"baud\":115200,\"data\":\"41\"}", "pin: expected an integer in 0..=63"),
        ("{\"type\":\"uart_rx\",\"t\":0,\"pin\":26,\"baud\":0,\"data\":\"41\"}", "baud: expected an integer"),
        ("{\"type\":\"uart_rx\",\"t\":0,\"pin\":26,\"baud\":115200,\"data\":\"4\"}", "data must be hex"),
        ("{\"type\":\"gpio_in\",\"t\":0}", "unexpected message \"gpio_in\""),
        ("{\"type\":\"done\",\"latency\":0}", "done without quiet_until (null: never)"),
    ] {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        std::io::Write::write_all(&mut &b, format!("{{\"type\":\"hello\",\"version\":1,\"quiet_until\":0,\"latency\":null}}\n{rx}\n").as_bytes()).unwrap();
        let board = SocketBoard::over("test".into(), Box::new(a), "esp32", HZ, TIMEOUT).unwrap();
        let mut m = machine(board);
        m.max_cycles = HZ / 1000;
        assert!(matches!(m.run(u64::MAX), Stop::Halted));
        let report = m.bus.board.report();
        assert!(report.contains(expect), "{rx}: {report}");
        drop(b);
    }
}

/// A peer that stops answering (or never reads what it is sent) is given up on after the timeout,
/// from the moment the sync went out, not left to stall the run.
#[cfg(unix)]
#[test]
fn a_silent_peer_times_out() {
    let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    std::io::Write::write_all(&mut &b, b"{\"type\":\"hello\",\"version\":1,\"quiet_until\":1000,\"latency\":null}\n{\"type\":\"log\",").unwrap();
    let board = SocketBoard::over("test".into(), Box::new(a), "esp32", HZ, TIMEOUT).unwrap();
    let mut m = machine(board);
    m.max_cycles = HZ / 1000;
    let t0 = std::time::Instant::now();
    assert!(matches!(m.run(u64::MAX), Stop::Halted));
    let waited = t0.elapsed();
    let report = m.bus.board.report();
    let prefix = "[socket] peer test: no answer within 300ms while the emulator waited for done after sync t=";
    let t: u64 = report.split(prefix).nth(1).and_then(|r| r.split(|c: char| !c.is_ascii_digit()).next()).and_then(|t| t.parse().ok()).unwrap_or_else(|| panic!("{report}"));
    // the first device tick at or after quiet_until (1600 ns, one 64-instruction quantum, when written)
    assert!((1000..1000 + SLACK).contains(&t), "sync at {t} ns: {report}");
    assert!(waited >= TIMEOUT && waited < TIMEOUT * 10, "{waited:?}");
    drop(b);
}

/// A long line (a recording replayed as one `uart_rx`) arrives in many reads and is taken whole.
#[cfg(unix)]
#[test]
fn a_long_line_arrives_in_pieces() {
    let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    let data = "41".repeat(200_000);
    let feeder = std::thread::spawn(move || {
        let mut w = &b;
        std::io::Write::write_all(&mut w, b"{\"type\":\"hello\",\"version\":1,\"quiet_until\":0,\"latency\":null}\n").unwrap();
        let line = format!("{{\"type\":\"uart_rx\",\"t\":0,\"pin\":26,\"baud\":115200,\"data\":\"{data}\"}}\n{{\"type\":\"done\",\"quiet_until\":null}}\n");
        for piece in line.as_bytes().chunks(1000) { std::io::Write::write_all(&mut w, piece).unwrap(); }
        b
    });
    let board = SocketBoard::over("test".into(), Box::new(a), "esp32", HZ, std::time::Duration::from_secs(10)).unwrap();
    let mut m = machine(board);
    m.max_cycles = HZ / 1000;
    assert!(matches!(m.run(u64::MAX), Stop::Halted));
    let report = m.bus.board.report();
    // all of it scheduled; in 1 ms the guest echoes the first 11 characters (86.8 µs each) after its AT\r ATI\r
    assert!(report.contains("1 syncs, 18 bytes to it, 200000 from it, 0 guest resets") && !report.contains('\n'), "{report}");
    drop(feeder.join().unwrap());
}

/// The board on one end of a socket pair whose other end has already said `peer_says`; nothing
/// reads what the board writes.
#[cfg(unix)]
fn scripted(peer_says: &str, timeout: std::time::Duration) -> (SocketBoard, std::os::unix::net::UnixStream) {
    let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    std::io::Write::write_all(&mut &b, peer_says.as_bytes()).unwrap();
    (SocketBoard::over("test".into(), Box::new(a), "esp32", HZ, timeout).unwrap(), b)
}
/// The first bus cycle at or after `ns`.
fn cyc(ns: u64) -> u64 { (ns as u128 * HZ as u128).div_ceil(1_000_000_000) as u64 }
fn uart1() -> esp_soc::uart::UartRoute { esp_soc::uart::UartRoute::new(1, 1 << 27, Some(26), Some(115_200)) }

/// The receive queue holds about one byte per received byte, and refuses to hold more than its
/// limit (a named protocol error, not unbounded memory).
#[cfg(unix)]
#[test]
fn the_receive_queue_is_compact_and_bounded() {
    use esp_soc::BoardModel;
    let n = 2 << 20;
    for limit in [esp32sim::socket_board::RX_QUEUE_MAX, 1 << 20] {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let feeder = std::thread::spawn(move || {
            let mut w = &b;
            let line = format!("{{\"type\":\"hello\",\"version\":1,\"quiet_until\":0,\"latency\":null}}\n{{\"type\":\"uart_rx\",\"t\":0,\"pin\":26,\"baud\":115200,\"data\":\"{}\"}}\n{{\"type\":\"done\",\"quiet_until\":null}}\n", "5a".repeat(n));
            std::io::Write::write_all(&mut w, line.as_bytes()).unwrap();
            b
        });
        let mut board = SocketBoard::over("test".into(), Box::new(a), "esp32", HZ, std::time::Duration::from_secs(10)).unwrap();
        board.set_limits(esp32sim::socket_board::LINE_MAX, limit);
        board.advance_to(0);
        let _b = feeder.join().unwrap();
        if limit > n {
            assert_eq!(board.error(), None);
            let held = board.rx_queue_memory();
            assert!((n..n + 4096).contains(&held), "{held} bytes held for {n} received");
            let first = board.uart_rx(cyc(CHAR_NS));
            assert_eq!(first.iter().map(|i| i.data.len()).sum::<usize>(), 1, "one character per character time");
        } else {
            let e = board.error().unwrap_or("");
            assert!(e.contains(&format!("would hold {n} received bytes not yet on the line; the limit is {limit}")), "{e}");
            assert_eq!(board.rx_queue_memory(), 0);
        }
    }
}

/// `quiet_until` at or before its sync ("at any moment") costs one exchange per `MIN_QUIET_NS`,
/// and the board never syncs twice at one cycle (a GPIO read and a tick both advance it).
#[cfg(unix)]
#[test]
fn a_quiet_until_in_the_past_costs_one_sync_per_min_step() {
    use esp_soc::BoardModel;
    let step = esp32sim::socket_board::MIN_QUIET_NS;
    let (mut board, _b) = scripted(&format!("{{\"type\":\"hello\",\"version\":1,\"quiet_until\":0,\"latency\":null}}\n{}", "{\"type\":\"done\",\"quiet_until\":0}\n".repeat(4)), TIMEOUT);
    board.advance_to(0);
    board.advance_to(0);
    assert_eq!(board.stats().syncs, 1);
    for k in 1..=3 {
        assert_eq!(board.next_deadline(), Some(cyc(k * step)), "sync {k}");
        board.advance_to(cyc(k * step) - 1);
        assert_eq!(board.stats().syncs, k);
        board.advance_to(cyc(k * step));
        board.advance_to(cyc(k * step));
        assert_eq!(board.stats().syncs, k + 1);
    }
    assert_eq!(board.error(), None);
}

/// With no latency, an event at the cycle of the last sync waits for the next cycle: never two
/// syncs at one cycle, however often the bus advances the board there.
#[cfg(unix)]
#[test]
fn never_two_syncs_at_one_cycle() {
    use esp_soc::BoardModel;
    let (mut board, _b) = scripted(&format!("{{\"type\":\"hello\",\"version\":1,\"quiet_until\":null,\"latency\":0}}\n{}", "{\"type\":\"done\",\"quiet_until\":null}\n".repeat(2)), TIMEOUT);
    let c = cyc(5000);
    board.uart_tx(c, uart1(), b'A');
    board.advance_to(c);
    assert_eq!(board.stats().syncs, 1);
    board.uart_tx(c, uart1(), b'B');
    board.advance_to(c);
    board.advance_to(c);
    assert_eq!(board.stats().syncs, 1, "a second sync at the same cycle");
    assert_eq!(board.next_deadline(), Some(c + 1));
    board.advance_to(c + 1);
    assert_eq!(board.stats().syncs, 2);
}

/// A `uart_rx` stamped before the sync that brought it starts on the line at that sync, and one
/// stamped before the peer's own promise is counted as late.
#[cfg(unix)]
#[test]
fn a_uart_rx_from_the_past_starts_at_its_sync() {
    use esp_soc::BoardModel;
    let (mut board, _b) = scripted("{\"type\":\"hello\",\"version\":1,\"quiet_until\":1000000,\"latency\":null}\n\
        {\"type\":\"uart_rx\",\"t\":500000,\"pin\":26,\"baud\":115200,\"data\":\"41\"}\n{\"type\":\"done\",\"quiet_until\":null}\n", TIMEOUT);
    let sync = cyc(1_000_000) + 7;
    board.advance_to(sync);
    assert_eq!(board.stats().syncs, 1);
    let due = board.next_deadline().unwrap();
    assert_eq!(due, sync + cyc(CHAR_NS), "one character after the sync, not after t=500000");
    assert!(board.uart_rx(due - 1).is_empty());
    assert_eq!(board.uart_rx(due).len(), 1);
    assert_eq!(board.stats().late_rx, 1);
    assert!(board.report().contains(", 1 uart_rx stamped before the peer's promises allowed"), "{}", board.report());
}

/// A `done` with a new `latency` moves the next sync after an event, and a guest reset is an event
/// the peer has one latency to answer.
#[cfg(unix)]
#[test]
fn latency_from_done_and_resets_set_the_next_sync() {
    use esp_soc::BoardModel;
    let (mut board, _b) = scripted("{\"type\":\"hello\",\"version\":1,\"quiet_until\":null,\"latency\":1000}\n\
        {\"type\":\"done\",\"quiet_until\":null,\"latency\":50000}\n{\"type\":\"done\",\"quiet_until\":null}\n", TIMEOUT);
    assert_eq!(board.next_deadline(), None, "nothing to answer, nothing of its own");
    board.uart_tx(cyc(2000), uart1(), b'A');
    assert_eq!(board.next_deadline(), Some(cyc(3000)));
    board.advance_to(cyc(3000));
    assert_eq!(board.stats().syncs, 1);
    board.uart_tx(cyc(10_000), uart1(), b'B');
    assert_eq!(board.next_deadline(), Some(cyc(60_000)), "the latency the done named");
    board.advance_to(cyc(60_000));
    board.guest_reset(cyc(100_000), 12);
    assert_eq!(board.next_deadline(), Some(cyc(150_000)), "one latency after the reset");
    assert_eq!(board.error(), None);
}

/// A line longer than the limit is an error, whether it is still arriving or already complete.
#[cfg(unix)]
#[test]
fn a_line_over_the_limit_is_an_error() {
    use esp_soc::BoardModel;
    for complete in [false, true] {
        let tail = if complete { "\"}\n" } else { "" };
        let (mut board, _b) = scripted(&format!("{{\"type\":\"hello\",\"version\":1,\"quiet_until\":0,\"latency\":null}}\n{{\"type\":\"log\",\"line\":\"{}{tail}", "x".repeat(3000)), TIMEOUT);
        board.set_limits(1000, esp32sim::socket_board::RX_QUEUE_MAX);
        board.advance_to(0);
        assert!(board.error().is_some_and(|e| e.contains("sent a line longer than 1000 bytes")), "complete {complete}: {:?}", board.error());
    }
}

/// A peer that hangs up while the board is writing to it is named as having closed the
/// connection, not left as a bare broken pipe.
#[cfg(unix)]
#[test]
fn a_peer_gone_while_written_to_is_named() {
    use esp_soc::BoardModel;
    let (mut board, b) = scripted("{\"type\":\"hello\",\"version\":1,\"quiet_until\":null,\"latency\":null}\n", TIMEOUT);
    drop(b);
    for i in 0..1u64 << 20 {
        board.uart_tx(i / 256, uart1(), b'U');
        if board.error().is_some() { break; }
    }
    let e = board.error().unwrap_or("no error");
    assert!(e.contains("[socket] peer test: closed the connection while the emulator wrote to it"), "{e}");
}

/// A peer that never reads what the board writes fails at the write timeout, not never.
#[cfg(unix)]
#[test]
fn a_peer_that_never_reads_times_out() {
    use esp_soc::BoardModel;
    let (mut board, _b) = scripted("{\"type\":\"hello\",\"version\":1,\"quiet_until\":null,\"latency\":null}\n", TIMEOUT);
    let t0 = std::time::Instant::now();
    for i in 0..64u64 << 20 {
        board.uart_tx(i / 256, uart1(), b'U');
        if board.error().is_some() { break; }
    }
    let waited = t0.elapsed();
    let e = board.error().unwrap_or("no error");
    assert!(e.contains("took nothing the emulator wrote for 300ms"), "{e}");
    assert!(waited >= TIMEOUT, "{waited:?}");
}

/// Every chip's reboot path calls `guest_reset` once per chip reset, with the cause the ROM will
/// report and the board's own clock (the time of its last `advance_to`: bus cycles, emulated ns on
/// the S31), and not at power-on.
#[test]
fn guest_reset_reaches_the_board_on_every_chip() {
    use std::{cell::RefCell, rc::Rc};
    #[derive(Default)]
    struct Seen { advanced: u64, resets: Vec<(u64, u32)> }
    struct Probe(Rc<RefCell<Seen>>);
    impl esp_soc::BoardModel for Probe {
        fn name(&self) -> &'static str { "reset-probe" }
        fn advance_to(&mut self, cycle: u64) { self.0.borrow_mut().advanced = cycle; }
        // always due, so every chip advances the board each tick (the C3 only advances a due board)
        fn next_deadline(&self) -> Option<u64> { Some(self.0.borrow().advanced + 1) }
        fn guest_reset(&mut self, cycle: u64, cause: u32) { self.0.borrow_mut().resets.push((cycle, cause)); }
    }
    fn chip<S: Soc>(_: &esp_soc::Machine<S>) -> &'static str { S::NAME }
    macro_rules! check {
        ($m:expr, $clock_is_cycles:expr) => {{
            let mut m = $m;
            let seen = Rc::new(RefCell::new(Seen::default()));
            m.bus.board = Box::new(Probe(seen.clone()));
            m.bus.attach_board_devices();
            let name = chip(&m);
            assert!(seen.borrow().resets.is_empty(), "{name}: attaching is not a reset");
            for _ in 0..100 { emu_core::Bus::tick(&mut m.bus, 1000); }
            m.bus.request_reset(12);
            let cause = m.reboot();
            assert_eq!(cause, 12, "{name}");
            let s = seen.borrow();
            assert!(s.advanced > 0, "{name}: the board's clock did not move");
            assert_eq!(s.resets, [(s.advanced, 12)], "{name}: reset at the board's clock");
            assert_eq!(s.advanced == m.bus.cycles(), $clock_is_cycles, "{name}: board clock {} vs bus cycles {}", s.advanced, m.bus.cycles());
        }};
    }
    use esp_soc::SocBus as _;
    check!(esp32::machine([0; 6], 4 << 20), true);
    check!(esp32s3::machine([0; 6]), true);
    check!(esp32c3::machine([0; 6], 4 << 20), true);
    check!(esp32c6::machine([0; 6], 4 << 20), true);
    check!(esp32p4::machine_rev::<esp32p4::RevLt3>([0; 6], 16 << 20), true);
    check!(esp32s31::machine([0; 6], 2 << 20), false);
}
