//! A small `--board socket:` peer (protocol v1, docs/socket-board.md): an AT modem on one UART
//! that answers `AT` with `OK`, `ATI` with its name and anything else starting with `AT` with
//! `ERROR`, after `latency`, plus output of its own at fixed times (`unprompted`), the way a GNSS
//! receiver streams NMEA. It is deterministic: what it sends depends only on what it received.
//! The CI test (`cli/tests/socket_board.rs`) runs it in a thread; `cli/examples/socket_at_peer.rs`
//! serves it on a socket for a real run.
#![allow(dead_code)]
use esp_soc::json::{parse_json, Json};
use esp_soc::web::{hex_decode, hex_encode};
use std::io::{BufRead, Write};

pub struct Script {
    /// UART port whose transmissions are commands; the peer asks for this port only.
    pub port: usize,
    /// Chip GPIO and baud the answers go to.
    pub rx_pin: u8,
    pub baud: u32,
    /// Nanoseconds from a command's end to its answer.
    pub latency: u64,
    /// Output at fixed emulated times (ns), in order.
    pub unprompted: Vec<(u64, Vec<u8>)>,
    /// Close the connection right after `hello` (a peer that dies).
    pub hang_up: bool,
}

impl Default for Script {
    fn default() -> Self { Script { port: 1, rx_pin: 26, baud: 115_200, latency: 1_000_000, unprompted: Vec::new(), hang_up: false } }
}

/// What the peer saw.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Transcript {
    /// Every line the emulator sent, verbatim.
    pub lines: Vec<String>,
    /// `uart_tx` payloads: (t, t_end, port, bytes).
    pub tx: Vec<(u64, u64, usize, Vec<u8>)>,
    /// `reset` events: (t, cause).
    pub resets: Vec<(u64, u32)>,
    pub syncs: u64,
    /// The `stop` time, if the run ended cleanly.
    pub stop: Option<u64>,
    /// Commands recognised, in order.
    pub commands: Vec<String>,
}

fn answer(cmd: &str) -> &'static [u8] {
    match cmd { "AT" => b"\r\nOK\r\n", "ATI" => b"\r\nesp32sim AT peer\r\nOK\r\n", _ => b"\r\nERROR\r\n" }
}

fn uart_rx(t: u64, s: &Script, data: &[u8]) -> String {
    format!("{{\"type\":\"uart_rx\",\"t\":{t},\"pin\":{},\"baud\":{},\"data\":\"{}\"}}", s.rx_pin, s.baud, hex_encode(data))
}

/// Serve one connection until the emulator's `stop` or the end of its stream.
pub fn serve(input: &mut dyn BufRead, output: &mut dyn Write, s: &Script) -> Transcript {
    let mut tr = Transcript::default();
    let mut line = String::new();
    let mut command = Vec::new();
    // answers owed: (earliest time, bytes)
    let mut owed: Vec<(u64, &'static [u8])> = Vec::new();
    let mut next = 0usize;
    let quiet = |next: usize| s.unprompted.get(next).map_or("null".to_string(), |(t, _)| t.to_string());
    loop {
        line.clear();
        if input.read_line(&mut line).unwrap_or(0) == 0 { break; }
        let text = line.trim_end().to_string();
        tr.lines.push(text.clone());
        let msg = parse_json(&text).expect("the emulator sends JSON");
        let t = msg.i64_or("t", 0) as u64;
        match msg.str_or("type", "") {
            "hello" => {
                assert_eq!(msg.i64_or("version", 0), 1);
                writeln!(output, "{{\"type\":\"hello\",\"version\":1,\"name\":\"at-peer\",\"latency\":{},\"quiet_until\":{},\"uarts\":[{}]}}", s.latency, quiet(next), s.port).unwrap();
                output.flush().unwrap();
                if s.hang_up { return tr; }
            }
            "uart_tx" => {
                let data = msg.get("data").and_then(Json::as_str).and_then(hex_decode).expect("hex data");
                let (t_end, port) = (msg.i64_or("t_end", 0) as u64, msg.i64_or("port", -1) as usize);
                tr.tx.push((t, t_end, port, data.clone()));
                if port != s.port { continue; }
                for b in data {
                    if b != b'\r' { command.push(b); continue; }
                    let cmd = String::from_utf8_lossy(&command).trim_start_matches('\n').to_string();
                    command.clear();
                    if cmd.starts_with("AT") { owed.push((t_end + s.latency, answer(&cmd))); tr.commands.push(cmd); }
                }
            }
            "reset" => tr.resets.push((t, msg.i64_or("cause", -1) as u32)),
            "sync" => {
                tr.syncs += 1;
                for (at, data) in owed.drain(..) { writeln!(output, "{}", uart_rx(at.max(t), s, data)).unwrap(); }
                while let Some((at, data)) = s.unprompted.get(next).filter(|(at, _)| *at <= t) { writeln!(output, "{}", uart_rx(*at, s, data)).unwrap(); next += 1; }
                writeln!(output, "{{\"type\":\"done\",\"t\":{t},\"quiet_until\":{}}}", quiet(next)).unwrap();
                output.flush().unwrap();
            }
            "stop" => { tr.stop = Some(t); break; }
            other => panic!("unknown message {other}"),
        }
    }
    tr
}
