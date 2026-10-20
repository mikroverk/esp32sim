//! Inbound port forwarding (`--hostfwd`): a host TCP listener whose connections are carried into
//! the guest's own stack over the emulated 802.11 link.
//!
//! The forward appears on the virtual subnet as one more host, [`PEER_IP`] with its own MAC, so
//! the guest sees an ordinary same-subnet client: `net.rs` answers ARP for that address, and every
//! forwarded connection uses one ephemeral port on the peer side, starting at 49152.
//!
//! The connection state machine is the NAT's, inverted rather than duplicated: `nat.rs`'s `Tcp`
//! already keeps guest bytes in `to_host` and host bytes in `unacked`, independent of which side
//! opened the connection. An inbound flow differs only in who owns the socket (the listener's
//! `accept` instead of a dial worker) and in the MAC frames are sourced from.
//!
//! Nothing runs on a thread of its own: [`HostFwd::poll`] accepts, reads and writes on the same
//! `NET_POLL_US` tick as the NAT, so a forwarded client changes the guest's execution only by
//! delivering frames through the normal receive path.

use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use crate::nat::{GuestWrite, Tcp, Transport, ACK, FIN, IDLE_CLOSE_US, MAX_FLOWS, MSS, RST, SYN, TIME_WAIT_US};
use crate::net::packet::{ethernet, ip_packet, transport_checksum};

/// The forwarding peer's address on the emulated subnet: the client address the guest sees.
pub const PEER_IP: [u8; 4] = [10, 0, 2, 4];
/// Its MAC, distinct from the gateway's and the station's.
pub const PEER_MAC: [u8; 6] = [0x02, 0x53, 0x49, 0x4d, 0x00, 0x04];
/// First peer-side ephemeral port; one per forwarded connection.
const PEER_PORT_FIRST: u16 = 49152;
/// A host connection waits this long for the station to associate before it is closed.
const WAIT_STATION_US: u64 = 30_000_000;
/// Host connections accepted per tick, so a connection flood cannot starve the guest.
const ACCEPT_BATCH: usize = 16;
/// The MSS option the peer advertises in its SYN: the largest segment it can receive.
const MSS_OPTION: [u8; 4] = [2, 4, (MSS >> 8) as u8, (MSS & 0xff) as u8];

/// One `--hostfwd` specification: a host listener mapped onto one guest port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    pub bind: SocketAddr,
    pub guest_port: u16,
}

impl Spec {
    /// Parse QEMU's `tcp:[hostaddr:]hostport-[:guestaddr:]guestport`.
    ///
    /// The bind address defaults to loopback; `0.0.0.0` is honoured only when written out. The
    /// guest side may name the station address explicitly, nothing else. Exactly one host port is
    /// forwarded: a range is not expanded.
    pub fn parse(text: &str, sta_ip: [u8; 4]) -> Result<Spec, String> {
        const FORM: &str = "expected tcp:[HOSTADDR:]HOSTPORT-[:GUESTADDR:]GUESTPORT, e.g. tcp:127.0.0.1:8080-:80";
        let Some((proto, rest)) = text.split_once(':') else {
            return Err(format!("--hostfwd {text:?}: {FORM}"));
        };
        if !proto.eq_ignore_ascii_case("tcp") {
            return Err(format!("--hostfwd {text:?}: only tcp is forwarded, not {proto:?}"));
        }
        let Some((host, guest)) = rest.split_once('-') else {
            return Err(format!("--hostfwd {text:?}: {FORM}"));
        };
        if guest.contains('-') {
            return Err(format!("--hostfwd {text:?}: one host port per specification, not a range"));
        }
        let (bind_addr, host_port) = match host.rsplit_once(':') {
            None => (Ipv4Addr::LOCALHOST, host),
            Some(("", _)) => {
                return Err(format!("--hostfwd {text:?}: give the bind address, e.g. tcp:127.0.0.1:8080-:80"));
            }
            Some((address, port)) => {
                let address = address.parse::<Ipv4Addr>()
                    .map_err(|_| format!("--hostfwd {text:?}: bind address {address:?} is not an IPv4 address"))?;
                (address, port)
            }
        };
        let bind_port = parse_port(host_port).ok_or_else(|| format!("--hostfwd {text:?}: host port {host_port:?} is not a port number"))?;
        if bind_port == 0 {
            return Err(format!("--hostfwd {text:?}: host port 0 would move on every run"));
        }
        let (guest_addr, guest_port) = match guest.rsplit_once(':') {
            None => (None, guest),
            Some(("", port)) => (None, port),
            Some((address, port)) => (Some(address), port),
        };
        if let Some(address) = guest_addr {
            let address = address.parse::<Ipv4Addr>()
                .map_err(|_| format!("--hostfwd {text:?}: guest address {address:?} is not an IPv4 address"))?;
            if address.octets() != sta_ip {
                return Err(format!(
                    "--hostfwd {text:?}: {address} is not the station; leave the guest address empty to mean {}.{}.{}.{}",
                    sta_ip[0], sta_ip[1], sta_ip[2], sta_ip[3]
                ));
            }
        }
        let guest_port = parse_port(guest_port).ok_or_else(|| format!("--hostfwd {text:?}: guest port {guest_port:?} is not a port number"))?;
        if guest_port == 0 {
            return Err(format!("--hostfwd {text:?}: guest port 0 is not a service"));
        }
        Ok(Spec { bind: SocketAddr::new(IpAddr::V4(bind_addr), bind_port), guest_port })
    }
}

