//! A NAT transport for a host without sockets (the browser build). The guest's socket operations
//! queue up as records the host takes after every run (`QueueHandle::take_ops`); the host answers
//! with events (`QueueHandle::event`) before the next run, from whatever network it has (a
//! WebSocket relay, say). No threads, no sockets: the transport and its handle share one
//! `HostQueue`.
//!
//! Records (little-endian, each padded to a multiple of 4 bytes):
//! `[u8 op][u8 flags][u16 port][u32 id][u32 len][len bytes]` — see `op` for the kinds.

use std::collections::{HashMap, VecDeque};
use std::io::{self, ErrorKind};
use std::net::SocketAddrV4;
use std::sync::{Arc, Mutex, MutexGuard};

use super::transport::{Connect, FlowId, NatTransport, Resolve};
use crate::net::packet::{dns_question, dns_reply};

/// TCP flows open at once; `tcp_connect` answers None beyond it (the guest's SYN is retried).
pub const MAX_TCP_FLOWS: usize = 16;
/// Bytes from the host waiting for the guest, per TCP flow; `TCP_DATA` takes only what fits.
pub const TCP_RX_MAX: usize = 64 * 1024;
/// Data bytes in records the host has not taken yet; `tcp_write` blocks beyond it.
pub const OUT_MAX: usize = 256 * 1024;
/// Datagrams waiting for the guest, per UDP flow; more are dropped.
pub const UDP_RX_MAX: usize = 16;
/// Name lookups waiting for the host; a new one forgets the oldest.
const PENDING_MAX: usize = 64;
/// Size of a record's header.
pub const RECORD_HEADER: usize = 12;
/// Where the guest's DNS goes: the emulated subnet's resolver.
const RESOLVER: [u8; 4] = [10, 0, 2, 3];

/// Record kinds (`op`) of `QueueHandle::take_ops`.
pub mod op {
    /// Open TCP: `port` = destination port, data = IPv4[4].
    pub const TCP_CONNECT: u8 = 1;
    /// Bytes the guest sent on a TCP flow.
    pub const TCP_DATA: u8 = 2;
    /// The guest finished sending (FIN).
    pub const TCP_SHUTDOWN: u8 = 3;
    /// The guest's side forgot the flow; flags bit 0: abort (reset) instead of close.
    pub const TCP_CLOSE: u8 = 4;
    /// Open a UDP flow: `port` = destination port, data = IPv4[4].
    pub const UDP_OPEN: u8 = 5;
    /// A datagram the guest sent on a UDP flow.
    pub const UDP_DATA: u8 = 6;
    pub const UDP_CLOSE: u8 = 7;
    /// Look a name up: `id` = query id, `port` = qtype (1 A, 28 AAAA), data = the name in UTF-8.
    pub const RESOLVE: u8 = 8;
}

/// Event kinds of the browser build's `esp32sim_nat_event`, for `HostEvent::from_abi`.
pub mod ev {
    pub const CONNECTED: u32 = 1;
    pub const FAILED: u32 = 2;
    pub const TCP_DATA: u32 = 3;
    pub const TCP_EOF: u32 = 4;
    pub const TCP_RESET: u32 = 5;
    pub const TX_CREDIT: u32 = 6;
    pub const UDP_DATA: u32 = 7;
    pub const RESOLVED: u32 = 8;
    pub const UNRESOLVED: u32 = 9;
}

/// Why the host could not open a TCP flow (`HostEvent::Failed`).
pub mod fail {
    pub const REFUSED: u32 = 1;
    pub const UNREACHABLE: u32 = 2;
    pub const POLICY: u32 = 3;
    pub const OFFLINE: u32 = 4;
}

