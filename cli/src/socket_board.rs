//! `--board socket:PATH` / `--board tcp:HOST:PORT`: a `BoardModel` whose devices run in another
//! process, reached over NDJSON on a Unix or TCP socket (protocol v1, docs/socket-board.md).
//!
//! The emulator keeps the clock and its normal run loop. The peer says how long it will stay quiet
//! (`quiet_until`) and how soon it can answer something it is sent (`latency`); the board waits for
//! it only at `min(quiet_until, first unanswered event + latency)`, which is its `next_deadline`.
//! There it sends `sync {t}` and reads the peer's output up to `done`. Between those points the
//! guest runs at full speed and the board's callbacks cost no exchange. Times are nanoseconds of
//! emulated time, derived from the bus's cycle counter, which never restarts, so a deterministic
//! peer gives a reproducible run.
//!
//! What the peer sends a UART is released one character time apart at the baud it names, from the
//! time it names (`UartInput` carries no line timing and the RX FIFO drops what overflows it, so a
//! 1500-byte PPP frame sent at once would mostly be lost). What the guest transmits is stamped at
//! the FIFO write (`uart_tx`), not when it would leave the wire.
use esp_soc::board::{BoardModel, VirtualCycle};
use esp_soc::json::{parse_json, Json};
use esp_soc::uart::{UartInput, UartRoute};
use esp_soc::web::{hex_decode, hex_encode, json_escape};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::time::Duration;

pub const VERSION: i64 = 1;
/// How long the board waits for the peer's answer to one message, in wall time.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Bytes of one `uart_tx` message at most; consecutive bytes on one route share a message.
const TX_RUN_MAX: usize = 256;

/// The two halves of a connection.
type Streams = (Box<dyn Read>, Box<dyn Write>);

/// Where the peer listens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint { Unix(std::path::PathBuf), Tcp(String) }
impl Endpoint {
    /// `socket:PATH` or `tcp:HOST:PORT`; `None` for any other board name.
    pub fn parse(spec: &str) -> Option<Result<Self, String>> {
        if let Some(path) = spec.strip_prefix("socket:") {
            if path.is_empty() { return Some(Err("--board socket:PATH: the path is empty".into())); }
            return Some(Ok(Endpoint::Unix(path.into())));
        }
        let addr = spec.strip_prefix("tcp:")?;
        Some(match addr.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() && port.parse::<u16>().is_ok_and(|p| p != 0) => Ok(Endpoint::Tcp(addr.into())),
            _ => Err(format!("--board tcp:HOST:PORT: expected a host and a port, got {addr:?}")),
        })
    }
    fn connect(&self, timeout: Duration) -> Result<Streams, String> {
        match self {
            #[cfg(unix)]
            Endpoint::Unix(path) => {
                let s = std::os::unix::net::UnixStream::connect(path).map_err(|e| format!("cannot connect: {e}"))?;
                s.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
                Ok((Box::new(s.try_clone().map_err(|e| e.to_string())?), Box::new(s)))
            }
            #[cfg(not(unix))]
            Endpoint::Unix(_) => Err("Unix sockets need a Unix host; use tcp:HOST:PORT".into()),
            Endpoint::Tcp(addr) => {
                let s = std::net::TcpStream::connect(addr.as_str()).map_err(|e| format!("cannot connect: {e}"))?;
                s.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
                s.set_nodelay(true).map_err(|e| e.to_string())?;
                Ok((Box::new(s.try_clone().map_err(|e| e.to_string())?), Box::new(s)))
            }
        }
    }
}
impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Endpoint::Unix(p) => write!(f, "socket:{}", p.display()), Endpoint::Tcp(a) => write!(f, "tcp:{a}") }
    }
}

/// Exchange counts for the end-of-run report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats { pub syncs: u64, pub tx_bytes: u64, pub rx_bytes: u64, pub resets: u64 }

/// Consecutive transmitted bytes on one route, not yet sent.
struct TxRun { route: UartRoute, t: u64, t_end: u64, data: Vec<u8> }

pub struct SocketBoard {
    /// The endpoint as given, which every error names.
    peer: String,
    /// What the peer's `hello` called itself.
    name: String,
    input: BufReader<Box<dyn Read>>,
    output: BufWriter<Box<dyn Write>>,
    hz: u64,
    now: VirtualCycle,
    /// No unprompted output before this (ns); `None`: none ever.
    quiet_until: Option<u64>,
    /// No answer sooner than this after an event (ns); `None`: it never answers.
    latency: Option<u64>,
    /// The first event since the last `sync` (ns).
    unanswered: Option<u64>,
    /// UART ports the peer listens to (bit n = port n).
    uarts: u32,
    tx: Option<TxRun>,
    /// Paced receive bytes by (due cycle, sequence): (pin, baud, byte).
    rx: BTreeMap<(VirtualCycle, u64), (u8, u32, u8)>,
    rx_seq: u64,
    /// Per pin, the cycle its line is free again after the bytes already scheduled.
    line_free: [VirtualCycle; 64],
    stats: Stats,
    failed: Option<String>,
    /// The CLI ends the run on a peer failure; a test inspects `report()` instead.
    exit_on_error: bool,
}