fn parse_port(text: &str) -> Option<u16> { text.parse().ok() }

struct Listener { spec: Spec, sock: TcpListener }

/// A host connection with no station to address yet: the guest has not associated.
struct Waiting { sock: TcpStream, spec: Spec, since_us: u64 }

pub struct HostFwd {
    listeners: Vec<Listener>,
    waiting: Vec<Waiting>,
    flows: Vec<Tcp>,
    sta_mac: Option<[u8; 6]>,
    sta_ip: [u8; 4],
    isn: u32,
    next_port: u16,
    pub log: bool,
    /// Host connections accepted, and how many of them completed a handshake.
    pub accepted: u64, pub established: u64,
    /// Flows removed after closing or timing out, and the flows live now.
    pub closed: u64,
    /// Connections the guest refused (RST) and connections refused for want of a flow slot.
    pub refused: u64, pub capacity_refused: u64,
    /// Host connections closed because the station never appeared.
    pub abandoned: u64,
    /// Segments addressed to a peer port with no flow.
    pub no_flow: u64,
    pub bytes_to_guest: u64, pub bytes_to_host: u64,
}

impl HostFwd {
    /// Bind every listener now, so a busy or privileged port fails before the guest boots.
    pub fn new(specs: &[Spec], sta_ip: [u8; 4], log: bool) -> Result<Self, String> {
        let mut listeners = Vec::with_capacity(specs.len());
        for spec in specs {
            let sock = TcpListener::bind(spec.bind)
                .map_err(|e| format!("--hostfwd tcp:{}:{}: {e}", spec.bind.ip(), spec.bind.port()))?;
            sock.set_nonblocking(true)
                .map_err(|e| format!("--hostfwd tcp:{}:{}: {e}", spec.bind.ip(), spec.bind.port()))?;
            listeners.push(Listener { spec: *spec, sock });
        }
        Ok(Self { listeners, waiting: Vec::new(), flows: Vec::new(), sta_mac: None, sta_ip,
                  isn: 0x2000_0000, next_port: PEER_PORT_FIRST, log,
                  accepted: 0, established: 0, closed: 0, refused: 0, capacity_refused: 0,
                  abandoned: 0, no_flow: 0, bytes_to_guest: 0, bytes_to_host: 0 })
    }

    /// The addresses whose frames this forward owns: one ARP responder and one TCP demux arm.
    pub fn peer_ip(&self) -> Option<[u8; 4]> { (!self.listeners.is_empty()).then_some(PEER_IP) }

    /// The station's MAC, once it has associated; a flow cannot be framed before that.
    pub fn set_station_mac(&mut self, mac: [u8; 6]) {
        if mac != [0; 6] { self.sta_mac = Some(mac); }
    }

    /// Connections that still hold a host socket: live flows and those waiting for the station.
    pub fn active(&self) -> usize {
        self.flows.iter().filter(|f| !matches!(f.transport, Transport::TimeWait)).count() + self.waiting.len()
    }

    /// Flows kept only to re-acknowledge a lost final ACK. They hold no host socket.
    pub fn time_wait(&self) -> usize { self.flows.iter().filter(|f| matches!(f.transport, Transport::TimeWait)).count() }

    /// Drop every live flow, closing its host socket: the guest's stack was reset.
    pub fn reset_flows(&mut self) {
        let dropped = self.flows.len() as u64;
        self.flows.clear();
        self.closed += dropped;
        if dropped != 0 && self.log { eprintln!("[hostfwd] {dropped} flow(s) dropped by a guest reset"); }
    }

    fn allocate_port(&mut self) -> Option<u16> {
        for _ in 0..(u16::MAX - PEER_PORT_FIRST) {
            let port = self.next_port;
            self.next_port = if port == u16::MAX { PEER_PORT_FIRST } else { port + 1 };
            if !self.flows.iter().any(|f| f.dst_port == port) { return Some(port); }
        }
        None
    }