/// What the host tells the guest's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEvent<'a> {
    /// The TCP flow is open; the host takes `window` bytes before the first `TxCredit`.
    Connected { id: FlowId, window: u32 },
    /// The TCP flow could not be opened (`fail::*`); one that was open is reset instead.
    Failed { id: FlowId, reason: u32 },
    TcpData { id: FlowId, data: &'a [u8] },
    /// The remote side finished sending.
    TcpEof { id: FlowId },
    TcpReset { id: FlowId },
    /// The host took `bytes` more of the guest's data.
    TxCredit { id: FlowId, bytes: u32 },
    UdpData { id: FlowId, data: &'a [u8] },
    /// The lookup `qid` found `addrs` (n × IPv4[4]; may be none) for `ttl` seconds.
    Resolved { qid: u32, ttl: u32, addrs: &'a [u8] },
    /// The lookup `qid` failed: DNS rcode 2 SERVFAIL or 3 NXDOMAIN.
    Unresolved { qid: u32, rcode: u32 },
}

impl<'a> HostEvent<'a> {
    /// The event of the ABI's `(kind, id, arg, data)`; None for an unknown kind or short data.
    pub fn from_abi(kind: u32, id: u32, arg: u32, data: &'a [u8]) -> Option<Self> {
        Some(match kind {
            ev::CONNECTED => HostEvent::Connected { id, window: arg },
            ev::FAILED => HostEvent::Failed { id, reason: arg },
            ev::TCP_DATA => HostEvent::TcpData { id, data },
            ev::TCP_EOF => HostEvent::TcpEof { id },
            ev::TCP_RESET => HostEvent::TcpReset { id },
            ev::TX_CREDIT => HostEvent::TxCredit { id, bytes: arg },
            ev::UDP_DATA => HostEvent::UdpData { id, data },
            ev::RESOLVED => HostEvent::Resolved { qid: id, ttl: arg, addrs: data },
            ev::UNRESOLVED => HostEvent::Unresolved { qid: id, rcode: arg },
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TcpState { Connecting, Open, Failed(ErrorKind), Reset }

struct TcpFlow {
    state: TcpState,
    /// bytes from the host the guest has not read yet
    rx: VecDeque<u8>,
    /// bytes the host still takes
    credit: u64,
    eof: bool,
}

struct UdpFlow {
    rx: VecDeque<Vec<u8>>,
    /// a DNS flow answered here (a query the host does not resolve: not A or AAAA)
    local_dns: bool,
}

/// What went through the queue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStats {
    /// data bytes in `TCP_DATA` / `UDP_DATA` records
    pub bytes_to_host: u64,
    /// bytes of `TcpData` / `UdpData` events taken
    pub bytes_to_guest: u64,
    /// TCP flows the host could not open
    pub tcp_failed: u64,
    /// lookups the host answered, and queries answered here
    pub dns_answers: u64,
}

/// The state the transport and its handle share.
pub struct HostQueue {
    ops: Vec<u8>,
    count: u32,
    /// data bytes in `ops`
    out_bytes: usize,
    tcp: HashMap<FlowId, TcpFlow>,
    udp: HashMap<FlowId, UdpFlow>,
    pending: VecDeque<u32>,
    resolved: VecDeque<(u32, Resolve)>,
    next_id: FlowId,
    next_qid: u32,
    /// the host resolves names (`RESOLVE`); otherwise DNS datagrams go to it as UDP
    resolve_names: bool,
    stats: QueueStats,
}

impl HostQueue {
    fn new(resolve_names: bool) -> Self {
        HostQueue { ops: Vec::new(), count: 0, out_bytes: 0, tcp: HashMap::new(), udp: HashMap::new(),
            pending: VecDeque::new(), resolved: VecDeque::new(), next_id: 0, next_qid: 0, resolve_names, stats: QueueStats::default() }
    }

    fn push(&mut self, op: u8, flags: u8, port: u16, id: u32, data: &[u8]) {
        self.ops.extend_from_slice(&[op, flags]);
        self.ops.extend_from_slice(&port.to_le_bytes());
        self.ops.extend_from_slice(&id.to_le_bytes());
        self.ops.extend_from_slice(&(data.len() as u32).to_le_bytes());
        self.ops.extend_from_slice(data);
        self.ops.resize(self.ops.len().next_multiple_of(4), 0);
        self.count += 1;
    }

    /// A flow id no live flow has: ids grow, so the host never sees one reused soon.
    fn new_id(&mut self) -> FlowId {
        loop {
            self.next_id = self.next_id.wrapping_add(1);
            let id = self.next_id;
            if id != 0 && !self.tcp.contains_key(&id) && !self.udp.contains_key(&id) { return id; }
        }
    }

    fn room(&self) -> usize { OUT_MAX - self.out_bytes }

    fn event(&mut self, ev: HostEvent<'_>) -> u32 {
        match ev {
            HostEvent::Connected { id, window } => match self.tcp.get_mut(&id) {
                Some(f) if f.state == TcpState::Connecting => { f.state = TcpState::Open; f.credit = u64::from(window); 1 }
                _ => 0,
            },
            HostEvent::Failed { id, reason } => match self.tcp.get_mut(&id) {
                Some(f) if f.state == TcpState::Connecting => {
                    f.state = TcpState::Failed(match reason {
                        fail::REFUSED => ErrorKind::ConnectionRefused,
                        fail::UNREACHABLE => ErrorKind::HostUnreachable,
                        fail::POLICY => ErrorKind::PermissionDenied,
                        fail::OFFLINE => ErrorKind::NetworkDown,
                        _ => ErrorKind::Other,
                    });
                    self.stats.tcp_failed += 1;
                    1
                }
                Some(f) if f.state == TcpState::Open => { f.state = TcpState::Reset; 1 }
                _ => 0,
            },
            HostEvent::TcpData { id, data } => match self.tcp.get_mut(&id) {
                Some(f) if matches!(f.state, TcpState::Connecting | TcpState::Open) && !f.eof => {
                    let n = data.len().min(TCP_RX_MAX - f.rx.len());
                    f.rx.extend(&data[..n]);
                    self.stats.bytes_to_guest += n as u64;
                    n as u32
                }
                // nobody will read it: taken, so the host does not offer it again
                _ => data.len() as u32,
            },
            HostEvent::TcpEof { id } => match self.tcp.get_mut(&id) {
                Some(f) if matches!(f.state, TcpState::Connecting | TcpState::Open) => { f.eof = true; 1 }
                _ => 0,
            },
            HostEvent::TcpReset { id } => match self.tcp.get_mut(&id) {
                Some(f) if f.state == TcpState::Connecting => { f.state = TcpState::Failed(ErrorKind::ConnectionReset); self.stats.tcp_failed += 1; 1 }
                Some(f) if f.state == TcpState::Open => { f.state = TcpState::Reset; 1 }
                _ => 0,
            },
            HostEvent::TxCredit { id, bytes } => match self.tcp.get_mut(&id) {
                Some(f) if f.state == TcpState::Open => { f.credit = f.credit.saturating_add(u64::from(bytes)); 1 }
                _ => 0,
            },
            HostEvent::UdpData { id, data } => match self.udp.get_mut(&id) {
                Some(f) if !f.local_dns && f.rx.len() < UDP_RX_MAX => {
                    f.rx.push_back(data.to_vec());
                    self.stats.bytes_to_guest += data.len() as u64;
                    1
                }
                _ => 0,
            },
            HostEvent::Resolved { qid, ttl, addrs } => {
                if !self.take_pending(qid) { return 0; }
                let addrs = addrs.as_chunks::<4>().0.to_vec();
                self.resolved.push_back((qid, Resolve::Addrs(addrs, ttl)));
                self.stats.dns_answers += 1;
                1
            }
            HostEvent::Unresolved { qid, rcode } => {
                if !self.take_pending(qid) { return 0; }
                self.resolved.push_back((qid, Resolve::Failed(if rcode == 3 { 3 } else { 2 })));
                self.stats.dns_answers += 1;
                1
            }
        }
    }

    fn take_pending(&mut self, qid: u32) -> bool {
        let Some(i) = self.pending.iter().position(|&q| q == qid) else { return false };
        self.pending.remove(i);
        true
    }

    fn tcp_live(&self) -> usize {
        self.tcp.values().filter(|f| matches!(f.state, TcpState::Connecting | TcpState::Open)).count()
    }
}

fn lock(q: &Mutex<HostQueue>) -> MutexGuard<'_, HostQueue> { q.lock().unwrap_or_else(|e| e.into_inner()) }

/// The NAT's side: give it to `Nat::with_transport`.
pub struct QueueTransport(Arc<Mutex<HostQueue>>);

/// The host's side: take the records, deliver the events.
#[derive(Clone)]
pub struct QueueHandle(Arc<Mutex<HostQueue>>);

impl QueueTransport {
    /// A transport whose host also resolves names (`RESOLVE`).
    #[allow(clippy::new_ret_no_self, reason = "the transport comes with the handle that drives it")]
    pub fn new() -> (QueueTransport, QueueHandle) { Self::with_dns(true) }

    /// With `resolve_names` false the guest's DNS datagrams go to the host as UDP to 10.0.2.3:53.
    pub fn with_dns(resolve_names: bool) -> (QueueTransport, QueueHandle) {
        let q = Arc::new(Mutex::new(HostQueue::new(resolve_names)));
        (QueueTransport(q.clone()), QueueHandle(q))
    }

    fn q(&self) -> MutexGuard<'_, HostQueue> { lock(&self.0) }
}

impl QueueHandle {
    /// Move the queued records into `out` (replacing what it held); returns how many.
    pub fn take_ops(&self, out: &mut Vec<u8>) -> u32 {
        let mut q = lock(&self.0);
        out.clear();
        std::mem::swap(out, &mut q.ops);
        q.out_bytes = 0;
        std::mem::take(&mut q.count)
    }

    /// Apply one host event. `TcpData` returns the bytes taken (all of them for a flow the guest
    /// has forgotten), `UdpData` 1 taken or 0 dropped, the rest 1 applied or 0 ignored.
    pub fn event(&self, ev: HostEvent<'_>) -> u32 { lock(&self.0).event(ev) }

    pub fn stats(&self) -> QueueStats { lock(&self.0).stats }

    /// TCP flows connecting or open.
    pub fn tcp_open(&self) -> usize { lock(&self.0).tcp_live() }
}

impl NatTransport for QueueTransport {
    fn tcp_connect(&mut self, dst: SocketAddrV4) -> Option<FlowId> {
        let mut q = self.q();
        if q.tcp_live() >= MAX_TCP_FLOWS { return None; }
        let id = q.new_id();
        q.tcp.insert(id, TcpFlow { state: TcpState::Connecting, rx: VecDeque::new(), credit: 0, eof: false });
        q.push(op::TCP_CONNECT, 0, dst.port(), id, &dst.ip().octets());
        Some(id)
    }

    fn tcp_state(&mut self, id: FlowId) -> Connect {
        match self.q().tcp.get(&id).map(|f| f.state) {
            Some(TcpState::Connecting) => Connect::Pending,
            Some(TcpState::Open) | Some(TcpState::Reset) => Connect::Open,
            Some(TcpState::Failed(kind)) => Connect::Failed(kind),
            None => Connect::Failed(ErrorKind::NotConnected),
        }
    }

    fn tcp_read(&mut self, id: FlowId, buf: &mut [u8]) -> io::Result<usize> {
        let mut q = self.q();
        let Some(f) = q.tcp.get_mut(&id) else { return Err(ErrorKind::NotConnected.into()) };
        if !f.rx.is_empty() {
            let n = f.rx.len().min(buf.len());
            for (b, x) in buf.iter_mut().zip(f.rx.drain(..n)) { *b = x; }
            return Ok(n);
        }
        match f.state {
            TcpState::Reset => Err(ErrorKind::ConnectionReset.into()),
            TcpState::Failed(kind) => Err(kind.into()),
            _ if f.eof => Ok(0),
            _ => Err(ErrorKind::WouldBlock.into()),
        }
    }

    fn tcp_write(&mut self, id: FlowId, data: &[u8]) -> io::Result<usize> {
        let mut q = self.q();
        let room = q.room();
        let Some(f) = q.tcp.get_mut(&id) else { return Err(ErrorKind::NotConnected.into()) };
        match f.state {
            // the host's bytes go to the guest first; the reset follows them
            TcpState::Reset if !f.rx.is_empty() => return Err(ErrorKind::WouldBlock.into()),
            TcpState::Reset => return Err(ErrorKind::ConnectionReset.into()),
            TcpState::Failed(kind) => return Err(kind.into()),
            TcpState::Connecting => return Err(ErrorKind::WouldBlock.into()),
            TcpState::Open => {}
        }
        let n = data.len().min(room).min(usize::try_from(f.credit).unwrap_or(usize::MAX));
        if n == 0 { return Err(ErrorKind::WouldBlock.into()); }
        f.credit -= n as u64;
        q.out_bytes += n;
        q.stats.bytes_to_host += n as u64;
        q.push(op::TCP_DATA, 0, 0, id, &data[..n]);
        Ok(n)
    }

    fn tcp_shutdown(&mut self, id: FlowId) {
        let mut q = self.q();
        if q.tcp.get(&id).is_some_and(|f| f.state == TcpState::Open) { q.push(op::TCP_SHUTDOWN, 0, 0, id, &[]); }
    }

    fn tcp_close(&mut self, id: FlowId, abort: bool) {
        let mut q = self.q();
        let Some(f) = q.tcp.remove(&id) else { return };
        // a flow the host failed or reset is already gone on its side
        if matches!(f.state, TcpState::Connecting | TcpState::Open) { q.push(op::TCP_CLOSE, u8::from(abort), 0, id, &[]); }
    }

    fn udp_open(&mut self, dst: SocketAddrV4) -> io::Result<FlowId> {
        let mut q = self.q();
        let id = q.new_id();
        // The host resolves A and AAAA; any other query the NAT relays is answered here, empty.
        let local_dns = q.resolve_names && dst.ip().octets() == RESOLVER && dst.port() == 53;
        q.udp.insert(id, UdpFlow { rx: VecDeque::new(), local_dns });
        if !local_dns { q.push(op::UDP_OPEN, 0, dst.port(), id, &dst.ip().octets()); }
        Ok(id)
    }

    fn udp_send(&mut self, id: FlowId, data: &[u8]) -> io::Result<usize> {
        let mut q = self.q();
        let room = q.room();
        let Some(f) = q.udp.get_mut(&id) else { return Err(ErrorKind::NotConnected.into()) };
        if f.local_dns {
            let Some(question) = dns_question(data) else { return Err(ErrorKind::InvalidData.into()) };
            if f.rx.len() < UDP_RX_MAX {
                f.rx.push_back(dns_reply(&data[..question.end], question.qtype, &[], 0, 0));
                q.stats.dns_answers += 1;
            }
            return Ok(data.len());
        }
        if data.len() > room { return Err(ErrorKind::WouldBlock.into()); }
        q.out_bytes += data.len();
        q.stats.bytes_to_host += data.len() as u64;
        q.push(op::UDP_DATA, 0, 0, id, data);
        Ok(data.len())
    }

    fn udp_recv(&mut self, id: FlowId, buf: &mut [u8]) -> io::Result<usize> {
        let mut q = self.q();
        let Some(d) = q.udp.get_mut(&id).and_then(|f| f.rx.pop_front()) else { return Err(ErrorKind::WouldBlock.into()) };
        let n = d.len().min(buf.len());
        buf[..n].copy_from_slice(&d[..n]);
        Ok(n)
    }

    fn udp_close(&mut self, id: FlowId) {
        let mut q = self.q();
        if let Some(f) = q.udp.remove(&id) {
            if !f.local_dns { q.push(op::UDP_CLOSE, 0, 0, id, &[]); }
        }
    }

    fn resolver(&self) -> [u8; 4] { RESOLVER }

    fn resolve(&mut self, name: &str, qtype: u16) -> Option<u32> {
        let mut q = self.q();
        if !q.resolve_names { return None; }
        q.next_qid = q.next_qid.wrapping_add(1);
        let qid = q.next_qid;
        if q.pending.len() >= PENDING_MAX { q.pending.pop_front(); }
        q.pending.push_back(qid);
        q.push(op::RESOLVE, 0, qtype, qid, name.as_bytes());
        Some(qid)
    }

    fn resolved(&mut self) -> Option<(u32, Resolve)> { self.q().resolved.pop_front() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// (op, flags, port, id, data) of every record taken.
    fn records(h: &QueueHandle) -> Vec<(u8, u8, u16, u32, Vec<u8>)> {
        let mut buf = Vec::new();
        let n = h.take_ops(&mut buf);
        let mut out = Vec::new();
        let mut at = 0;
        for _ in 0..n {
            let r = &buf[at..];
            let len = u32::from_le_bytes(r[8..12].try_into().unwrap()) as usize;
            out.push((r[0], r[1], u16::from_le_bytes([r[2], r[3]]), u32::from_le_bytes(r[4..8].try_into().unwrap()), r[12..12 + len].to_vec()));
            at += (RECORD_HEADER + len).next_multiple_of(4);
        }
        assert_eq!(at, buf.len(), "records fill the buffer exactly");
        out
    }

    fn dst(port: u16) -> SocketAddrV4 { SocketAddrV4::new(Ipv4Addr::new(93, 184, 215, 14), port) }

    fn open(t: &mut QueueTransport, h: &QueueHandle, window: u32) -> FlowId {
        let id = t.tcp_connect(dst(80)).unwrap();
        assert_eq!(h.event(HostEvent::Connected { id, window }), 1);
        records(h);
        id
    }

    #[test]
    fn records_are_little_endian_and_padded_to_four_bytes() {
        let (mut t, h) = QueueTransport::new();
        let id = t.tcp_connect(dst(8080)).unwrap();
        assert_eq!(t.tcp_state(id), Connect::Pending);
        let q = t.resolve("example.com", 28).unwrap();
        let mut buf = Vec::new();
        assert_eq!(h.take_ops(&mut buf), 2);
        assert_eq!(&buf[..16], &[op::TCP_CONNECT, 0, 0x90, 0x1f, id as u8, 0, 0, 0, 4, 0, 0, 0, 93, 184, 215, 14]);
        assert_eq!(&buf[16..28], &[op::RESOLVE, 0, 28, 0, q as u8, 0, 0, 0, 11, 0, 0, 0]);
        assert_eq!(&buf[28..39], b"example.com");
        assert_eq!(buf.len(), 40, "11 bytes of name padded to 12");
        assert_eq!(h.take_ops(&mut buf), 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn the_seventeenth_connect_waits_until_a_flow_closes() {
        let (mut t, h) = QueueTransport::new();
        let ids: Vec<_> = (0..MAX_TCP_FLOWS).map(|_| t.tcp_connect(dst(80)).unwrap()).collect();
        assert_eq!(t.tcp_connect(dst(80)), None);
        assert_eq!(h.tcp_open(), MAX_TCP_FLOWS);
        t.tcp_close(ids[3], false);
        assert!(t.tcp_connect(dst(80)).is_some());
        let closes: Vec<_> = records(&h).into_iter().filter(|r| r.0 == op::TCP_CLOSE).collect();
        assert_eq!(closes, vec![(op::TCP_CLOSE, 0, 0, ids[3], vec![])]);
    }

    #[test]
    fn writes_follow_the_credit_and_the_outgoing_limit() {
        let (mut t, h) = QueueTransport::new();
        let id = t.tcp_connect(dst(80)).unwrap();
        assert_eq!(t.tcp_write(id, b"early").unwrap_err().kind(), ErrorKind::WouldBlock);
        assert_eq!(h.event(HostEvent::Connected { id, window: 4 }), 1);
        assert_eq!(t.tcp_state(id), Connect::Open);
        assert_eq!(t.tcp_write(id, b"hello").unwrap(), 4);
        assert_eq!(t.tcp_write(id, b"o").unwrap_err().kind(), ErrorKind::WouldBlock);
        assert_eq!(h.event(HostEvent::TxCredit { id, bytes: u32::MAX }), 1);
        assert_eq!(t.tcp_write(id, b"o").unwrap(), 1);
        let data: Vec<_> = records(&h).into_iter().filter(|r| r.0 == op::TCP_DATA).map(|r| r.4).collect();
        assert_eq!(data, vec![b"hell".to_vec(), b"o".to_vec()]);
        // Untaken records hold at most OUT_MAX data bytes; taking them makes room again.
        let big = vec![7u8; OUT_MAX];
        assert_eq!(t.tcp_write(id, &big).unwrap(), OUT_MAX);
        assert_eq!(t.tcp_write(id, b"x").unwrap_err().kind(), ErrorKind::WouldBlock);
        let udp = t.udp_open(dst(123)).unwrap();
        assert_eq!(t.udp_send(udp, b"x").unwrap_err().kind(), ErrorKind::WouldBlock);
        records(&h);
        assert_eq!(t.tcp_write(id, b"x").unwrap(), 1);
        assert_eq!(h.stats().bytes_to_host, 5 + OUT_MAX as u64 + 1);
    }

    #[test]
    fn tcp_data_takes_only_what_fits_and_eof_follows_the_bytes() {
        let (mut t, h) = QueueTransport::new();
        let id = open(&mut t, &h, 65536);
        let mut buf = [0u8; 8];
        assert_eq!(t.tcp_read(id, &mut buf).unwrap_err().kind(), ErrorKind::WouldBlock);
        let big = vec![1u8; TCP_RX_MAX + 100];
        assert_eq!(h.event(HostEvent::TcpData { id, data: &big }), TCP_RX_MAX as u32);
        assert_eq!(h.event(HostEvent::TcpData { id, data: b"more" }), 0, "full: the host offers it again");
        assert_eq!(t.tcp_read(id, &mut buf).unwrap(), 8);
        assert_eq!(h.event(HostEvent::TcpData { id, data: b"more" }), 4);
        assert_eq!(h.event(HostEvent::TcpEof { id }), 1);
        let mut all = vec![0u8; TCP_RX_MAX * 2];
        let left = TCP_RX_MAX - 8 + 4;
        assert_eq!(t.tcp_read(id, &mut all).unwrap(), left);
        assert_eq!(&all[left - 4..left], b"more");
        assert_eq!(t.tcp_read(id, &mut all).unwrap(), 0, "EOF after the last byte");
        assert_eq!(h.stats().bytes_to_guest, TCP_RX_MAX as u64 + 4);
        // EOF closes only the host's direction: the guest still writes, then closes.
        assert_eq!(t.tcp_write(id, b"bye").unwrap(), 3);
        t.tcp_shutdown(id);
        t.tcp_close(id, false);
        let ops: Vec<_> = records(&h).into_iter().map(|r| (r.0, r.1)).collect();
        assert_eq!(ops, vec![(op::TCP_DATA, 0), (op::TCP_SHUTDOWN, 0), (op::TCP_CLOSE, 0)]);
        assert_eq!(h.event(HostEvent::TcpData { id, data: b"late" }), 4, "a forgotten flow's data is taken and dropped");
    }

    #[test]
    fn reset_comes_after_the_received_bytes_and_needs_no_close() {
        let (mut t, h) = QueueTransport::new();
        let id = open(&mut t, &h, 100);
        assert_eq!(h.event(HostEvent::TcpData { id, data: b"tail" }), 4);
        assert_eq!(h.event(HostEvent::TcpReset { id }), 1);
        assert_eq!(t.tcp_write(id, b"x").unwrap_err().kind(), ErrorKind::WouldBlock, "the bytes go first");
        let mut buf = [0u8; 8];
        assert_eq!(t.tcp_read(id, &mut buf).unwrap(), 4);
        assert_eq!(t.tcp_read(id, &mut buf).unwrap_err().kind(), ErrorKind::ConnectionReset);
        assert_eq!(t.tcp_write(id, b"x").unwrap_err().kind(), ErrorKind::ConnectionReset);
        t.tcp_close(id, true);
        assert!(records(&h).is_empty(), "the host already forgot a reset flow");
        assert_eq!(h.event(HostEvent::TcpReset { id }), 0);
    }

    #[test]
    fn failed_connect_reports_its_reason_and_close_is_silent() {
        let (mut t, h) = QueueTransport::new();
        let kinds = [(fail::REFUSED, ErrorKind::ConnectionRefused), (fail::UNREACHABLE, ErrorKind::HostUnreachable),
            (fail::POLICY, ErrorKind::PermissionDenied), (fail::OFFLINE, ErrorKind::NetworkDown)];
        for (reason, kind) in kinds {
            let id = t.tcp_connect(dst(80)).unwrap();
            assert_eq!(h.event(HostEvent::Failed { id, reason }), 1);
            assert_eq!(t.tcp_state(id), Connect::Failed(kind));
            assert_eq!(h.event(HostEvent::Connected { id, window: 1 }), 0, "too late");
            t.tcp_close(id, true); // the NAT closes a failed flow
        }
        assert!(records(&h).iter().all(|r| r.0 == op::TCP_CONNECT));
        assert_eq!(h.stats().tcp_failed, 4);
        assert_eq!(h.tcp_open(), 0);
    }

    #[test]
    fn lookups_answer_once_and_unknown_ones_are_ignored() {
        let (mut t, h) = QueueTransport::new();
        let a = t.resolve("example.com", 1).unwrap();
        let b = t.resolve("nowhere.invalid", 1).unwrap();
        assert_ne!(a, b);
        let addrs = [93, 184, 215, 14, 1, 2, 3, 4];
        assert_eq!(h.event(HostEvent::Resolved { qid: a, ttl: 60, addrs: &addrs }), 1);
        assert_eq!(h.event(HostEvent::Resolved { qid: a, ttl: 60, addrs: &addrs }), 0, "answered already");
        assert_eq!(h.event(HostEvent::Unresolved { qid: b, rcode: 3 }), 1);
        assert_eq!(h.event(HostEvent::Unresolved { qid: 999, rcode: 3 }), 0);
        assert_eq!(t.resolved(), Some((a, Resolve::Addrs(vec![[93, 184, 215, 14], [1, 2, 3, 4]], 60))));
        assert_eq!(t.resolved(), Some((b, Resolve::Failed(3))));
        assert_eq!(t.resolved(), None);
        assert_eq!(h.stats().dns_answers, 2);
        let (mut t, _h) = QueueTransport::with_dns(false);
        assert_eq!(t.resolve("example.com", 1), None, "without DNS through the host the NAT relays datagrams");
    }

    #[test]
    fn udp_keeps_sixteen_datagrams_per_flow() {
        let (mut t, h) = QueueTransport::new();
        let id = t.udp_open(dst(123)).unwrap();
        assert_eq!(t.udp_send(id, b"query").unwrap(), 5);
        for i in 0..UDP_RX_MAX { assert_eq!(h.event(HostEvent::UdpData { id, data: &[i as u8] }), 1); }
        assert_eq!(h.event(HostEvent::UdpData { id, data: b"x" }), 0, "dropped");
        let mut buf = [0u8; 4];
        assert_eq!(t.udp_recv(id, &mut buf).unwrap(), 1);
        assert_eq!(buf[0], 0);
        t.udp_close(id);
        assert_eq!(h.event(HostEvent::UdpData { id, data: b"x" }), 0);
        let ops: Vec<_> = records(&h).into_iter().map(|r| (r.0, r.2, r.4)).collect();
        assert_eq!(ops, vec![(op::UDP_OPEN, 123, vec![93, 184, 215, 14]), (op::UDP_DATA, 0, b"query".to_vec()), (op::UDP_CLOSE, 0, vec![])]);
    }

    #[test]
    fn other_dns_queries_are_answered_here_empty() {
        let (mut t, h) = QueueTransport::new();
        let id = t.udp_open(SocketAddrV4::new(RESOLVER.into(), 53)).unwrap();
        // a TXT query for "x"
        let query = [0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, b'x', 0, 0, 16, 0, 1];
        assert_eq!(t.udp_send(id, &query).unwrap(), query.len());
        let mut buf = [0u8; 64];
        let n = t.udp_recv(id, &mut buf).unwrap();
        assert_eq!(&buf[..4], &[0x12, 0x34, 0x81, 0x80], "same id, a NOERROR response");
        assert_eq!(&buf[6..8], &[0, 0], "no answers");
        assert_eq!(n, query.len());
        t.udp_close(id);
        assert!(records(&h).is_empty(), "nothing for the host");
        let (mut t, h) = QueueTransport::with_dns(false);
        t.udp_open(SocketAddrV4::new(RESOLVER.into(), 53)).unwrap();
        assert_eq!(records(&h)[0].0, op::UDP_OPEN);
    }

    #[test]
    fn abi_events_decode() {
        assert_eq!(HostEvent::from_abi(ev::CONNECTED, 5, 65536, &[]), Some(HostEvent::Connected { id: 5, window: 65536 }));
        assert_eq!(HostEvent::from_abi(ev::RESOLVED, 2, 60, &[1, 2, 3, 4]), Some(HostEvent::Resolved { qid: 2, ttl: 60, addrs: &[1, 2, 3, 4] }));
        assert_eq!(HostEvent::from_abi(0, 1, 0, &[]), None);
        assert_eq!(HostEvent::from_abi(10, 1, 0, &[]), None);
    }

    /// The NAT over the queue: a guest SYN becomes a record, the host's events a SYN-ACK and data.
    #[test]
    fn a_nat_over_the_queue_connects_and_relays() {
        let (t, h) = QueueTransport::new();
        let mut nat = super::super::Nat::with_transport(false, Box::new(t));
        let mut syn = vec![0u8; 20];
        syn[..2].copy_from_slice(&1234u16.to_be_bytes());
        syn[2..4].copy_from_slice(&80u16.to_be_bytes());
        syn[4..8].copy_from_slice(&99u32.to_be_bytes());
        syn[12] = 0x50; syn[13] = 0x02;
        syn[14..16].copy_from_slice(&5840u16.to_be_bytes());
        assert!(nat.tcp_in(&[2; 6], &[10, 0, 2, 15], &[93, 184, 215, 14], &syn, 0).is_empty());
        let r = records(&h);
        assert_eq!((r[0].0, r[0].2, r[0].4.as_slice()), (op::TCP_CONNECT, 80, &[93, 184, 215, 14][..]));
        assert!(nat.poll(1).is_empty(), "still connecting");
        h.event(HostEvent::Connected { id: r[0].3, window: 65536 });
        let out = nat.poll(2);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0][47], 0x12, "SYN-ACK");
        nat.abort_all();
        assert_eq!(records(&h), vec![(op::TCP_CLOSE, 1, 0, r[0].3, vec![])]);
    }
}