/// The first cycle at or after `ns`, and the time of a cycle, at `hz` bus cycles per second.
fn cycle_of_ns(ns: u64, hz: u64) -> VirtualCycle { (ns as u128 * hz as u128).div_ceil(1_000_000_000).min(u64::MAX as u128) as u64 }
fn ns_of_cycle(c: VirtualCycle, hz: u64) -> u64 { (c as u128 * 1_000_000_000 / hz as u128).min(u64::MAX as u128) as u64 }

/// A time field: absent or null is `None`; anything but a nonnegative integer is an error.
fn time_field(msg: &Json, key: &str) -> Result<Option<u64>, String> {
    match msg.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Int(t)) if *t >= 0 => Ok(Some(*t as u64)),
        Some(v) => Err(format!("{key}: expected nanoseconds or null, got {v:?}")),
    }
}
fn int_field(msg: &Json, key: &str, range: std::ops::RangeInclusive<i64>) -> Result<i64, String> {
    match msg.get(key) {
        Some(Json::Int(v)) if range.contains(v) => Ok(*v),
        v => Err(format!("{key}: expected an integer in {}..={}, got {v:?}", range.start(), range.end())),
    }
}

impl SocketBoard {
    /// Connect to the peer and exchange `hello`. `hz` is the bus cycle rate of `chip`.
    pub fn connect(endpoint: &Endpoint, chip: &str, hz: u64) -> Result<Self, String> {
        let (input, output) = endpoint.connect(DEFAULT_TIMEOUT).map_err(|e| format!("[socket] peer {endpoint}: {e}"))?;
        Self::over(endpoint.to_string(), input, output, chip, hz)
    }
    /// End the process when the peer fails, as the CLI does, instead of going inert.
    pub fn exit_on_failure(mut self) -> Self { self.exit_on_error = true; self }

    /// The board over an established connection (tests use a socket pair). Exchanges `hello`.
    pub fn over(peer: String, input: Box<dyn Read>, output: Box<dyn Write>, chip: &str, hz: u64) -> Result<Self, String> {
        let mut b = SocketBoard {
            peer, name: String::new(), input: BufReader::new(input), output: BufWriter::new(output), hz: hz.max(1), now: 0,
            quiet_until: None, latency: None, unanswered: None, uarts: u32::MAX, tx: None, rx: BTreeMap::new(), rx_seq: 0,
            line_free: [0; 64], stats: Stats::default(), failed: None, exit_on_error: false,
        };
        b.hello(chip).map_err(|e| format!("[socket] peer {}: {e}", b.peer))?;
        Ok(b)
    }

    pub fn stats(&self) -> Stats { self.stats }
    /// Why the peer was given up on, if it was.
    pub fn error(&self) -> Option<&str> { self.failed.as_deref() }

    fn hello(&mut self, chip: &str) -> Result<(), String> {
        self.send(&format!("{{\"type\":\"hello\",\"version\":{VERSION},\"chip\":\"{}\"}}", json_escape(chip)))?;
        self.output.flush().map_err(|e| format!("write: {e}"))?;
        let msg = self.read_msg("hello")?;
        if msg.str_or("type", "") != "hello" { return Err(format!("expected hello, got {:?}", msg.str_or("type", ""))); }
        let version = msg.i64_or("version", 0);
        if version != VERSION { return Err(format!("speaks protocol version {version}, this emulator {VERSION}")); }
        self.name = msg.str_or("name", "").to_string();
        self.quiet_until = time_field(&msg, "quiet_until")?;
        self.latency = time_field(&msg, "latency")?;
        if let Some(ports) = msg.get("uarts") {
            let ports = ports.as_arr().ok_or("uarts: expected an array of port numbers")?;
            self.uarts = 0;
            for p in ports { match p { Json::Int(n @ 0..=31) => self.uarts |= 1 << n, v => return Err(format!("uarts: bad port {v:?}")) } }
        }
        Ok(())
    }

    fn send(&mut self, line: &str) -> Result<(), String> {
        self.output.write_all(line.as_bytes()).and_then(|_| self.output.write_all(b"\n")).map_err(|e| format!("write: {e}"))
    }