    /// Give one accepted host connection a flow and put its SYN on the air.
    fn open_flow(&mut self, sock: TcpStream, spec: Spec, now_us: u64) -> Vec<Vec<u8>> {
        let Some(mac) = self.sta_mac else { return Vec::new() };
        let Some(port) = self.allocate_port() else { self.capacity_refused += 1; return Vec::new() };
        self.isn = self.isn.wrapping_add(0x10000);
        let mut flow = Tcp {
            guest_mac: mac, guest_ip: self.sta_ip, guest_port: spec.guest_port,
            dst_ip: PEER_IP, dst_port: port, src_mac: PEER_MAC,
            transport: Transport::Connected(sock),
            guest_write: GuestWrite::Open, host_closed: false,
            our_seq: self.isn, guest_seq: 0, guest_window: 0,
            to_host: Default::default(), unacked: Default::default(), last_activity_us: now_us,
        };
        let frame = flow.send_with(SYN, Vec::new(), MSS_OPTION.to_vec(), now_us);
        if self.log {
            eprintln!("[hostfwd] {} -> guest {}:{} connecting (peer port {})",
                      self.listeners.iter().find(|l| l.spec == spec).map_or_else(String::new, |l| l.spec.bind.to_string()),
                      ip(&self.sta_ip), spec.guest_port, port);
        }
        self.flows.push(flow);
        vec![frame]
    }

    /// A segment from the guest addressed to the peer address. Returns frames for the guest.
    pub fn tcp_in(&mut self, sip: &[u8; 4], seg: &[u8], now_us: u64) -> Vec<Vec<u8>> {
        if seg.len() < 20 { return Vec::new(); }
        let off = ((seg[12] >> 4) as usize) * 4;
        if off < 20 || off > seg.len() { return Vec::new(); }
        let sport = u16::from_be_bytes([seg[0], seg[1]]);
        let dport = u16::from_be_bytes([seg[2], seg[3]]);
        let seq = u32::from_be_bytes(seg[4..8].try_into().unwrap());
        let ack = u32::from_be_bytes(seg[8..12].try_into().unwrap());
        let flags = seg[13];
        let window = u16::from_be_bytes([seg[14], seg[15]]);
        let data = &seg[off..];
        let Some(i) = self.flows.iter().position(|c| c.guest_ip == *sip && c.guest_port == sport && c.dst_port == dport) else {
            // Nothing is listening on that peer port: a stale segment after its flow went away,
            // or a probe. Refuse it instead of letting the guest retransmit into the void.
            if flags & RST != 0 { return Vec::new(); }
            self.no_flow += 1;
            let gmac = self.sta_mac.unwrap_or([0xff; 6]);
            return vec![reset(&gmac, sip, sport, dport, seq, ack, flags, data.len())];
        };
        let log = self.log;
        let c = &mut self.flows[i];
        c.last_activity_us = now_us;
        if flags & RST != 0 {
            self.refused += 1;
            if log { eprintln!("[hostfwd] guest {}:{} reset (peer port {})", ip(sip), sport, dport); }
            c.transport = Transport::Closed;
            return Vec::new();
        }
        if flags & SYN != 0 {
            if c.handshake_pending() && ack == c.our_seq {
                c.acknowledge(ack);
                c.guest_seq = seq.wrapping_add(1);
                c.guest_window = window;
                self.established += 1;
                if log { eprintln!("[hostfwd] guest {}:{} accepted (peer port {})", ip(sip), sport, dport); }
                return vec![c.segment(ACK, &[], c.our_seq)];
            }
            // A second SYN|ACK means our ACK was lost: repeat it, do not restart the handshake.
            return if ack == c.our_seq && seq.wrapping_add(1) == c.guest_seq {
                vec![c.segment(ACK, &[], c.our_seq)]
            } else { Vec::new() };
        }
        if !matches!(c.transport, Transport::Connected(_)) {
            // In TIME_WAIT only a repeated FIN still needs an answer.
            return if matches!(c.transport, Transport::TimeWait) && flags & FIN != 0 {
                vec![c.segment(ACK, &[], c.our_seq)]
            } else { Vec::new() };
        }
        if flags & ACK != 0 { c.acknowledge(ack); c.guest_window = window; }
        if c.handshake_pending() { return Vec::new(); }
        if !data.is_empty() || flags & FIN != 0 {
            c.accept(seq, data, flags & FIN != 0);
            return vec![c.segment(ACK, &[], c.our_seq)];
        }
        Vec::new()
    }

