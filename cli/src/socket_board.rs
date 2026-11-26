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
use std::io::{BufWriter, ErrorKind, Read, Write};
use std::time::{Duration, Instant};

pub const VERSION: i64 = 1;
/// Wall time the peer has to answer a message (from the moment it was sent to its `hello` or
/// `done`), and to take what the board writes.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Bytes of one `uart_tx` message at most; consecutive bytes on one route share a message.
const TX_RUN_MAX: usize = 256;
/// The longest line accepted from the peer, by default.
pub const LINE_MAX: usize = 16 << 20;
/// Received bytes not yet on the line, at most, by default: 64 MiB is 97 minutes at 115200 baud.
pub const RX_QUEUE_MAX: usize = 64 << 20;
/// A `quiet_until` at or before its sync means "at any moment"; the board then comes back this
/// much later, so such a peer costs at most one exchange per 10 µs of emulated time.
pub const MIN_QUIET_NS: u64 = 10_000;

/// A connection to the peer: a stream the board can put wall-time limits on.
pub trait Conn: Read + Write {
    fn set_timeouts(&self, read: Option<Duration>, write: Option<Duration>) -> std::io::Result<()>;
    fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>>;
}
macro_rules! conn {
    ($t:ty) => {
        impl Conn for $t {
            fn set_timeouts(&self, read: Option<Duration>, write: Option<Duration>) -> std::io::Result<()> { self.set_read_timeout(read)?; self.set_write_timeout(write) }
            fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>> { Ok(Box::new(self.try_clone()?)) }
        }
    };
}
conn!(std::net::TcpStream);
#[cfg(unix)]
conn!(std::os::unix::net::UnixStream);

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
    fn connect(&self, timeout: Duration) -> Result<Box<dyn Conn>, String> {
        match self {
            #[cfg(unix)]
            Endpoint::Unix(path) => Ok(Box::new(std::os::unix::net::UnixStream::connect(path).map_err(|e| format!("cannot connect: {e}"))?)),
            #[cfg(not(unix))]
            Endpoint::Unix(_) => Err("Unix sockets need a Unix host; use tcp:HOST:PORT".into()),
            Endpoint::Tcp(addr) => {
                use std::net::ToSocketAddrs;
                let mut last = format!("{addr} resolves to no address");
                for a in addr.to_socket_addrs().map_err(|e| format!("cannot resolve: {e}"))? {
                    match std::net::TcpStream::connect_timeout(&a, timeout) {
                        Ok(s) => { s.set_nodelay(true).map_err(|e| e.to_string())?; return Ok(Box::new(s)); }
                        Err(e) => last = format!("{a}: {e}"),
                    }
                }
                Err(format!("cannot connect: {last}"))
            }
        }
    }
}
impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { Endpoint::Unix(p) => write!(f, "socket:{}", p.display()), Endpoint::Tcp(a) => write!(f, "tcp:{a}") }
    }
}

/// Exchange counts for the end-of-run report. `late_rx`: `uart_rx` messages stamped before the
/// peer's own promises allowed, delivered at the sync that brought them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats { pub syncs: u64, pub tx_bytes: u64, pub rx_bytes: u64, pub resets: u64, pub late_rx: u64 }

/// Transmitted bytes on one route written closer together than half a character time (a FIFO
/// fill), not yet sent.
struct TxRun { route: UartRoute, t: u64, t_end: u64, data: Vec<u8> }

/// One `uart_rx` message on the line: byte i completes at `start + (i + 1) * bits * hz / baud`.
struct RxRun { pin: u8, baud: u32, bits: u64, start: VirtualCycle, data: Vec<u8>, next: usize }
impl RxRun {
    fn due(&self, i: usize, hz: u64) -> VirtualCycle {
        let c = ((i as u128 + 1) * self.bits as u128 * hz as u128).div_ceil(self.baud as u128);
        self.start.saturating_add(c.min(u64::MAX as u128) as u64)
    }
}