    fn read_msg(&mut self, waiting_for: &str) -> Result<Json, String> {
        let mut line = String::new();
        loop {
            line.clear();
            match self.input.read_line(&mut line) {
                Ok(0) => return Err(format!("closed the connection while the emulator waited for {waiting_for}")),
                Ok(_) => {}
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                    return Err(format!("no answer within the read timeout while the emulator waited for {waiting_for}"));
                }
                Err(e) => return Err(format!("read: {e}")),
            }
            let text = line.trim_end();
            if text.is_empty() { continue; }
            return parse_json(text).map_err(|e| format!("bad message ({e}): {text:.120}"));
        }
    }

    fn ns(&self, cycle: VirtualCycle) -> u64 { ns_of_cycle(cycle, self.hz) }

    /// The first cycle at which the board must hear from the peer.
    fn horizon(&self) -> Option<VirtualCycle> {
        let answer = self.unanswered.zip(self.latency).map(|(t, l)| t.saturating_add(l));
        let ns = match (self.quiet_until, answer) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) }?;
        Some(cycle_of_ns(ns, self.hz))
    }

    fn flush_tx(&mut self) -> Result<(), String> {
        let Some(run) = self.tx.take() else { return Ok(()) };
        let r = run.route;
        let pins: Vec<String> = (0..64).filter(|&p| r.tx_pins & (1u64 << p) != 0).map(|p: u32| p.to_string()).collect();
        let opt = |v: Option<u32>| v.map_or("null".to_string(), |v| v.to_string());
        self.send(&format!("{{\"type\":\"uart_tx\",\"t\":{},\"t_end\":{},\"port\":{},\"tx\":[{}],\"rx\":{},\"baud\":{},\"data\":\"{}\"}}",
            run.t, run.t_end, r.port, pins.join(","), opt(r.rx_pin.map(u32::from)), opt(r.baud), hex_encode(&run.data)))
    }

    /// An event the peer may answer: it must hear back within `latency`.
    fn note_event(&mut self, t: u64) { if self.unanswered.is_none() { self.unanswered = Some(t); } }

    fn sync(&mut self, cycle: VirtualCycle) -> Result<(), String> {
        let t = self.ns(cycle);
        self.flush_tx()?;
        self.send(&format!("{{\"type\":\"sync\",\"t\":{t}}}"))?;
        self.output.flush().map_err(|e| format!("write: {e}"))?;
        self.stats.syncs += 1;
        self.unanswered = None;
        loop {
            let msg = self.read_msg(&format!("done after sync t={t}"))?;
            match msg.str_or("type", "") {
                "uart_rx" => self.schedule_rx(&msg, cycle)?,
                "log" => eprintln!("[socket] {}: {}", if self.name.is_empty() { &self.peer } else { &self.name }, msg.str_or("line", "")),
                "done" => {
                    self.quiet_until = time_field(&msg, "quiet_until")?;
                    if msg.get("latency").is_some() { self.latency = time_field(&msg, "latency")?; }
                    return Ok(());
                }
                other => return Err(format!("unexpected message {other:?} after sync t={t}")),
            }
        }
    }

    /// `uart_rx {t, pin, baud, data, bits?}`: byte i completes `(i + 1) * bits` bit times after
    /// its line is free, which is `t` or the end of what that pin already carries, and never
    /// before the sync that delivered it.
    fn schedule_rx(&mut self, msg: &Json, now: VirtualCycle) -> Result<(), String> {
        let t = time_field(msg, "t")?.ok_or("uart_rx without t")?;
        let pin = int_field(msg, "pin", 0..=63)? as u8;
        let baud = int_field(msg, "baud", 1..=20_000_000)? as u32;
        let bits = match msg.get("bits") { None => 10, Some(_) => int_field(msg, "bits", 1..=32)? as u64 };
        let data = msg.get("data").and_then(|d| d.as_str()).and_then(hex_decode).ok_or("uart_rx: data must be hex")?;
        let start = cycle_of_ns(t, self.hz).max(self.line_free[pin as usize]).max(now);
        let char_cycles = |n: u64| (n as u128 * bits as u128 * self.hz as u128).div_ceil(baud as u128).min(u64::MAX as u128) as u64;
        for (i, &byte) in data.iter().enumerate() {
            let due = start.saturating_add(char_cycles(i as u64 + 1));
            self.rx.insert((due, self.rx_seq), (pin, baud, byte));
            self.rx_seq += 1;
            self.line_free[pin as usize] = due;
        }
        self.stats.rx_bytes += data.len() as u64;
        Ok(())
    }

    /// Give up on the peer: the CLI ends the run, a test reads `report()`.
    fn fail(&mut self, e: String) {
        let text = format!("[socket] peer {}: {e}", self.peer);
        if self.exit_on_error {
            eprintln!("{text}");
            eprintln!("[emu] stop: the board's peer failed at t={} ns", self.ns(self.now));
            std::process::exit(1);
        }
        self.failed = Some(text);
        self.quiet_until = None; self.latency = None; self.unanswered = None; self.tx = None; self.rx.clear();
    }
    fn check(&mut self, r: Result<(), String>) { if let Err(e) = r { self.fail(e); } }
}