    /// Accept, pump and expire: everything hostfwd does that costs host syscalls, once per tick.
    pub fn poll(&mut self, now_us: u64) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut accepted: Vec<(Spec, TcpStream)> = Vec::new();
        for listener in &self.listeners {
            for _ in 0..ACCEPT_BATCH {
                match listener.sock.accept() {
                    Ok((sock, _)) => accepted.push((listener.spec, sock)),
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => break,   // WouldBlock: nothing more is queued
                }
            }
        }
        for (spec, sock) in accepted {
            self.accepted += 1;
            if sock.set_nonblocking(true).is_err() || sock.set_nodelay(true).is_err() { continue; }
            if self.flows.len() + self.waiting.len() >= MAX_FLOWS {
                self.capacity_refused += 1;
                if self.log { eprintln!("[hostfwd] no flow slot left, closing the connection from the host"); }
                continue;   // dropping the socket closes it: the client fails now, not later
            }
            if self.sta_mac.is_some() {
                out.extend(self.open_flow(sock, spec, now_us));
            } else {
                if self.log { eprintln!("[hostfwd] {}: host connection waiting for the station", spec.bind); }
                self.waiting.push(Waiting { sock, spec, since_us: now_us });
            }
        }
        if self.sta_mac.is_some() && !self.waiting.is_empty() {
            for w in std::mem::take(&mut self.waiting) { out.extend(self.open_flow(w.sock, w.spec, now_us)); }
        }
        for i in 0..self.flows.len() {
            let (frames, to_host, to_guest) = self.flows[i].pump(now_us, self.log.then_some("hostfwd"));
            self.bytes_to_host += to_host as u64;
            self.bytes_to_guest += to_guest as u64;
            out.extend(frames);
            if let Some(frame) = self.flows[i].retransmit(now_us) { out.push(frame); }
        }
        let mut removed = 0u64;
        self.flows.retain(|c| {
            let timeout = if matches!(c.transport, Transport::TimeWait) { TIME_WAIT_US } else { IDLE_CLOSE_US };
            let live = !c.closed() && now_us.wrapping_sub(c.last_activity_us) < timeout;
            if !live { removed += 1; }
            live
        });
        self.closed += removed;
        let waiting = self.waiting.len();
        self.waiting.retain(|w| now_us.wrapping_sub(w.since_us) < WAIT_STATION_US);
        let dropped = (waiting - self.waiting.len()) as u64;
        if dropped != 0 && self.log {
            eprintln!("[hostfwd] closed {dropped} host connection(s) that waited for a station");
        }
        self.abandoned += dropped;
        out
    }

    /// The end-of-run accounting line, empty when no forward was configured.
    pub fn report(&self) -> Option<String> {
        if self.listeners.is_empty() { return None; }
        Some(format!(
            "[emu] hostfwd: {} host connections ({} established, {} refused by the guest, {} refused for want of a slot, {} abandoned), {} closed, {} active, {} in TIME_WAIT, {} segments without a flow, {} bytes in, {} bytes out",
            self.accepted, self.established, self.refused, self.capacity_refused, self.abandoned,
            self.closed, self.active(), self.time_wait(), self.no_flow, self.bytes_to_guest, self.bytes_to_host))
    }
}

fn ip(a: &[u8; 4]) -> Ipv4Addr { Ipv4Addr::from(*a) }

/// A RST for a segment the peer has no flow for, built the way RFC 793 asks: answer an ACK with
/// RST/seq = incoming ack, otherwise RST|ACK/ack = incoming seq plus what the segment occupied.
#[allow(clippy::too_many_arguments, reason = "the segment's fields stay explicit at the protocol boundary")]
fn reset(gmac: &[u8; 6], sip: &[u8; 4], sport: u16, dport: u16, seq: u32, ack: u32, flags: u8, data_len: usize) -> Vec<u8> {
    let occupied = data_len as u32 + u32::from(flags & SYN != 0) + u32::from(flags & FIN != 0);
    let (rst_seq, rst_ack, rst_flags) = if flags & ACK != 0 { (ack, 0, RST) } else { (0, seq.wrapping_add(occupied), RST | ACK) };
    let mut seg = Vec::with_capacity(20);
    seg.extend_from_slice(&dport.to_be_bytes());
    seg.extend_from_slice(&sport.to_be_bytes());
    seg.extend_from_slice(&rst_seq.to_be_bytes());
    seg.extend_from_slice(&rst_ack.to_be_bytes());
    seg.extend_from_slice(&[0x50, rst_flags, 0, 0, 0, 0, 0, 0]);
    let check = transport_checksum(&PEER_IP, sip, 6, &seg).to_be_bytes();
    seg[16..18].copy_from_slice(&check);
    ethernet(gmac, &PEER_MAC, 0x0800, &ip_packet(6, &PEER_IP, sip, &seg))
}

#[cfg(test)]
mod tests;
