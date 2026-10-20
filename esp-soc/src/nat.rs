//! User-mode NAT: guest TCP and UDP flows are relayed through ordinary host sockets.
//!
//! TCP uses bounded buffers in both directions. SYN, data and FIN share one retransmission queue,
//! so a dropped virtual-air frame cannot discard acknowledged guest bytes or strand a handshake.
//! This is a small relay, without window scaling, SACK or congestion control.

use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use crate::net::packet::{ethernet, ip_packet, transport_checksum, udp_packet, GATEWAY_MAC};

pub(crate) const MSS: usize = 1400;
pub(crate) const WINDOW: u16 = 5840;
pub(crate) const RETRANSMIT_US: u64 = 300_000;
pub(crate) const IDLE_CLOSE_US: u64 = 120_000_000;
pub(crate) const TIME_WAIT_US: u64 = 30_000_000;
pub(crate) const MAX_FLOWS: usize = 64;
const MAX_CONNECTS: usize = 8;
const UDP_BATCH: usize = 16;
pub(crate) const FIN: u8 = 0x01;
pub(crate) const SYN: u8 = 0x02;
pub(crate) const RST: u8 = 0x04;
pub(crate) const ACK: u8 = 0x10;
pub(crate) const PSH: u8 = 0x08;

fn ip(a: &[u8; 4]) -> Ipv4Addr { Ipv4Addr::from(*a) }
fn address(a: &[u8; 4], port: u16) -> SocketAddr { SocketAddr::new(IpAddr::V4(ip(a)), port) }

// A permit belongs to the blocking connect worker, not its guest flow. Removing a flow with RST
// must not free a slot while connect_timeout is still running on the host.
static CONNECTING: AtomicUsize = AtomicUsize::new(0);
struct ConnectPermit<'a>(&'a AtomicUsize);
impl<'a> ConnectPermit<'a> {
    fn acquire(counter: &'a AtomicUsize) -> Option<Self> {
        counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| (n < MAX_CONNECTS).then_some(n + 1)).ok().map(|_| Self(counter))
    }
}
impl Drop for ConnectPermit<'_> {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::Relaxed); }
}

fn connect(addr: SocketAddr) -> Option<Receiver<io::Result<TcpStream>>> {
    let permit = ConnectPermit::acquire(&CONNECTING)?;
    let (tx, rx) = channel();
    std::thread::Builder::new().name("nat-connect".into()).spawn(move || {
        let _permit = permit;
        let result = TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(4))
            .and_then(|sock| { sock.set_nonblocking(true)?; sock.set_nodelay(true)?; Ok(sock) });
        let _ = tx.send(result);
    }).ok()?;
    Some(rx)
}

pub(crate) enum Transport { Connecting(Receiver<io::Result<TcpStream>>), Connected(TcpStream), TimeWait, Closed }
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuestWrite { Open, Draining, Closed }

pub(crate) struct Sent {
    seq: u32,
    flags: u8,
    data: Vec<u8>,
    /// TCP options (an MSS on the SYN, empty on data): a retransmission repeats them verbatim
    options: Vec<u8>,
    at_us: u64,
}
impl Sent {
    fn len(&self) -> usize { self.data.len() + usize::from(self.flags & (SYN | FIN) != 0) }
}

/// One TCP connection between the guest and the host side. The direction is fixed — `to_host`
/// carries guest bytes to the host socket, `unacked` carries host bytes to the guest — so one
/// state machine serves a guest-initiated flow (the NAT dials and answers SYN|ACK) and a
/// host-initiated one (inbound forwarding has the socket and sends the SYN).
pub(crate) struct Tcp {
    pub(crate) guest_mac: [u8; 6], pub(crate) guest_ip: [u8; 4], pub(crate) guest_port: u16,
    pub(crate) dst_ip: [u8; 4], pub(crate) dst_port: u16,
    /// The MAC frames toward the guest are sourced from: the gateway for a NAT flow, the
    /// forwarding peer for an inbound one.
    pub(crate) src_mac: [u8; 6],
    pub(crate) transport: Transport,
    pub(crate) guest_write: GuestWrite,
    pub(crate) host_closed: bool,
    pub(crate) our_seq: u32,
    pub(crate) guest_seq: u32,
    pub(crate) guest_window: u16,
    pub(crate) to_host: VecDeque<u8>,
    pub(crate) unacked: VecDeque<Sent>,
    pub(crate) last_activity_us: u64,
}

