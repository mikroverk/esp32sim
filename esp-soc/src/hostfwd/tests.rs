use super::*;
use crate::nat::{RETRANSMIT_US, WINDOW, PSH};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::time::{Duration, Instant};

const GUEST: [u8; 4] = [10, 0, 2, 15];
const GUEST_MAC: [u8; 6] = [0x44; 6];

fn forward() -> (HostFwd, u16) {
    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let spec = Spec { bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port), guest_port: 80 };
    let mut fwd = HostFwd::new(&[spec], GUEST, false).unwrap();
    fwd.set_station_mac([0x44; 6]);
    (fwd, port)
}

fn client(port: u16) -> TcpStream {
    let sock = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    sock
}

fn segment(sport: u16, dport: u16, seq: u32, ack: u32, flags: u8, data: &[u8]) -> Vec<u8> {
    let mut seg = Vec::with_capacity(20 + data.len());
    seg.extend_from_slice(&sport.to_be_bytes());
    seg.extend_from_slice(&dport.to_be_bytes());
    seg.extend_from_slice(&seq.to_be_bytes());
    seg.extend_from_slice(&ack.to_be_bytes());
    seg.extend_from_slice(&[0x50, flags]);
    seg.extend_from_slice(&WINDOW.to_be_bytes());
    seg.extend_from_slice(&[0; 4]);
    seg.extend_from_slice(data);
    seg
}

/// (source port, destination port, seq, ack, flags, payload, options length)
fn fields(frame: &[u8]) -> (u16, u16, u32, u32, u8, Vec<u8>, usize) {
    let t = &frame[34..];
    let options = ((t[12] >> 4) as usize) * 4 - 20;
    (u16::from_be_bytes([t[0], t[1]]), u16::from_be_bytes([t[2], t[3]]),
     u32::from_be_bytes(t[4..8].try_into().unwrap()), u32::from_be_bytes(t[8..12].try_into().unwrap()),
     t[13], t[20 + options..].to_vec(), options)
}

// Host socket delivery is asynchronous only in the kernel; the emulated clock does not move.
fn poll_ready(fwd: &mut HostFwd, now_us: u64) -> Vec<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let frames = fwd.poll(now_us);
        if !frames.is_empty() { return frames; }
        assert!(Instant::now() < deadline, "host socket delivery timed out");
        std::thread::yield_now();
    }
}

