//! The CI test's AT peer (`cli/tests/support/at_peer.rs`) on a socket, for a real run:
//!   cargo run --release -p esp32sim --example socket_at_peer -- /tmp/board.sock [PORT RX_PIN BAUD]
//!   esp32sim --chip esp32 --board socket:/tmp/board.sock ...
//! A `HOST:PORT` argument listens on TCP instead (`--board tcp:HOST:PORT`). It serves one run and
//! prints what it saw.
#[path = "../tests/support/at_peer.rs"]
mod at_peer;
use std::io::BufReader;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(addr) = args.get(1) else { eprintln!("usage: socket_at_peer PATH|HOST:PORT [UART_PORT RX_PIN BAUD]"); std::process::exit(2) };
    let mut script = at_peer::Script::default();
    let num = |i: usize, what: &str| args.get(i).map(|v| v.parse::<u32>().unwrap_or_else(|_| { eprintln!("{what}: expected a number, got {v:?}"); std::process::exit(2) }));
    if let Some(p) = num(2, "UART_PORT") { script.port = p as usize; }
    if let Some(p) = num(3, "RX_PIN") { script.rx_pin = p as u8; }
    if let Some(b) = num(4, "BAUD") { script.baud = b; }
    let tr = if addr.contains(':') {
        let listener = std::net::TcpListener::bind(addr.as_str()).unwrap_or_else(|e| { eprintln!("{addr}: {e}"); std::process::exit(1) });
        eprintln!("[at-peer] listening on tcp:{}", listener.local_addr().unwrap());
        let (s, _) = listener.accept().unwrap();
        at_peer::serve(&mut BufReader::new(s.try_clone().unwrap()), &mut &s, &script)
    } else {
        serve_unix(addr, &script)
    };
    eprintln!("[at-peer] {} syncs, {} uart_tx messages, commands {:?}, resets {:?}, stop at {:?} ns", tr.syncs, tr.tx.len(), tr.commands, tr.resets, tr.stop);
}

#[cfg(unix)]
fn serve_unix(path: &str, script: &at_peer::Script) -> at_peer::Transcript {
    let _ = std::fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path).unwrap_or_else(|e| { eprintln!("{path}: {e}"); std::process::exit(1) });
    eprintln!("[at-peer] listening on socket:{path}");
    let (s, _) = listener.accept().unwrap();
    let tr = at_peer::serve(&mut BufReader::new(s.try_clone().unwrap()), &mut &s, script);
    let _ = std::fs::remove_file(path);
    tr
}
#[cfg(not(unix))]
fn serve_unix(_: &str, _: &at_peer::Script) -> at_peer::Transcript { eprintln!("Unix sockets need a Unix host; give HOST:PORT"); std::process::exit(2) }