impl Tcp {
    pub(crate) fn handshake_pending(&self) -> bool { self.unacked.front().is_some_and(|s| s.flags & SYN != 0) }
    pub(crate) fn in_flight(&self) -> usize { self.unacked.iter().map(Sent::len).sum() }

    pub(crate) fn acknowledge(&mut self, ack: u32) {
        let Some(first) = self.unacked.front() else { return };
        let mut count = ack.wrapping_sub(first.seq) as usize;
        if count > self.in_flight() { return; } // old or beyond anything sent
        while count > 0 {
            let sent = self.unacked.front_mut().unwrap();
            let n = sent.len();
            if count < n {
                sent.data.drain(..count);
                sent.seq = ack;
                break;
            }
            count -= n;
            self.unacked.pop_front();
        }
    }

    pub(crate) fn accept(&mut self, seq: u32, data: &[u8], fin: bool) {
        if self.guest_write != GuestWrite::Open || seq != self.guest_seq { return; }
        if data.len() > WINDOW as usize - self.to_host.len() { return; }
        self.to_host.extend(data);
        self.guest_seq = self.guest_seq.wrapping_add(data.len() as u32);
        if fin {
            self.guest_seq = self.guest_seq.wrapping_add(1);
            self.guest_write = GuestWrite::Draining;
        }
    }

    pub(crate) fn segment(&self, flags: u8, data: &[u8], seq: u32) -> Vec<u8> {
        self.segment_with(flags, data, seq, &[])
    }

    /// `options` must be a multiple of four bytes; the data offset follows their length.
    pub(crate) fn segment_with(&self, flags: u8, data: &[u8], seq: u32, options: &[u8]) -> Vec<u8> {
        let mut seg = Vec::with_capacity(20 + options.len() + data.len());
        seg.extend_from_slice(&self.dst_port.to_be_bytes());
        seg.extend_from_slice(&self.guest_port.to_be_bytes());
        seg.extend_from_slice(&seq.to_be_bytes());
        seg.extend_from_slice(&self.guest_seq.to_be_bytes());
        seg.extend_from_slice(&[(((20 + options.len()) / 4) as u8) << 4, flags]);
        seg.extend_from_slice(&(WINDOW - self.to_host.len() as u16).to_be_bytes());
        seg.extend_from_slice(&[0; 4]);
        seg.extend_from_slice(options);
        seg.extend_from_slice(data);
        let check = transport_checksum(&self.dst_ip, &self.guest_ip, 6, &seg);
        seg[16..18].copy_from_slice(&check.to_be_bytes());
        ethernet(&self.guest_mac, &self.src_mac, 0x0800, &ip_packet(6, &self.dst_ip, &self.guest_ip, &seg))
    }

    pub(crate) fn send(&mut self, flags: u8, data: Vec<u8>, now_us: u64) -> Vec<u8> {
        self.send_with(flags, data, Vec::new(), now_us)
    }

    pub(crate) fn send_with(&mut self, flags: u8, data: Vec<u8>, options: Vec<u8>, now_us: u64) -> Vec<u8> {
        let frame = self.segment_with(flags, &data, self.our_seq, &options);
        let sent = Sent { seq: self.our_seq, flags, data, options, at_us: now_us };
        self.our_seq = self.our_seq.wrapping_add(sent.len() as u32);
        self.unacked.push_back(sent);
        frame
    }

    pub(crate) fn retransmit(&mut self, now_us: u64) -> Option<Vec<u8>> {
        let sent = self.unacked.front_mut()?;
        if now_us.wrapping_sub(sent.at_us) < RETRANSMIT_US { return None; }
        sent.at_us = now_us;
        let sent = self.unacked.front().unwrap();
        Some(self.segment_with(sent.flags, &sent.data, sent.seq, &sent.options))
    }

    pub(crate) fn closed(&self) -> bool {
        matches!(self.transport, Transport::Closed)
    }