pub struct SocketBoard {
    /// The endpoint as given, which every error names.
    peer: String,
    /// What the peer's `hello` called itself.
    name: String,
    input: Box<dyn Conn>,
    /// Received bytes not yet taken as lines, and how far they hold no newline.
    pending: Vec<u8>,
    scanned: usize,
    output: BufWriter<Box<dyn Conn>>,
    timeout: Duration,
    /// Warned once that the peer asks for a sync at every device tick.
    warned_busy: bool,
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
    /// Messages on the line, in arrival order, and their bytes not yet delivered.
    rx: Vec<RxRun>,
    rx_queued: usize,
    line_max: usize,
    rx_max: usize,
    /// The cycle of the last sync: the next one comes later.
    last_sync: Option<VirtualCycle>,
    warned_late: bool,
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
/// A promise the peer must always state: a missing one is an error, `null` means never.
fn required_time(msg: &Json, key: &str) -> Result<Option<u64>, String> {
    match msg.get(key) {
        None => Err(format!("{} without {key} (null: never)", msg.str_or("type", "message"))),
        Some(Json::Null) => Ok(None),
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
        let conn = endpoint.connect(DEFAULT_TIMEOUT).map_err(|e| format!("[socket] peer {endpoint}: {e}"))?;
        Self::over(endpoint.to_string(), conn, chip, hz, DEFAULT_TIMEOUT)
    }
    /// End the process when the peer fails, as the CLI does, instead of going inert.
    pub fn exit_on_failure(mut self) -> Self { self.exit_on_error = true; self }

    /// The board over an established connection (tests use a socket pair), with `timeout` for
    /// each answer and each write. Exchanges `hello`.
    pub fn over(peer: String, conn: Box<dyn Conn>, chip: &str, hz: u64, timeout: Duration) -> Result<Self, String> {
        let timeout = timeout.max(Duration::from_millis(1));
        let setup = |e: std::io::Error| format!("[socket] peer {peer}: {e}");
        conn.set_timeouts(Some(timeout), Some(timeout)).map_err(setup)?;
        let output = BufWriter::new(conn.try_clone_conn().map_err(setup)?);
        let mut b = SocketBoard {
            peer, name: String::new(), input: conn, pending: Vec::new(), scanned: 0, output, timeout, warned_busy: false, hz: hz.max(1), now: 0,
            quiet_until: None, latency: None, unanswered: None, uarts: u32::MAX, tx: None, rx: Vec::new(), rx_queued: 0, line_max: LINE_MAX, rx_max: RX_QUEUE_MAX, last_sync: None, warned_late: false,
            line_free: [0; 64], stats: Stats::default(), failed: None, exit_on_error: false,
        };
        b.hello(chip).map_err(|e| format!("[socket] peer {}: {e}", b.peer))?;
        Ok(b)
    }

    pub fn stats(&self) -> Stats { self.stats }
    /// The longest line accepted and the most received bytes held before they are on the line
    /// (`LINE_MAX`, `RX_QUEUE_MAX` by default); beyond either the peer is given up on.
    pub fn set_limits(&mut self, line_max: usize, rx_queue_max: usize) { self.line_max = line_max; self.rx_max = rx_queue_max; }
    /// Memory the receive queue holds, in bytes of payload capacity.
    pub fn rx_queue_memory(&self) -> usize { self.rx.iter().map(|r| r.data.capacity()).sum() }
    /// Why the peer was given up on, if it was.
    pub fn error(&self) -> Option<&str> { self.failed.as_deref() }

    fn hello(&mut self, chip: &str) -> Result<(), String> {
        self.send(&format!("{{\"type\":\"hello\",\"version\":{VERSION},\"chip\":\"{}\"}}", json_escape(chip)))?;
        self.flush()?;
        let deadline = Instant::now() + self.timeout;
        let msg = self.read_msg("hello", deadline)?;
        if msg.str_or("type", "") != "hello" { return Err(format!("expected hello, got {:?}", msg.str_or("type", ""))); }
        let version = msg.i64_or("version", 0);
        if version != VERSION { return Err(format!("speaks protocol version {version}, this emulator {VERSION}")); }
        self.name = msg.str_or("name", "").to_string();
        self.quiet_until = required_time(&msg, "quiet_until")?;
        self.latency = required_time(&msg, "latency")?;
        if let Some(ports) = msg.get("uarts") {
            let ports = ports.as_arr().ok_or("uarts: expected an array of port numbers")?;
            self.uarts = 0;
            for p in ports { match p { Json::Int(n @ 0..=31) => self.uarts |= 1 << n, v => return Err(format!("uarts: bad port {v:?}")) } }
        }
        Ok(())
    }

    fn write_error(&self, e: std::io::Error) -> String {
        match e.kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => format!("took nothing the emulator wrote for {:?}", self.timeout),
            ErrorKind::BrokenPipe | ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => format!("closed the connection while the emulator wrote to it ({e})"),
            _ => format!("write: {e}"),
        }
    }
    fn send(&mut self, line: &str) -> Result<(), String> {
        self.output.write_all(line.as_bytes()).and_then(|_| self.output.write_all(b"\n")).map_err(|e| self.write_error(e))
    }
    fn flush(&mut self) -> Result<(), String> { self.output.flush().map_err(|e| self.write_error(e)) }