#[test]
fn spec_parsing_takes_the_qemu_form_and_rejects_malformed_ones() {
    let sta = GUEST;
    let spec = Spec::parse("tcp:127.0.0.1:8080-:80", sta).unwrap();
    assert_eq!(spec.bind, SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8080));
    assert_eq!(spec.guest_port, 80);
    // A bare host port binds loopback; the guest side may repeat the station address.
    assert_eq!(Spec::parse("tcp:8080-80", sta).unwrap(), spec);
    assert_eq!(Spec::parse("tcp:8080-10.0.2.15:80", sta).unwrap(), spec);
    assert_eq!(Spec::parse("tcp:0.0.0.0:8080-:80", sta).unwrap().bind.ip(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    for bad in ["udp:127.0.0.1:8080-:80", "tcp:127.0.0.1:8080", "tcp::8080-:80", "tcp:localhost:8080-:80",
                "tcp:127.0.0.1:0-:80", "tcp:127.0.0.1:8080-:0", "tcp:127.0.0.1:8080-10.0.2.16:80",
                "tcp:127.0.0.1:8080-10.0.2.15:80-90", "8080-80", "tcp:127.0.0.1:http-:80"] {
        assert!(Spec::parse(bad, sta).is_err(), "{bad} should be rejected");
    }
    // The range QEMU spells as host-guest is one mapping here, not an expanding range.
    assert_eq!(Spec::parse("tcp:127.0.0.1:8080-8090", sta).unwrap().guest_port, 8090);
}

#[test]
fn listener_binds_at_construction_and_fails_loudly() {
    let (fwd, port) = forward();
    let again = HostFwd::new(&[Spec { bind: fwd.listeners[0].spec.bind, guest_port: 80 }], GUEST, false);
    match again {
        Ok(_) => panic!("binding a port twice must fail"),
        Err(err) => assert!(err.contains(&port.to_string()), "{err}"),
    }
}

#[test]
fn syn_carries_the_mss_option_and_is_retransmitted_unchanged() {
    let (mut fwd, port) = forward();
    let _client = client(port);
    let frames = fwd.poll(0);
    assert_eq!(frames.len(), 1);
    assert_eq!(&frames[0][6..12], &PEER_MAC, "the peer sources the frame");
    let (sport, dport, seq, ack, flags, payload, options) = fields(&frames[0]);
    assert_eq!((dport, flags, ack, payload.len()), (80, SYN, 0, 0));
    assert!(sport >= PEER_PORT_FIRST);
    assert_eq!(options, 4);
    assert_eq!(&frames[0][54..58], &MSS_OPTION);
    assert_eq!(crate::net::packet::transport_checksum(&PEER_IP, &GUEST, 6, &frames[0][34..]), 0);
    assert_eq!(fwd.poll(RETRANSMIT_US - 1), Vec::<Vec<u8>>::new());
    let resent = fwd.poll(RETRANSMIT_US);
    assert_eq!(resent, frames, "the retransmitted SYN keeps its sequence and option");
    assert_eq!(u32::from_be_bytes(frames[0][38..42].try_into().unwrap()), seq);
    assert_eq!(fwd.established, 0);
}

#[test]
fn handshake_data_and_half_close_reach_both_sides() {
    let (mut fwd, port) = forward();
    let mut host = client(port);
    let syn = fwd.poll(0);
    let (peer_port, _, isn, _, _, _, _) = fields(&syn[0]);

    let reply = fwd.tcp_in(&GUEST_MAC, &GUEST, &segment(80, peer_port, 1000, isn + 1, SYN | ACK, &[]), 1);
    assert_eq!(reply.len(), 1);
    let (sport, dport, seq, ack, flags, _, _) = fields(&reply[0]);
    assert_eq!((sport, dport, seq, ack, flags), (peer_port, 80, isn + 1, 1001, ACK));
    assert_eq!((fwd.established, fwd.active()), (1, 1));

    host.write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
    let frames = poll_ready(&mut fwd, 2);
    assert_eq!(frames.len(), 1);
    let (_, _, seq, ack, flags, payload, _) = fields(&frames[0]);
    assert_eq!(flags, PSH | ACK);
    assert_eq!((seq, ack), (isn + 1, 1001));
    assert_eq!(payload, b"GET / HTTP/1.0\r\n\r\n");
    fwd.tcp_in(&GUEST_MAC, &GUEST, &segment(80, peer_port, 1001, isn + 19, ACK, &[]), 3);

    let body = b"HTTP/1.0 200 OK\r\n\r\nhello";
    let reply = fwd.tcp_in(&GUEST_MAC, &GUEST, &segment(80, peer_port, 1001, isn + 19, PSH | ACK, body), 4);
    assert_eq!(reply.len(), 1);
    assert_eq!(fields(&reply[0]).4, ACK);
    fwd.poll(5);
    let mut received = [0u8; 32];
    let n = host.read(&mut received).unwrap();
    assert_eq!(&received[..n], body);
    assert_eq!(fwd.bytes_to_host, body.len() as u64);

    // The client half-closes: the guest sees a FIN and answers with its own.
    host.shutdown(Shutdown::Write).unwrap();
    let frames = poll_ready(&mut fwd, 6);
    let (_, _, seq, _, flags, _, _) = fields(&frames[0]);
    assert_eq!(flags, FIN | ACK);
    let fin_seq = seq;
    fwd.tcp_in(&GUEST_MAC, &GUEST, &segment(80, peer_port, 1001 + body.len() as u32, fin_seq + 1, ACK | FIN, &[]), 7);
    let replay = fwd.tcp_in(&GUEST_MAC, &GUEST, &segment(80, peer_port, 1001 + body.len() as u32, fin_seq + 1, ACK | FIN, &[]), 8);
    assert_eq!(fields(&replay[0]).4, ACK, "a repeated FIN is still acknowledged");
    fwd.poll(9);
    assert_eq!(host.read(&mut received).unwrap(), 0, "the guest's FIN closes the host's read side");
    fwd.poll(10);
    assert!(matches!(fwd.flows[0].transport, Transport::TimeWait));
    fwd.poll(10 + TIME_WAIT_US);
    assert!(fwd.flows.is_empty());
    assert_eq!((fwd.closed, fwd.active()), (1, 0));
}

#[test]
fn guest_reset_closes_the_host_connection_and_is_counted() {
    let (mut fwd, port) = forward();
    let mut host = client(port);
    let syn = fwd.poll(0);
    let (peer_port, _, isn, _, _, _, _) = fields(&syn[0]);
    fwd.tcp_in(&GUEST_MAC, &GUEST, &segment(80, peer_port, 1000, isn + 1, SYN | ACK, &[]), 1);
    assert_eq!(fwd.tcp_in(&GUEST_MAC, &GUEST, &segment(80, peer_port, 1001, isn + 1, RST | ACK, &[]), 2), Vec::<Vec<u8>>::new());
    assert_eq!(fwd.refused, 1);
    fwd.poll(3);
    assert!(fwd.flows.is_empty());
    assert_eq!(fwd.closed, 1);
    let mut buf = [0u8; 8];
    assert!(matches!(host.read(&mut buf), Ok(0) | Err(_)), "the client sees the connection end");
    assert!(fwd.report().unwrap().contains("1 refused by the guest"));
}

#[test]
fn a_segment_for_an_unused_peer_port_is_refused_instead_of_ignored() {
    let (mut fwd, _port) = forward();
    let frames = fwd.tcp_in(&GUEST_MAC, &GUEST, &segment(80, PEER_PORT_FIRST, 500, 0, SYN, &[]), 0);
    assert_eq!(frames.len(), 1);
    let (sport, dport, seq, ack, flags, _, _) = fields(&frames[0]);
    assert_eq!((sport, dport, seq, ack, flags), (PEER_PORT_FIRST, 80, 0, 501, RST | ACK));
    assert_eq!(&frames[0][6..12], &PEER_MAC);
    assert_eq!(&frames[0][0..6], &GUEST_MAC, "the refusal goes to the station that sent the segment");
    assert_eq!(crate::net::packet::transport_checksum(&PEER_IP, &GUEST, 6, &frames[0][34..]), 0);
    assert_eq!(fwd.no_flow, 1);
    // A RST for an unknown flow is not answered with another RST.
    assert!(fwd.tcp_in(&GUEST_MAC, &GUEST, &segment(80, PEER_PORT_FIRST, 501, 0, RST, &[]), 1).is_empty());
    assert_eq!(fwd.no_flow, 1);
}

#[test]
fn a_host_connection_waits_for_the_station_then_opens() {
    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let spec = Spec { bind: SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port), guest_port: 80 };
    let mut fwd = HostFwd::new(&[spec], GUEST, false).unwrap();
    fwd.set_station_mac([0; 6]);   // not associated yet: the zero MAC is ignored
    let _host = client(port);
    assert!(fwd.poll(0).is_empty(), "no SYN before the station is known");
    assert_eq!((fwd.accepted, fwd.active()), (1, 1));
    fwd.set_station_mac([0x44; 6]);
    let frames = fwd.poll(WAIT_STATION_US / 2);
    assert_eq!(frames.len(), 1);
    assert_eq!(fields(&frames[0]).4, SYN);
    assert_eq!(&frames[0][0..6], &[0x44; 6]);
}