    /// Move bytes between an established flow's host socket and the guest: flush what the guest
    /// sent, close our write side once it has drained, then read the host up to the guest's
    /// advertised window. Returns the frames for the guest and (bytes written to the host, bytes
    /// queued toward the guest) so each caller keeps its own accounting.
    pub(crate) fn pump(&mut self, now_us: u64, tag: Option<&str>) -> (Vec<Vec<u8>>, usize, usize) {
        enum Read { Data(usize), Eof, Failed }
        let mut out = Vec::new();
        let mut to_guest = 0;
        if self.host_closed && self.guest_write == GuestWrite::Closed && self.unacked.is_empty()
            && matches!(self.transport, Transport::Connected(_)) {
            // Release the host socket but keep enough state to re-ACK a lost final ACK.
            self.transport = Transport::TimeWait;
            self.last_activity_us = now_us;
            return (out, 0, 0);
        }
        if !matches!(self.transport, Transport::Connected(_)) || self.handshake_pending() {
            return (out, 0, 0);
        }
        let queued = self.to_host.len();
        let flushed = match &mut self.transport {
            Transport::Connected(sock) => flush_pending(sock, &mut self.to_host),
            _ => unreachable!(),
        };
        let to_host = match flushed {
            Ok(n) => n,
            Err(_) => {
                out.push(self.segment(RST | ACK, &[], self.our_seq));
                self.transport = Transport::Closed;
                return (out, 0, 0);
            }
        };
        if self.guest_write == GuestWrite::Draining && self.to_host.is_empty() {
            if let Transport::Connected(sock) = &mut self.transport { let _ = sock.shutdown(std::net::Shutdown::Write); }
            self.guest_write = GuestWrite::Closed;
        }
        if self.to_host.len() < queued { out.push(self.segment(ACK, &[], self.our_seq)); } // reopen the receive window
        // One byte at a zero window becomes a persist probe. The same retransmission
        // queue retries it, recovering even when the guest's window-update ACK is lost.
        let send_window = usize::from(self.guest_window.clamp(1, WINDOW));
        while !self.host_closed && self.in_flight() < send_window {
            let room = send_window - self.in_flight();
            let mut buf = [0; MSS];
            let status = match &mut self.transport {
                Transport::Connected(sock) => match sock.read(&mut buf[..room.min(MSS)]) {
                    Ok(0) => Read::Eof,
                    Ok(n) => Read::Data(n),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => Read::Failed,
                },
                _ => break,
            };
            match status {
                Read::Data(n) => {
                    if let Some(tag) = tag { eprintln!("[{tag}] TCP {}:{} -> guest {} bytes (seq {})", ip(&self.dst_ip), self.dst_port, n, self.our_seq); }
                    self.last_activity_us = now_us;
                    to_guest += n;
                    out.push(self.send(PSH | ACK, buf[..n].to_vec(), now_us));
                }
                Read::Eof => {
                    self.host_closed = true;
                    out.push(self.send(FIN | ACK, Vec::new(), now_us));
                }
                Read::Failed => {
                    out.push(self.segment(RST | ACK, &[], self.our_seq));
                    self.transport = Transport::Closed;
                    break;
                }
            }
        }
        (out, to_host, to_guest)
    }
}

/// Preserve every unwritten byte across short writes and WouldBlock. Returns bytes actually sent.
pub(crate) fn flush_pending(writer: &mut impl Write, pending: &mut VecDeque<u8>) -> io::Result<usize> {
    let mut written = 0;
    while !pending.is_empty() {
        match writer.write(pending.as_slices().0) {
            Ok(0) => return Err(ErrorKind::WriteZero.into()),
            Ok(n) => { pending.drain(..n); written += n; }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => return Err(e),
        }
    }
    Ok(written)
}

struct Udp {
    guest_mac: [u8; 6], guest_ip: [u8; 4], guest_port: u16, dst_ip: [u8; 4], dst_port: u16,
    reply_src: [u8; 4], // DNS's guest-visible address, before host resolver redirection
    sock: UdpSocket,
    last_activity_us: u64,
}

pub struct Nat {
    tcp: Vec<Tcp>,
    udp: Vec<Udp>,
    isn: u32,
    pub resolver: [u8; 4],
    pub log: bool,
    pub tcp_opened: u64, pub tcp_refused: u64, pub udp_flows: u64,
    pub udp_evicted: u64, pub udp_send_errors: u64,
    pub bytes_to_host: u64, pub bytes_to_guest: u64,
}

impl Nat {
    pub fn new(log: bool) -> Self {
        Self { tcp: Vec::new(), udp: Vec::new(), isn: 0x1000, resolver: host_resolver(), log,
            tcp_opened: 0, tcp_refused: 0, udp_flows: 0, udp_evicted: 0, udp_send_errors: 0, bytes_to_host: 0, bytes_to_guest: 0 }
    }