    /// The next message, which must be complete by `deadline` (wall time).
    fn read_msg(&mut self, waiting_for: &str, deadline: Instant) -> Result<Json, String> {
        let mut chunk = [0u8; 4096];
        loop {
            while let Some(nl) = self.pending[self.scanned..].iter().position(|&b| b == b'\n').map(|i| self.scanned + i) {
                if nl > self.line_max { return Err(format!("sent a line longer than {} bytes", self.line_max)); }
                // The line keeps the buffer it arrived in; only what follows it is copied.
                let rest = self.pending.split_off(nl + 1);
                let line = std::mem::replace(&mut self.pending, rest);
                self.scanned = 0;
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end();
                if text.is_empty() { continue; }
                return parse_json(text).map_err(|e| format!("bad message ({e}): {text:.120}"));
            }
            self.scanned = self.pending.len();
            if self.pending.len() > self.line_max { return Err(format!("sent a line longer than {} bytes", self.line_max)); }
            let timeout = || format!("no answer within {:?} while the emulator waited for {waiting_for}", self.timeout);
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() { return Err(timeout()); }
            self.input.set_timeouts(Some(left), Some(self.timeout)).map_err(|e| format!("read: {e}"))?;
            match self.input.read(&mut chunk) {
                Ok(0) => return Err(format!("closed the connection while the emulator waited for {waiting_for}")),
                Ok(n) => self.pending.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => return Err(timeout()),
                Err(e) => return Err(format!("read: {e}")),
            }
        }
    }

    fn ns(&self, cycle: VirtualCycle) -> u64 { ns_of_cycle(cycle, self.hz) }

