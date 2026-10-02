//! A packet-level BLE controller and one virtual peer, independent of the guest CPU.
pub mod vhci;

use std::collections::VecDeque;

const HANDLE: u16 = 1;
const ADDRESS: [u8; 6] = [1, 0, 0, 0x49, 0x53, 2];
const PEER: [u8; 6] = [2, 0, 0, 0x49, 0x53, 2];
const ADV: &[u8] = b"\x02\x01\x06\x03\x03\x0f\x18\x09\x09esp32sim";

fn word(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect()
}
fn uuid(b: &[u8]) -> String {
    let s = hex(&b.iter().rev().copied().collect::<Vec<_>>());
    if b.len() == 16 {
        format!(
            "{}-{}-{}-{}-{}",
            &s[..8],
            &s[8..12],
            &s[12..16],
            &s[16..20],
            &s[20..]
        )
    } else {
        s
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Request {
    Services,
    Characteristics,
    Descriptors,
    Read(u16),
    Write(u16),
}

/// No RF, encryption sessions or pairing. One ACL link is enough for the Arduino examples.
/// `send` and `pop_packet` exchange complete H4 packets including the packet-type byte.
pub struct Controller {
    packets: VecDeque<Vec<u8>>,
    log: Vec<String>,
    advertising: bool,
    adv: Vec<u8>,
    scan_response: Vec<u8>,
    connected: bool,
    guest_central: bool,
    active_scan: bool,
    fragments: Vec<u8>,
    pending: Option<Request>,
    discovery_start: u16,
    random: u64,
    value: Vec<u8>,
    local_name: [u8; 248],
    default_link_policy: u16,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}
impl Controller {
    pub fn new() -> Self {
        Self {
            packets: VecDeque::new(),
            log: Vec::new(),
            advertising: false,
            adv: Vec::new(),
            scan_response: Vec::new(),
            connected: false,
            guest_central: false,
            active_scan: false,
            fragments: Vec::new(),
            pending: None,
            discovery_start: 1,
            random: 0x3253_5045_424c_4531,
            value: vec![100],
            local_name: [0; 248],
            default_link_policy: 0,
        }
    }
    pub fn pop_packet(&mut self) -> Option<Vec<u8>> {
        self.packets.pop_front()
    }
    pub fn drain_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }
    pub fn is_advertising(&self) -> bool {
        self.advertising
    }
    fn event(&mut self, event: u8, data: &[u8]) {
        let mut p = vec![4, event, data.len() as u8];
        p.extend_from_slice(data);
        self.packets.push_back(p);
    }
    fn complete(&mut self, op: u16, response: &[u8]) {
        let mut p = vec![1, op as u8, (op >> 8) as u8];
        p.extend_from_slice(response);
        self.event(0x0e, &p);
    }
    fn status(&mut self, op: u16, status: u8) {
        self.event(0x0f, &[status, 1, op as u8, (op >> 8) as u8]);
    }
    fn att(&mut self, pdu: &[u8]) {
        self.l2cap(4, pdu);
    }
    fn l2cap(&mut self, cid: u16, pdu: &[u8]) {
        let len = pdu.len() as u16;
        let mut p = vec![2, HANDLE as u8, 0x20];
        p.extend_from_slice(&(len + 4).to_le_bytes());
        p.extend_from_slice(&len.to_le_bytes());
        p.extend_from_slice(&cid.to_le_bytes());
        p.extend_from_slice(pdu);
        self.packets.push_back(p);
    }
    fn connect(&mut self, guest_central: bool) {
        self.connected = true;
        self.guest_central = guest_central;
        self.advertising = false;
        let mut e = vec![1, 0, HANDLE as u8, 0, if guest_central { 0 } else { 1 }, 0];
        e.extend_from_slice(&PEER);
        e.extend_from_slice(&[24, 0, 0, 0, 0xf4, 1, 0]);
        self.event(0x3e, &e);
        self.log.push(format!(
            "connected handle=0x{HANDLE:04x} guest={}",
            if guest_central {
                "central"
            } else {
                "peripheral"
            }
        ));
    }
    fn advertisement(&mut self) {
        let mut fields = Vec::new();
        for source in [&self.adv, &self.scan_response] {
            let mut data = source.as_slice();
            while !data.is_empty() {
                let len = data[0] as usize;
                if len == 0 || len + 1 > data.len() {
                    break;
                }
                let v = &data[2..len + 1];
                match data[1] {
                    8 | 9 => fields.push(format!("name={:?}", String::from_utf8_lossy(v))),
                    2 | 3 => {
                        for u in v.as_chunks::<2>().0 {
                            fields.push(format!("service={}", uuid(u)));
                        }
                    }
                    6 | 7 => {
                        for u in v.as_chunks::<16>().0 {
                            fields.push(format!("service={}", uuid(u)));
                        }
                    }
                    _ => {}
                }
                data = &data[len + 1..];
            }
        }
        self.log.push(format!(
            "advertising {} data={}",
            fields.join(" "),
            hex(&self.adv)
        ));
    }
    pub fn send(&mut self, packet: &[u8]) {
        match packet.first() {
            Some(1) if packet.len() >= 4 => {
                let op = word(&packet[1..]);
                if packet.len() != 4 + packet[3] as usize {
                    self.complete(op, &[0x12]);
                    return;
                }
                self.hci(op, &packet[4..]);
            }
            Some(2) if packet.len() >= 5 => self.acl(packet),
            _ => self.log.push(format!("invalid H4 packet {}", hex(packet))),
        }
    }
    fn hci(&mut self, op: u16, p: &[u8]) {
        let expected = match op {
            0x0c03 | 0x0c14 | 0x1001 | 0x1002 | 0x1003 | 0x1005 | 0x1009 | 0x2002 | 0x2003
            | 0x2007 | 0x200e | 0x200f | 0x2010 | 0x2018 | 0x201c | 0xfd0c => Some(0),
            0x1004 | 0x0c1a | 0x0c2f | 0x0c45 | 0x0c56 | 0x0c5b | 0x0c7a | 0x200a | 0x0c31
            | 0xfd12 | 0xfd82 => Some(1),
            0x080f | 0x0c16 | 0x0c18 | 0x0c6d | 0x200c | 0x2015 | 0x2016 | 0x1405 | 0x041d
            | 0xfd0a => Some(2),
            0x0406 | 0x0c24 | 0xfc82 => Some(3),
            0xfd16 => Some(4),
            0x0c01 | 0x0c63 | 0x2001 => Some(8),
            0x2005 => Some(6),
            0x2006 => Some(15),
            0x2008 | 0x2009 | 0x2017 => Some(32),
            0x200b | 0x0c33 | 0x2011 | 0x2012 => Some(7),
            0x200d => Some(25),
            0x2013 => Some(14),
            0x0c13 => Some(248),
            0xfd09 | 0x2014 => Some(5),
            0x0c35 => p.first().map(|n| 1 + 4 * *n as usize),
            _ => None,
        };
        if expected.is_some_and(|n| n != p.len()) || (op == 0x0c35 && p.is_empty()) {
            self.complete(op, &[0x12]);
            return;
        }
        match op {
            0x0c03 => {
                self.advertising = false;
                self.connected = false;
                self.pending = None;
                self.fragments.clear();
                self.adv.clear();
                self.scan_response.clear();
                self.active_scan = false;
                self.packets.clear();
                self.complete(op, &[0]);
            }
            0x080f => {
                self.default_link_policy = word(p);
                self.complete(op, &[0]);
            }
            0x0c13 => {
                self.local_name.copy_from_slice(p);
                self.complete(op, &[0]);
            }
            0x0c14 => {
                let mut response = vec![0];
                response.extend_from_slice(&self.local_name);
                self.complete(op, &response);
            }
            0x1001 => self.complete(op, &[0, 8, 0, 0, 8, 0xff, 0xff, 0, 0]),
            0x1002 => {
                let mut commands = [0u8; 65];
                // Bluetooth Core Vol 4, Part E, 6.27. One status byte precedes the bitfield.
                for (octet, bits) in [
                    (0, 0x20),
                    (2, 0x80),
                    (5, 0xd0),
                    (7, 0xab),
                    (10, 0xf0),
                    (14, 0xf8),
                    (15, 0x22),
                    (18, 8),
                    (22, 4),
                    (25, 0xf7),
                    (26, 0xff),
                    (27, 0xff),
                    (28, 8),
                ] {
                    commands[octet + 1] = bits;
                }
                self.complete(op, &commands);
            }
            0x1003 => self.complete(op, &[0, 0, 0, 0, 0, 0x40, 0, 0, 0x80]),
            0x1004 => {
                if p[0] > 1 {
                    self.complete(op, &[0x12]);
                    return;
                }
                let mut out = vec![0, p[0], 1];
                out.extend_from_slice(if p[0] == 0 {
                    &[0, 0, 0, 0, 0x40, 0, 0, 0x80]
                } else {
                    &[2, 0, 0, 0, 0, 0, 0, 0]
                });
                self.complete(op, &out);
            }
            0x1005 => self.complete(op, &[0, 0xfb, 0, 0, 8, 0, 0, 0]),
            0x1009 => {
                let mut v = vec![0];
                v.extend_from_slice(&ADDRESS);
                self.complete(op, &v);
            }
            0x2002 => self.complete(op, &[0, 0xfb, 0, 8]),
            0x2003 => self.complete(op, &[0; 9]),
            0x2007 => self.complete(op, &[0, 0]),
            0x200f => self.complete(op, &[0, 8]),
            0x201c => self.complete(op, &[0, 0xff, 0x07, 0, 0, 0, 0, 0, 0]),
            0x2008 | 0x2009 => {
                if p[0] > 31 {
                    self.complete(op, &[0x12]);
                    return;
                }
                let data = p[1..1 + p[0] as usize].to_vec();
                if op == 0x2008 {
                    self.adv = data;
                } else {
                    self.scan_response = data;
                }
                self.complete(op, &[0]);
                if self.advertising {
                    self.advertisement();
                }
            }
            0x200a => {
                if p[0] > 1 {
                    self.complete(op, &[0x12]);
                    return;
                }
                self.advertising = p[0] == 1;
                self.complete(op, &[0]);
                if self.advertising {
                    self.advertisement();
                }
            }
            0x200b => {
                if p[0] > 1 {
                    self.complete(op, &[0x12]);
                    return;
                }
                self.active_scan = p[0] == 1;
                self.complete(op, &[0]);
            }
            0x200c => {
                if p[0] > 1 || p[1] > 1 {
                    self.complete(op, &[0x12]);
                    return;
                }
                self.complete(op, &[0]);
                if p[0] == 1 {
                    let mut e = vec![2, 1, 0, 0];
                    e.extend_from_slice(&PEER);
                    e.push(ADV.len() as u8);
                    e.extend_from_slice(ADV);
                    e.push((-35i8) as u8);
                    self.event(0x3e, &e);
                    if self.active_scan {
                        e[2] = 4;
                        self.event(0x3e, &e);
                    }
                    self.log
                        .push("scan report name=esp32sim service=180f".into());
                }
            }
            0x200d => {
                if p[4] != 0 || p[5] != 0 || p[6..12] != PEER {
                    self.status(op, 0x12);
                    return;
                }
                self.status(op, if self.connected { 0x0c } else { 0 });
                if !self.connected {
                    self.connect(true);
                }
            }
            0x0406 => {
                if !self.connected || word(p) != HANDLE {
                    self.status(op, 2);
                    return;
                }
                self.status(op, 0);
                self.event(5, &[0, HANDLE as u8, 0, p[2]]);
                self.connected = false;
                self.pending = None;
                self.fragments.clear();
                self.log.push("disconnected".into());
            }
            0x2015 => self.complete(op, &[0, p[0], p[1], 0xff, 0xff, 0xff, 0xff, 0x1f]),
            0x2016 => {
                self.status(op, 0);
                self.event(0x3e, &[4, 0, HANDLE as u8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            }
            0x041d => {
                self.status(op, 0);
                self.event(0x0c, &[0, HANDLE as u8, 0, 8, 0xff, 0xff, 0, 0]);
            }
            0x2013 => {
                self.status(op, 0);
                let mut e = vec![3, 0, HANDLE as u8, 0];
                e.extend_from_slice(&p[2..4]);
                e.extend_from_slice(&p[6..10]);
                self.event(0x3e, &e);
            }
            0x2017 => {
                let mut key: [u8; 16] = p[..16].try_into().unwrap();
                key.reverse();
                let mut data: [u8; 16] = p[16..].try_into().unwrap();
                data.reverse();
                let mut encrypted = esp_periph::crypto::aes128_encrypt_block(&key, &data);
                encrypted.reverse();
                let mut response = vec![0];
                response.extend_from_slice(&encrypted);
                self.complete(op, &response);
            }
            0x2018 => {
                // ponytail: repeatable bytes for unpaired fixtures, never a cryptographic entropy source.
                self.random ^= self.random << 13;
                self.random ^= self.random >> 7;
                self.random ^= self.random << 17;
                let mut response = vec![0];
                response.extend_from_slice(&self.random.to_le_bytes());
                self.complete(op, &response);
            }
            0x1405 => self.complete(op, &[0, p[0], p[1], (-35i8) as u8]),
            0x0c35 => {} // Host Number Of Completed Packets has no completion event.
            0x0c01 | 0x0c16 | 0x0c18 | 0x0c1a | 0x0c24 | 0x0c2f | 0x0c31 | 0x0c33 | 0x0c45
            | 0x0c56 | 0x0c5b | 0x0c63 | 0x0c6d | 0x0c7a | 0x2001 | 0x2005 | 0x2006 | 0x200e
            | 0x2010 | 0x2011 | 0x2012 | 0x2014 | 0xfc82 | 0xfd09 | 0xfd0a | 0xfd0c | 0xfd12
            | 0xfd16 | 0xfd82 => self.complete(op, &[0]),
            _ => {
                self.log.push(format!("unsupported HCI opcode=0x{op:04x}"));
                self.complete(op, &[1]);
            }
        }
    }
    fn acl(&mut self, packet: &[u8]) {
        let header = word(&packet[1..]);
        if packet.len() != 5 + word(&packet[3..]) as usize
            || header & 0xfff != HANDLE
            || !self.connected
        {
            self.log
                .push("invalid ACL length or connection handle".into());
            return;
        }
        let boundary = (header >> 12) & 3;
        if boundary == 0 || boundary == 2 {
            self.fragments.clear();
        } else if boundary != 1 || self.fragments.is_empty() {
            self.log.push("invalid ACL continuation".into());
            return;
        }
        self.fragments.extend_from_slice(&packet[5..]);
        self.event(0x13, &[1, HANDLE as u8, 0, 1, 0]);
        if self.fragments.len() < 4 {
            return;
        }
        let len = word(&self.fragments) as usize;
        if len > 517 || self.fragments.len() > len + 4 {
            self.fragments.clear();
            self.log.push("invalid L2CAP length".into());
            return;
        }
        if self.fragments.len() < len + 4 {
            return;
        }
        let data = std::mem::take(&mut self.fragments);
        match word(&data[2..]) {
            4 => self.receive_att(&data[4..]),
            5 if data.len() >= 8 && data[4] == 0x12 => self.l2cap(5, &[0x13, data[5], 2, 0, 0, 0]),
            6 => self.l2cap(6, &[5, 5]), // Pairing Failed: Pairing Not Supported.
            _ => self
                .log
                .push(format!("unsupported L2CAP cid=0x{:04x}", word(&data[2..]))),
        }
    }
    /// Commands: connect, discover, read HANDLE, write HANDLE HEX, subscribe CCC_HANDLE.
    /// Handles accept decimal or 0x-prefixed hexadecimal. Only one ATT request may be outstanding.
    pub fn command(&mut self, command: &str) -> Result<(), String> {
        let parts: Vec<_> = command.split_whitespace().collect();
        if parts.as_slice() == ["connect"] {
            if !self.advertising || self.connected {
                return Err("BLE connect requires an advertising guest".into());
            }
            self.connect(false);
            return Ok(());
        }
        if !self.connected || self.guest_central {
            return Err("BLE command requires a virtual-central connection".into());
        }
        if self.pending.is_some() {
            return Err("BLE ATT request is still pending".into());
        }
        if parts.as_slice() == ["discover"] {
            self.discover(Request::Services, 1);
            return Ok(());
        }
        if parts.len() < 2 {
            return Err(
                "expected connect, discover, read HANDLE, write HANDLE HEX, subscribe CCC_HANDLE"
                    .into(),
            );
        }
        let handle = if let Some(h) = parts[1].strip_prefix("0x") {
            u16::from_str_radix(h, 16)
        } else {
            parts[1].parse()
        }
        .ok()
        .filter(|h| *h != 0)
        .ok_or("invalid BLE attribute handle")?;
        let mut pdu = vec![0, handle as u8, (handle >> 8) as u8];
        match (parts[0], parts.len()) {
            ("read", 2) => {
                pdu[0] = 0x0a;
                self.pending = Some(Request::Read(handle));
            }
            ("subscribe", 2) => {
                pdu[0] = 0x12;
                pdu.extend_from_slice(&[1, 0]);
                self.pending = Some(Request::Write(handle));
            }
            ("write", 3) => {
                let value = parts[2];
                if value.len() % 2 != 0
                    || !value.bytes().all(|b| b.is_ascii_hexdigit())
                    || value.len() > 40
                {
                    return Err("BLE write requires at most 20 bytes of hexadecimal data".into());
                }
                pdu[0] = 0x12;
                for i in (0..value.len()).step_by(2) {
                    pdu.push(u8::from_str_radix(&value[i..i + 2], 16).unwrap());
                }
                self.pending = Some(Request::Write(handle));
            }
            _ => return Err("unknown BLE command or argument count".into()),
        }
        self.att(&pdu);
        Ok(())
    }
    fn discover(&mut self, kind: Request, start: u16) {
        self.pending = Some(kind);
        self.discovery_start = start;
        let mut p = vec![
            match kind {
                Request::Services => 0x10,
                Request::Characteristics => 8,
                _ => 4,
            },
            start as u8,
            (start >> 8) as u8,
            0xff,
            0xff,
        ];
        if kind != Request::Descriptors {
            p.extend_from_slice(if kind == Request::Services {
                &[0, 0x28]
            } else {
                &[3, 0x28]
            });
        }
        self.att(&p);
    }
    fn discovery_next(&mut self, kind: Request) {
        match kind {
            Request::Services => self.discover(Request::Characteristics, 1),
            Request::Characteristics => self.discover(Request::Descriptors, 1),
            Request::Descriptors => self.log.push("discovery complete".into()),
            _ => {}
        }
    }
    fn receive_att(&mut self, p: &[u8]) {
        if p.is_empty() {
            self.log.push("empty ATT packet".into());
            return;
        }
        if p[0] == 2 && p.len() == 3 {
            self.att(&[3, 23, 0]);
            return;
        }
        if p[0] == 0x1b || p[0] == 0x1d {
            if p.len() >= 3 {
                self.log.push(format!(
                    "notification handle=0x{:04x} value={} text={:?}",
                    word(&p[1..]),
                    hex(&p[3..]),
                    String::from_utf8_lossy(&p[3..])
                ));
            }
            if p[0] == 0x1d {
                self.att(&[0x1e]);
            }
            return;
        }
        if self.guest_central {
            self.peripheral_att(p);
            return;
        }
        let Some(kind) = self.pending.take() else {
            self.log.push(format!("unsolicited ATT {}", hex(p)));
            return;
        };
        if p[0] == 1 && p.len() == 5 {
            if p[4] == 0x0a
                && matches!(
                    kind,
                    Request::Services | Request::Characteristics | Request::Descriptors
                )
            {
                self.discovery_next(kind);
            } else {
                self.log.push(format!(
                    "ATT error request=0x{:02x} handle=0x{:04x} error=0x{:02x}",
                    p[1],
                    word(&p[2..]),
                    p[4]
                ));
            }
            return;
        }
        match kind {
            Request::Read(handle) if p[0] == 0x0b => self.log.push(format!(
                "read handle=0x{handle:04x} value={} text={:?}",
                hex(&p[1..]),
                String::from_utf8_lossy(&p[1..])
            )),
            Request::Write(handle) if p == [0x13] => self
                .log
                .push(format!("write complete handle=0x{handle:04x}")),
            Request::Services | Request::Characteristics | Request::Descriptors => {
                let expected = match kind {
                    Request::Services => 0x11,
                    Request::Characteristics => 9,
                    _ => 5,
                };
                if p[0] != expected || p.len() < 2 {
                    self.log
                        .push(format!("invalid ATT discovery response {}", hex(p)));
                    return;
                }
                let size = if kind == Request::Descriptors {
                    match p[1] {
                        1 => 4,
                        2 => 18,
                        _ => 0,
                    }
                } else {
                    p[1] as usize
                };
                let allowed = match kind {
                    Request::Services => matches!(size, 6 | 20),
                    Request::Characteristics => matches!(size, 7 | 21),
                    _ => matches!(size, 4 | 18),
                };
                if !allowed || p.len() == 2 || !(p.len() - 2).is_multiple_of(size) {
                    self.log.push(format!("invalid ATT entry size {}", hex(p)));
                    return;
                }
                let mut last = 0;
                for item in p[2..].chunks_exact(size) {
                    let handle = word(item);
                    let end = if kind == Request::Services {
                        word(&item[2..])
                    } else {
                        handle
                    };
                    if handle < self.discovery_start || handle <= last || end < handle {
                        self.log
                            .push("invalid non-progressing ATT discovery handles".into());
                        return;
                    }
                    last = end;
                    match kind {
                        Request::Services => { last = word(&item[2..]); self.log.push(format!("service start=0x{:04x} end=0x{last:04x} uuid={}", word(item), uuid(&item[4..]))); }
                        Request::Characteristics => self.log.push(format!("characteristic declaration=0x{last:04x} handle=0x{:04x} properties=0x{:02x} uuid={}", word(&item[3..]), item[2], uuid(&item[5..]))),
                        _ => self.log.push(format!("attribute handle=0x{last:04x} uuid={}", uuid(&item[2..]))),
                    }
                }
                if last == u16::MAX {
                    self.discovery_next(kind);
                } else {
                    self.discover(kind, last + 1);
                }
            }
            _ => self.log.push(format!("unexpected ATT response {}", hex(p))),
        }
    }
    fn peripheral_att(&mut self, p: &[u8]) {
        let error = |code, handle, reason| vec![1, code, handle as u8, (handle >> 8) as u8, reason];
        let response = match p[0] {
            0x10 if p.len() == 7 && word(&p[5..]) == 0x2800 => {
                if word(&p[1..]) == 1 && word(&p[3..]) >= 1 {
                    vec![0x11, 6, 1, 0, 3, 0, 0x0f, 0x18]
                } else {
                    error(p[0], word(&p[1..]), 0x0a)
                }
            }
            8 if p.len() == 7 && word(&p[5..]) == 0x2803 => {
                if (1..=2).contains(&word(&p[1..])) && word(&p[3..]) >= 2 {
                    vec![9, 7, 2, 0, 0x0a, 3, 0, 0x19, 0x2a]
                } else {
                    error(p[0], word(&p[1..]), 0x0a)
                }
            }
            4 if p.len() == 5 => {
                let mut response = vec![5, 1];
                for (handle, kind) in [(1u16, 0x2800u16), (2, 0x2803), (3, 0x2a19)] {
                    if (word(&p[1..])..=word(&p[3..])).contains(&handle) {
                        response.extend_from_slice(&handle.to_le_bytes());
                        response.extend_from_slice(&kind.to_le_bytes());
                    }
                }
                if response.len() == 2 {
                    error(p[0], word(&p[1..]), 0x0a)
                } else {
                    response
                }
            }
            0x0a if p.len() == 3 && word(&p[1..]) == 3 => {
                let mut r = vec![0x0b];
                r.extend_from_slice(&self.value);
                r
            }
            0x12 | 0x52 if p.len() >= 3 && word(&p[1..]) == 3 => {
                self.value = p[3..].to_vec();
                if p[0] == 0x52 {
                    return;
                }
                vec![0x13]
            }
            _ => error(p[0], if p.len() >= 3 { word(&p[1..]) } else { 0 }, 0x0a),
        };
        self.att(&response);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cmd(c: &mut Controller, op: u16, p: &[u8]) {
        let mut packet = vec![1, op as u8, (op >> 8) as u8, p.len() as u8];
        packet.extend_from_slice(p);
        c.send(&packet);
    }
    fn acl(c: &mut Controller, p: &[u8]) {
        let mut packet = vec![2, 1, 0x20, (p.len() + 4) as u8, 0, p.len() as u8, 0, 4, 0];
        packet.extend_from_slice(p);
        c.send(&packet);
    }
    fn connect(c: &mut Controller) {
        cmd(c, 0x200a, &[1]);
        c.command("connect").unwrap();
        c.packets.clear();
    }
    #[test]
    fn initialization_advertising_and_scanning_use_standard_packets() {
        let mut c = Controller::new();
        cmd(&mut c, 0x080f, &[3, 0]);
        assert_eq!(c.pop_packet().unwrap(), [4, 14, 4, 1, 15, 8, 0]);
        assert_eq!(c.default_link_policy, 3);
        let mut name = [0u8; 248];
        name[..8].copy_from_slice(b"esp32sim");
        cmd(&mut c, 0x0c13, &name);
        c.pop_packet();
        cmd(&mut c, 0x0c14, &[]);
        let response = c.pop_packet().unwrap();
        assert_eq!(&response[..7], &[4, 14, 252, 1, 20, 12, 0]);
        assert_eq!(&response[7..], name);
        cmd(&mut c, 0x1004, &[1]);
        assert_eq!(
            c.pop_packet().unwrap(),
            [4, 14, 14, 1, 4, 16, 0, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0]
        );
        let mut data = [0u8; 32];
        data[0] = ADV.len() as u8;
        data[1..1 + ADV.len()].copy_from_slice(ADV);
        cmd(&mut c, 0x2008, &data);
        cmd(&mut c, 0x200a, &[1]);
        assert!(c.is_advertising());
        let log = c.drain_log();
        assert!(log[0].contains("name=\"esp32sim\""));
        assert!(log[0].contains("service=180f"));
        c.packets.clear();
        cmd(&mut c, 0x200c, &[1, 1]);
        c.pop_packet();
        let report = c.pop_packet().unwrap();
        assert_eq!(&report[..7], &[4, 0x3e, (ADV.len() + 12) as u8, 2, 1, 0, 0]);
        assert!(report.windows(8).any(|s| s == b"esp32sim"));
        c.send(&[1, 3, 12, 1]);
        assert_eq!(c.pop_packet().unwrap(), [4, 14, 4, 1, 3, 12, 0x12]);
        cmd(&mut c, 0x7777, &[]);
        assert_eq!(c.pop_packet().unwrap(), [4, 14, 4, 1, 0x77, 0x77, 1]);
    }
    #[test]
    fn central_discovers_reads_writes_subscribes_and_receives_notifications() {
        let mut c = Controller::new();
        connect(&mut c);
        c.command("discover").unwrap();
        assert_eq!(
            &c.pop_packet().unwrap()[9..],
            &[0x10, 1, 0, 0xff, 0xff, 0, 0x28]
        );
        acl(&mut c, &[0x11, 6, 1, 0, 5, 0, 0x0f, 0x18]);
        acl(&mut c, &[1, 0x10, 6, 0, 0x0a]);
        acl(&mut c, &[9, 7, 2, 0, 0x1a, 3, 0, 0x19, 0x2a]);
        acl(&mut c, &[1, 8, 3, 0, 0x0a]);
        acl(&mut c, &[5, 1, 4, 0, 2, 0x29]);
        acl(&mut c, &[1, 4, 5, 0, 0x0a]);
        assert!(c.pending.is_none());
        assert!(c.log.iter().any(|s| s.contains("uuid=180f")));
        c.command("read 3").unwrap();
        assert!(c.command("read 3").is_err());
        acl(&mut c, b"\x0bhello");
        c.command("write 0x3 4849").unwrap();
        acl(&mut c, &[0x13]);
        c.command("subscribe 4").unwrap();
        acl(&mut c, &[0x13]);
        acl(&mut c, &[0x1b, 3, 0, 0, 0, 0, 0]);
        assert!(c.log.iter().any(|s| s.contains("text=\"hello\"")));
        assert!(c
            .log
            .iter()
            .any(|s| s.contains("notification handle=0x0003 value=00000000")));
        assert!(c.command("write 3 invalid").is_err());
    }
    #[test]
    fn malformed_commands_and_discovery_do_not_advance_state() {
        let mut c = Controller::new();
        cmd(&mut c, 0x0c13, &[1]);
        assert_eq!(c.pop_packet().unwrap().last(), Some(&0x12));
        cmd(&mut c, 0x0c35, &[1]);
        assert_eq!(c.pop_packet().unwrap().last(), Some(&0x12));
        cmd(&mut c, 0xfd09, &[1]);
        assert_eq!(c.pop_packet().unwrap().last(), Some(&0x12));
        connect(&mut c);
        c.command("discover").unwrap();
        c.packets.clear();
        acl(&mut c, &[0x11, 6, 2, 0, 1, 0, 0x0f, 0x18]);
        assert!(c.pending.is_none());
        assert!(c.drain_log().iter().any(|s| s.contains("non-progressing")));
        assert_eq!(
            c.packets.len(),
            1,
            "only ACL credit, no repeated discovery request"
        );
    }
    #[test]
    fn acl_reassembly_rejects_bad_lengths_and_virtual_peripheral_answers_att() {
        let mut c = Controller::new();
        c.connect(true);
        c.packets.clear();
        c.send(&[2, 1, 0x20, 5, 0, 3, 0, 4, 0, 0x0a]);
        c.send(&[2, 1, 0x10, 2, 0, 3, 0]);
        let packets: Vec<_> = c.packets.drain(..).collect();
        assert_eq!(&packets[2][9..], &[0x0b, 100]);
        c.send(&[2, 1, 0x20, 5, 0]);
        assert!(c.drain_log().iter().any(|s| s.contains("invalid ACL")));
        acl(&mut c, &[0x12, 3, 0, 42]);
        acl(&mut c, &[0x0a, 3, 0]);
        assert_eq!(&c.packets.back().unwrap()[9..], &[0x0b, 42]);
    }
}