    /// Forward a UDP datagram through a connected socket, which accepts replies only from its peer.
    #[allow(clippy::too_many_arguments, reason = "packet fields stay explicit at the protocol boundary")]
    pub fn udp_out(&mut self, gmac: &[u8; 6], gip: &[u8; 4], sport: u16, dip: &[u8; 4], reply_src: &[u8; 4],
                   dport: u16, payload: &[u8], now_us: u64) {
        let idx = self.udp.iter().position(|f| f.guest_ip == *gip && f.guest_port == sport && f.dst_ip == *dip && f.dst_port == dport);
        let idx = match idx {
            Some(i) => i,
            None => {
                let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return };
                if sock.connect(address(dip, dport)).is_err() || sock.set_nonblocking(true).is_err() { return; }
                if self.udp.len() >= MAX_FLOWS {
                    let oldest = self.udp.iter().enumerate().max_by_key(|(_, f)| now_us.wrapping_sub(f.last_activity_us)).unwrap().0;
                    self.udp.remove(oldest);
                    self.udp_evicted += 1;
                    if self.log { eprintln!("[nat] UDP evicted least recently active flow (table full)"); }
                }
                self.udp.push(Udp { guest_mac: *gmac, guest_ip: *gip, guest_port: sport, dst_ip: *dip, dst_port: dport,
                    reply_src: *reply_src, sock, last_activity_us: now_us });
                self.udp_flows += 1;
                if self.log { eprintln!("[nat] UDP {}:{} -> {}:{} ({} bytes)", ip(gip), sport, ip(dip), dport, payload.len()); }
                self.udp.len() - 1
            }
        };
        let flow = &mut self.udp[idx];
        flow.last_activity_us = now_us;
        match flow.sock.send(payload) {
            Ok(n) => self.bytes_to_host += n as u64,
            Err(e) => {
                self.udp_send_errors += 1;
                if self.log { eprintln!("[nat] UDP {}:{} send failed: {}", ip(dip), dport, e); }
            }
        }
    }

    pub fn tcp_in(&mut self, gmac: &[u8; 6], gip: &[u8; 4], dip: &[u8; 4], seg: &[u8], now_us: u64) -> Vec<Vec<u8>> {
        self.tcp_in_with_connect(gmac, gip, dip, seg, now_us, connect)
    }

    #[allow(clippy::too_many_arguments, reason = "inject the connector without process-global state in admission tests")]
    fn tcp_in_with_connect(&mut self, gmac: &[u8; 6], gip: &[u8; 4], dip: &[u8; 4], seg: &[u8], now_us: u64,
                           connector: impl FnOnce(SocketAddr) -> Option<Receiver<io::Result<TcpStream>>>) -> Vec<Vec<u8>> {
        if seg.len() < 20 { return Vec::new(); }
        let off = ((seg[12] >> 4) as usize) * 4;
        if off < 20 || off > seg.len() { return Vec::new(); }
        let sport = u16::from_be_bytes([seg[0], seg[1]]);
        let dport = u16::from_be_bytes([seg[2], seg[3]]);
        let seq = u32::from_be_bytes(seg[4..8].try_into().unwrap());
        let ack = u32::from_be_bytes(seg[8..12].try_into().unwrap());
        let flags = seg[13];
        let data = &seg[off..];
        let window = u16::from_be_bytes([seg[14], seg[15]]);
        let idx = self.tcp.iter().position(|c| c.guest_ip == *gip && c.guest_port == sport && c.dst_port == dport && c.dst_ip == *dip);
        if flags & (SYN | ACK | RST) == SYN && idx.is_none() {
            let evict = if self.tcp.len() >= MAX_FLOWS {
                // Finished flows remember final ACKs only while their bounded slots are spare.
                let Some(i) = self.tcp.iter().position(|c| matches!(c.transport, Transport::TimeWait | Transport::Closed)) else { return Vec::new() };
                Some(i)
            } else { None };
            let Some(pending) = connector(address(dip, dport)) else { return Vec::new() };
            if let Some(i) = evict { self.tcp.remove(i); }
            self.isn = self.isn.wrapping_add(0x10000);
            self.tcp.push(Tcp { guest_mac: *gmac, guest_ip: *gip, guest_port: sport, dst_ip: *dip, dst_port: dport,
                src_mac: GATEWAY_MAC, transport: Transport::Connecting(pending), guest_write: GuestWrite::Open, host_closed: false,
                our_seq: self.isn, guest_seq: seq.wrapping_add(1), guest_window: window,
                to_host: VecDeque::new(), unacked: VecDeque::new(), last_activity_us: now_us });
            if self.log { eprintln!("[nat] TCP {}:{} -> {}:{} connecting", ip(gip), sport, ip(dip), dport); }
            return Vec::new();
        }
        let Some(i) = idx else { return Vec::new() };
        let c = &mut self.tcp[i];
        c.last_activity_us = now_us;
        if flags & RST != 0 { c.transport = Transport::Closed; return Vec::new(); }
        if matches!(c.transport, Transport::TimeWait) {
            return if flags & FIN != 0 { vec![c.segment(ACK, &[], c.our_seq)] } else { Vec::new() };
        }
        if flags & SYN != 0 {
            return if c.handshake_pending() { vec![c.segment(SYN | ACK, &[], c.unacked[0].seq)] } else { Vec::new() };
        }
        if !matches!(c.transport, Transport::Connected(_)) { return Vec::new(); }
        if flags & ACK != 0 { c.acknowledge(ack); c.guest_window = window; }
        if c.handshake_pending() { return Vec::new(); }
        if !data.is_empty() || flags & FIN != 0 {
            c.accept(seq, data, flags & FIN != 0);
            return vec![c.segment(ACK, &[], c.our_seq)];
        }
        Vec::new()
    }

    /// Pump a bounded amount of host traffic, then retransmit and expire flows.
    pub fn poll(&mut self, now_us: u64) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for c in &mut self.tcp {
            if c.closed() { continue; }
            if c.host_closed && c.guest_write == GuestWrite::Closed && c.unacked.is_empty()
                && matches!(c.transport, Transport::Connected(_)) {
                // Release the host socket but keep enough state to re-ACK a lost final ACK.
                c.transport = Transport::TimeWait;
                c.last_activity_us = now_us;
            }
            if let Transport::Connecting(rx) = &c.transport {
                let result = match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Disconnected) => Some(Err(ErrorKind::ConnectionAborted.into())),
                    Err(TryRecvError::Empty) => None,
                };
                match result {
                    Some(Ok(sock)) => {
                        c.transport = Transport::Connected(sock);
                        self.tcp_opened += 1;
                        if self.log { eprintln!("[nat] TCP {}:{} connected", ip(&c.dst_ip), c.dst_port); }
                        out.push(c.send(SYN | ACK, Vec::new(), now_us));
                    }
                    Some(Err(e)) => {
                        if self.log { eprintln!("[nat] TCP {}:{} failed: {}", ip(&c.dst_ip), c.dst_port, e); }
                        self.tcp_refused += 1;
                        out.push(c.segment(RST | ACK, &[], c.our_seq));
                        c.transport = Transport::Closed;
                    }
                    None => {}
                }
            }
            if matches!(c.transport, Transport::Connected(_)) && !c.handshake_pending() {
                let (frames, to_host, to_guest) = c.pump(now_us, self.log.then_some("nat"));
                self.bytes_to_host += to_host as u64;
                self.bytes_to_guest += to_guest as u64;
                out.extend(frames);
            }
            if let Some(frame) = c.retransmit(now_us) { out.push(frame); }
        }
        for flow in &mut self.udp {
            let mut buf = [0; 2048];
            for _ in 0..UDP_BATCH {
                let Ok(n) = flow.sock.recv(&mut buf) else { break };
                if self.log { eprintln!("[nat] UDP reply {} bytes -> guest port {}", n, flow.guest_port); }
                self.bytes_to_guest += n as u64;
                flow.last_activity_us = now_us;
                let udp = udp_packet(&flow.reply_src, &flow.guest_ip, flow.dst_port, flow.guest_port, &buf[..n]);
                out.push(ethernet(&flow.guest_mac, &GATEWAY_MAC, 0x0800, &ip_packet(17, &flow.reply_src, &flow.guest_ip, &udp)));
            }
        }
        self.tcp.retain(|c| {
            let timeout = if matches!(c.transport, Transport::TimeWait) { TIME_WAIT_US } else { IDLE_CLOSE_US };
            !c.closed() && now_us.wrapping_sub(c.last_activity_us) < timeout
        });
        self.udp.retain(|f| now_us.wrapping_sub(f.last_activity_us) < IDLE_CLOSE_US);
        out
    }
}

fn host_resolver() -> [u8; 4] {
    if let Ok(conf) = std::fs::read_to_string("/etc/resolv.conf") {
        for line in conf.lines() {
            if let Some(rest) = line.trim().strip_prefix("nameserver ") {
                if let Ok(a) = rest.trim().parse::<Ipv4Addr>() { return a.octets(); }
            }
        }
    }
    [1, 1, 1, 1]
}

#[cfg(test)]
mod tests;