    /// The earliest time (ns) the peer's promises let it speak: the next sync's due time.
    fn horizon_ns(&self) -> Option<u64> {
        let answer = self.unanswered.zip(self.latency).map(|(t, l)| t.saturating_add(l));
        match (self.quiet_until, answer) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) }
    }
    /// The first cycle at which the board must hear from the peer, never the last sync's.
    fn horizon(&self) -> Option<VirtualCycle> {
        let c = cycle_of_ns(self.horizon_ns()?, self.hz);
        Some(self.last_sync.map_or(c, |l| c.max(l.saturating_add(1))))
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
        self.flush()?;
        let deadline = Instant::now() + self.timeout;
        self.stats.syncs += 1;
        // Nothing the peer says can be stamped before this without breaking a promise.
        let promised = self.horizon_ns().unwrap_or(t).min(t);
        self.last_sync = Some(cycle);
        // The peer answers everything it has been sent in this exchange (with later `t` where it
        // must), or names the time it will speak next in `quiet_until`.
        self.unanswered = None;
        loop {
            let msg = self.read_msg(&format!("done after sync t={t}"), deadline)?;
            match msg.str_or("type", "") {
                "uart_rx" => self.schedule_rx(&msg, cycle, promised)?,
                "log" => eprintln!("[socket] {}: {}", if self.name.is_empty() { &self.peer } else { &self.name }, msg.str_or("line", "")),
                "done" => {
                    self.quiet_until = required_time(&msg, "quiet_until")?;
                    if msg.get("latency").is_some() { self.latency = time_field(&msg, "latency")?; }
                    if let Some(q) = self.quiet_until.filter(|&q| q <= t) {
                        if !self.warned_busy {
                            self.warned_busy = true;
                            eprintln!("[socket] peer {}: quiet_until {q} is not after the sync at t={t}: taken as t + {MIN_QUIET_NS} ns, an exchange every {} µs while it lasts", self.peer, MIN_QUIET_NS / 1000);
                        }
                        self.quiet_until = Some(t.saturating_add(MIN_QUIET_NS));
                    }
                    return Ok(());
                }
                other => return Err(format!("unexpected message {other:?} after sync t={t}")),
            }
        }
    }

    /// `uart_rx {t, pin, baud, data, bits?}`: byte i completes `(i + 1) * bits` bit times after
    /// its line is free, which is `t` or the end of what that pin already carries, and never
    /// before the sync that delivered it. A `t` before `promised` broke a promise: counted, and
    /// warned about once.
    fn schedule_rx(&mut self, msg: &Json, now: VirtualCycle, promised: u64) -> Result<(), String> {
        let t = time_field(msg, "t")?.ok_or("uart_rx without t")?;
        let pin = int_field(msg, "pin", 0..=63)? as u8;
        let baud = int_field(msg, "baud", 1..=20_000_000)? as u32;
        let bits = match msg.get("bits") { None => 10, Some(_) => int_field(msg, "bits", 1..=32)? as u64 };
        let mut data = msg.get("data").and_then(|d| d.as_str()).and_then(hex_decode).ok_or("uart_rx: data must be hex")?;
        if t < promised {
            self.stats.late_rx += 1;
            if !self.warned_late {
                self.warned_late = true;
                eprintln!("[socket] peer {}: uart_rx at t={t} is earlier than its quiet_until/latency allowed (t={promised}); delivered from the sync at t={}", self.peer, self.ns(now));
            }
        }
        if data.is_empty() { return Ok(()); }
        if self.rx_queued.saturating_add(data.len()) > self.rx_max {
            return Err(format!("would hold {} received bytes not yet on the line; the limit is {}", self.rx_queued + data.len(), self.rx_max));
        }
        data.shrink_to_fit();
        let start = cycle_of_ns(t, self.hz).max(self.line_free[pin as usize]).max(now);
        let run = RxRun { pin, baud, bits, start, data, next: 0 };
        self.line_free[pin as usize] = run.due(run.data.len() - 1, self.hz);
        self.stats.rx_bytes += run.data.len() as u64;
        self.rx_queued += run.data.len();
        self.rx.push(run);
        Ok(())
    }
    /// The run whose next byte is due first (the earlier message on a tie), and when.
    fn next_rx(&self) -> Option<(usize, VirtualCycle)> {
        self.rx.iter().enumerate().map(|(i, r)| (i, r.due(r.next, self.hz))).min_by_key(|&(i, due)| (due, i))
    }

    /// Give up on the peer: the CLI ends the run, a test reads `report()`.
    fn fail(&mut self, e: String) {
        self.failed = Some(format!("[socket] peer {}: {e}", self.peer));
        if self.exit_on_error {
            // A board cannot stop the run, so the process ends here: no end-of-run report, and
            // output files (--vcd, --pcap, --wav, profiles) may be incomplete.
            eprintln!("{}", self.report());
            eprintln!("[emu] stop: the board's peer failed at t={} ns; output files may be incomplete", self.ns(self.now));
            std::process::exit(1);
        }
        self.quiet_until = None; self.latency = None; self.unanswered = None; self.tx = None; self.rx.clear(); self.rx_queued = 0;
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
            // Bytes written faster than the line drains them share a message; a byte written half
            // a character time or more after the last one starts a new one (no baud: a new time).
            let gap = t.saturating_sub(run.t_end) as u128;
            let together = route.baud.map_or(gap == 0, |b| gap * b as u128 * 2 < 10 * 1_000_000_000);
            if run.route == route && run.data.len() < TX_RUN_MAX && together { run.data.push(byte); run.t_end = t; self.note_event(t); return; }
        }
        let r = self.flush_tx();
        self.check(r);
        self.tx = Some(TxRun { route, t, t_end: t, data: vec![byte] });
        self.note_event(t);
    }
    fn uart_rx(&mut self, cycle: VirtualCycle) -> Vec<UartInput> {
        let mut out: Vec<UartInput> = Vec::new();
        while let Some((i, due)) = self.next_rx() {
            if due > cycle { break; }
            let run = &mut self.rx[i];
            let (pin, baud, byte) = (run.pin, run.baud, run.data[run.next]);
            run.next += 1;
            if run.next == run.data.len() { self.rx.remove(i); }
            self.rx_queued -= 1;
            match out.last_mut() {
                Some(last) if last.pin == pin && last.baud == baud => last.data.push(byte),
                _ => out.push(UartInput::new(pin, baud, vec![byte])),
            }
        }
        out
    }
    fn next_deadline(&self) -> Option<VirtualCycle> {
        let rx = self.next_rx().map(|(_, due)| due);
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
        let mut r = format!("[socket] peer {who}: {} syncs, {} bytes to it, {} from it, {} guest reset{}", s.syncs, s.tx_bytes, s.rx_bytes, s.resets, if s.resets == 1 { "" } else { "s" });
        if s.late_rx > 0 { r.push_str(&format!(", {} uart_rx stamped before the peer's promises allowed", s.late_rx)); }
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
        let _ = self.flush();
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