impl BoardModel for SocketBoard {
    fn name(&self) -> &'static str { "socket" }
    fn uses_uart_pins(&self) -> bool { true }
    fn uses_gpio_edges(&self) -> bool { false }
    fn uart_tx(&mut self, cycle: VirtualCycle, route: UartRoute, byte: u8) {
        if self.failed.is_some() || route.port >= 32 || self.uarts & (1 << route.port) == 0 { return; }
        let t = self.ns(cycle);
        self.stats.tx_bytes += 1;
        if let Some(run) = &mut self.tx {
            if run.route == route && run.data.len() < TX_RUN_MAX { run.data.push(byte); run.t_end = t; self.note_event(t); return; }
        }
        let r = self.flush_tx();
        self.check(r);
        self.tx = Some(TxRun { route, t, t_end: t, data: vec![byte] });
        self.note_event(t);
    }
    fn uart_rx(&mut self, cycle: VirtualCycle) -> Vec<UartInput> {
        let mut out: Vec<UartInput> = Vec::new();
        while let Some((&(due, seq), &(pin, baud, byte))) = self.rx.first_key_value() {
            if due > cycle { break; }
            self.rx.remove(&(due, seq));
            match out.last_mut() {
                Some(last) if last.pin == pin && last.baud == baud => last.data.push(byte),
                _ => out.push(UartInput::new(pin, baud, vec![byte])),
            }
        }
        out
    }
    fn next_deadline(&self) -> Option<VirtualCycle> {
        let rx = self.rx.first_key_value().map(|(&(due, _), _)| due);
        match (rx, self.horizon()) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) }
    }
    fn advance_to(&mut self, cycle: VirtualCycle) {
        self.now = self.now.max(cycle);
        if self.horizon().is_some_and(|h| self.now >= h) { let r = self.sync(self.now); self.check(r); }
    }
    fn guest_reset(&mut self, cycle: VirtualCycle, cause: u32) {
        if self.failed.is_some() { return; }
        self.now = self.now.max(cycle);
        let t = self.ns(self.now);
        self.stats.resets += 1;
        let r = self.flush_tx().and_then(|_| self.send(&format!("{{\"type\":\"reset\",\"t\":{t},\"cause\":{cause}}}")));
        self.check(r);
        self.note_event(t);
    }
    fn report(&self) -> String {
        let who = if self.name.is_empty() { self.peer.clone() } else { format!("{} ({})", self.peer, self.name) };
        let s = self.stats;
        let mut r = format!("[socket] peer {who}: {} syncs, {} bytes to it, {} from it, {} guest resets", s.syncs, s.tx_bytes, s.rx_bytes, s.resets);
        if let Some(e) = &self.failed { r.push('\n'); r.push_str(e); }
        r
    }
}

/// Tell the peer the run is over (best effort: it may be gone already).
impl Drop for SocketBoard {
    fn drop(&mut self) {
        if self.failed.is_some() { return; }
        let t = self.ns(self.now);
        let _ = self.flush_tx().and_then(|_| self.send(&format!("{{\"type\":\"stop\",\"t\":{t}}}")));
        let _ = self.output.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_parse() {
        assert_eq!(Endpoint::parse("cyd"), None);
        assert_eq!(Endpoint::parse("socket:/tmp/b.sock"), Some(Ok(Endpoint::Unix("/tmp/b.sock".into()))));
        assert_eq!(Endpoint::parse("tcp:127.0.0.1:7000"), Some(Ok(Endpoint::Tcp("127.0.0.1:7000".into()))));
        assert_eq!(Endpoint::parse("tcp:[::1]:7000"), Some(Ok(Endpoint::Tcp("[::1]:7000".into()))));
        for bad in ["socket:", "tcp:", "tcp:host", "tcp::7000", "tcp:host:0", "tcp:host:70000"] { assert!(matches!(Endpoint::parse(bad), Some(Err(_))), "{bad}"); }
        assert_eq!(Endpoint::Unix("/tmp/b.sock".into()).to_string(), "socket:/tmp/b.sock");
    }

    #[test]
    fn ns_cycle_mapping_rounds_toward_the_later_cycle() {
        let hz = 240_000_000;
        assert_eq!(cycle_of_ns(1_000_000_000, hz), hz);
        assert_eq!(cycle_of_ns(1, hz), 1);
        assert_eq!(ns_of_cycle(1, hz), 4);
        for ns in [0u64, 1, 5, 1_000_003, 1 << 50] { assert!(ns_of_cycle(cycle_of_ns(ns, hz), hz) >= ns); }
        assert_eq!(ns_of_cycle(u64::MAX, 1), u64::MAX);
    }
}