#[test]
fn a_connection_that_never_finds_a_station_is_closed() {
    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let spec = Spec { bind: SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port), guest_port: 80 };
    let mut fwd = HostFwd::new(&[spec], GUEST, false).unwrap();
    let mut host = client(port);
    assert!(fwd.poll(0).is_empty());
    assert_eq!(fwd.active(), 1);
    assert!(fwd.poll(WAIT_STATION_US).is_empty());
    assert_eq!((fwd.active(), fwd.abandoned), (0, 1));
    let mut buf = [0u8; 8];
    assert!(matches!(host.read(&mut buf), Ok(0) | Err(_)), "the waiting client is released");
}

#[test]
fn flows_are_capped_and_the_next_connection_is_refused_immediately() {
    let (mut fwd, port) = forward();
    let mut hosts: Vec<TcpStream> = Vec::new();
    for _ in 0..MAX_FLOWS { hosts.push(client(port)); }
    let mut frames = Vec::new();
    for tick in 0..MAX_FLOWS as u64 { frames.extend(fwd.poll(tick)); }
    assert_eq!(fwd.flows.len(), MAX_FLOWS);
    assert_eq!(frames.len(), MAX_FLOWS, "one SYN per accepted connection");
    let extra = client(port);
    fwd.poll(MAX_FLOWS as u64 + 1);
    assert_eq!(fwd.capacity_refused, 1);
    assert_eq!(fwd.flows.len(), MAX_FLOWS);
    let mut buf = [0u8; 8];
    let mut extra = extra;
    assert!(matches!(extra.read(&mut buf), Ok(0) | Err(_)), "the refused client is not left hanging");
    drop(hosts);
}

#[test]
fn a_guest_reset_drops_every_flow() {
    let (mut fwd, port) = forward();
    let mut hosts: Vec<TcpStream> = Vec::new();
    for _ in 0..3 { hosts.push(client(port)); }
    let mut frames = Vec::new();
    for tick in 0..3 { frames.extend(fwd.poll(tick)); }
    assert_eq!(fwd.active(), 3);
    fwd.reset_flows();
    assert_eq!((fwd.active(), fwd.closed), (0, 3));
    assert!(fwd.poll(10).is_empty());
    drop(hosts);
}
