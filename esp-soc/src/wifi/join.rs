//! The air with the roles swapped: the emulated MAC is the station, the guest firmware runs the
//! access point.
//!
//! Firmware that is configured through its own soft AP (the usual first-boot flow) puts the guest
//! on the air as an AP: beacons, open-system authentication and association in the AP role, EAPOL
//! as the authenticator, a DHCP server on its own subnet. Nothing of that can be reached with the
//! virtual AP of `wifi.rs`, where the guest is the station. This side is the mirror image: it
//! hears the guest's beacons, joins them as a client, runs the WPA2 four-way handshake in the
//! supplicant role and takes an address from the guest's DHCP server. What the guest then serves
//! on that link is reachable from the host through `--hostfwd`.
//!
//! The crypto is not duplicated: the PMK, PTK, MIC and key-wrap primitives are the ones the
//! authenticator side uses (`esp_periph::crypto`), and the handshake messages are built in the
//! same wire format, only with the roles exchanged.

#[cfg(test)]
mod tests;

use super::{ies, mac_str, AirFrame, RSN_IE};
use crate::net::packet::{checksum, ethernet, ip_packet, udp_packet};

/// The station address the emulated MAC uses on the guest's AP: locally administered, distinct
/// from the virtual gateway and the forward peer.
pub const STA_MAC: [u8; 6] = [0x02, 0x53, 0x49, 0x4d, 0x00, 0x11];

/// A probe request goes out this often while scanning.
const PROBE_INTERVAL_US: u64 = 100_000;
/// How long past association an authentication or association exchange is tried again.
const RETRY_US: u64 = 250_000;
/// Tries per authentication/association phase before the join is given up.
const MAX_TRIES: u16 = 8;
/// Beacons (or probe responses) heard while scanning, before the join is given up.
const MAX_SCAN_TRIES: u16 = 300;
/// A DHCP DISCOVER is repeated this often, this many times, before the lease is given up.
const DHCP_RETRY_US: u64 = 1_000_000;
const MAX_DHCP_TRIES: u16 = 20;
/// How long after the association the AP's first EAPOL message waits before the association is
/// repeated (some APs drop the first message-1 while the station's port is still being set up).
const HANDSHAKE_RETRY_US: u64 = 1_000_000;

/// `--wifi-join` options. The guest decides the SSID, so every filter is optional: with none, the
/// first AP that beacons is joined.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JoinConfig {
    /// Join only an AP with this SSID.
    pub ssid: Option<String>,
    /// WPA2-PSK passphrase; without it only an open AP is joined.
    pub psk: Option<String>,
    /// Join only the AP with this BSSID.
    pub bssid: Option<[u8; 6]>,
}

impl JoinConfig {
    /// Parse `[ssid=NAME][,psk=PASSPHRASE][,bssid=AA:BB:CC:DD:EE:FF]`. Unknown keys are errors, so
    /// a misspelled passphrase option cannot silently join an open network.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut cfg = Self::default();
        if spec.is_empty() { return Ok(cfg); }
        for entry in spec.split(',') {
            let (key, value) = entry.split_once('=')
                .ok_or_else(|| format!("invalid join option '{entry}': expected key=value"))?;
            match key {
                "ssid" => cfg.ssid = Some(value.to_string()),
                "psk" | "password" | "pass" => cfg.psk = Some(value.to_string()),
                "bssid" => {
                    let invalid = || format!("invalid join BSSID '{value}': expected six hexadecimal octets");
                    let mut bssid = [0u8; 6];
                    let mut octets = value.split(':');
                    for byte in &mut bssid {
                        let octet = octets.next().filter(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                            .ok_or_else(invalid)?;
                        *byte = u8::from_str_radix(octet, 16).map_err(|_| invalid())?;
                    }
                    if octets.next().is_some() { return Err(invalid()); }
                    cfg.bssid = Some(bssid);
                }
                _ => return Err(format!("unknown join option '{key}'")),
            }
        }
        if cfg.psk.as_ref().is_some_and(|p| p.len() < 8) {
            return Err("invalid join passphrase: WPA2 needs at least 8 characters".into());
        }
        Ok(cfg)
    }

    /// One line for the console, naming what the join will accept.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        parts.push(match &self.ssid { Some(s) => format!("ssid '{s}'"), None => "any ssid".into() });
        parts.push(match &self.psk { Some(_) => "WPA2-PSK".into(), None => "open only".into() });
        if let Some(b) = &self.bssid { parts.push(format!("bssid {}", mac_str(b))); }
        parts.join(", ")
    }
}

/// What the join has done so far. A join that cannot proceed stops in [`Phase::Failed`] with the
/// reason, which is what the console line and the end-of-run report name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase { Scanning, Authenticating, Associating, Handshaking, Leasing, Online, Failed }

/// An access point heard on the air.
#[derive(Clone, Debug)]
pub struct ApSeen {
    pub bssid: [u8; 6],
    pub ssid: String,
    pub channel: u8,
    pub capability: u16,
    /// The AP's RSN information element, echoed back in the association request and in message 2
    /// the way a real supplicant does.
    pub rsn_ie: Option<Vec<u8>>,
}

impl ApSeen {
    pub fn secured(&self) -> bool { self.capability & 0x0010 != 0 }
}

/// The address the guest's DHCP server handed out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lease { pub ip: [u8; 4], pub mask: [u8; 4], pub router: Option<[u8; 4]>, pub dns: Option<[u8; 4]> }

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Beacons and probe responses heard, of which `matched` were eligible.
    pub beacons: u64, pub matched: u64,
    /// Probe requests, authentication and association requests sent.
    pub probes: u64, pub auths: u64, pub assocs: u64,
    /// EAPOL messages 2 and 4 sent, and whether the four-way handshake installed the keys.
    pub eapol2: u64, pub eapol4: u64, pub keys: bool,
    /// DHCP messages sent and the lease obtained.
    pub discovers: u64, pub requests: u64, pub lease: bool,
    /// Ethernet frames received from the guest and sent to it, ARP exchanges, and IPv4 frames
    /// whose protocol nothing here speaks.
    pub eth_rx: u64, pub eth_tx: u64, pub arps: u64, pub unhandled: u64,
}

/// The station half of the air: what the emulated MAC transmits to and receives from a guest that
/// runs the access point.
pub struct Supplicant {
    pub cfg: JoinConfig,
    pub phase: Phase,
    pub ap: Option<ApSeen>,
    pub lease: Option<Lease>,
    pub stats: Stats,
    /// Why the join stopped, when it did.
    pub failure: Option<String>,
    pub log: bool,
    sta: [u8; 6],
    seq: u16,
    pn: u64,
    queue: Vec<AirFrame>,
    /// The next moment a timer of the current phase is due.
    next_timer_us: u64,
    tries: u16,
    pmk: Option<[u8; 32]>,
    snonce: [u8; 32],
    anonce: [u8; 32],
    ptk: Option<Vec<u8>>,
    replay: u64,
    installed: bool,
    xid: u32,
    /// The address offered but not yet acknowledged.
    offered: Option<[u8; 4]>,
    server_id: Option<[u8; 4]>,
}

impl Supplicant {
    pub fn new(cfg: JoinConfig, log: bool) -> Self {
        let mut s = Supplicant {
            cfg, phase: Phase::Scanning, ap: None, lease: None, stats: Stats::default(), failure: None, log,
            sta: STA_MAC, seq: 0, pn: 0, queue: Vec::new(), next_timer_us: 0, tries: 0,
            pmk: None, snonce: [0; 32], anonce: [0; 32], ptk: None, replay: 0, installed: false,
            xid: 0x5350_0000, offered: None, server_id: None,
        };
        // deterministic nonce: the emulator must replay identically run to run
        let seed = esp_periph::crypto::sha1(&[&STA_MAC[..], b"esp32sim supplicant"].concat());
        for i in 0..32 { s.snonce[i] = seed[i % 20] ^ (i as u8); }
        s
    }

    pub fn sta_mac(&self) -> [u8; 6] { self.sta }
    /// Frames that were due but not delivered yet, back in the queue in order.
    pub fn requeue(&mut self, frames: Vec<AirFrame>) { self.queue.extend(frames); }
    /// Back to scanning, keeping the configuration: the guest's access point was reset.
    pub fn restart(&mut self) {
        self.phase = Phase::Scanning;
        self.ap = None;
        self.lease = None;
        self.offered = None;
        self.server_id = None;
        self.installed = false;
        self.ptk = None;
        self.replay = 0;
        self.tries = 0;
        self.queue.clear();
        self.next_timer_us = 0;
    }
    /// The channel the guest's AP beacons on, once one has been heard.
    pub fn channel(&self) -> Option<u8> { self.ap.as_ref().map(|a| a.channel) }
    /// The guest address the forward must aim at: the AP itself, from the lease's router.
    pub fn guest_ip(&self) -> Option<[u8; 4]> {
        self.lease.and_then(|l| l.router).or_else(|| self.ap.as_ref().map(|_| [192, 168, 4, 1]))
    }
    /// The MAC the guest sees for this side of the link.
    pub fn peer_mac(&self) -> [u8; 6] { self.sta }

    /// True when `step` would return nothing and change nothing.
    #[inline]
    pub fn idle_at(&self, now_us: u64) -> bool { self.queue.is_empty() && now_us < self.next_timer_us }

    /// Frames due at or before `now_us`, including the timers of the current phase.
    pub fn step(&mut self, now_us: u64) -> Vec<AirFrame> {
        if now_us >= self.next_timer_us { self.timer(now_us); }
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.queue).into_iter().partition(|a| a.at_us <= now_us);
        self.queue = later;
        due
    }

    fn timer(&mut self, now_us: u64) {
        let next = now_us + RETRY_US;
        match self.phase {
            Phase::Scanning => {
                self.tries = self.tries.saturating_add(1);
                if self.tries > MAX_SCAN_TRIES { return self.fail("no matching access point on the air") }
                let frame = self.probe_request(); self.send_at(now_us, frame);
                self.stats.probes += 1;
                self.next_timer_us = now_us + PROBE_INTERVAL_US;
            }
            Phase::Authenticating => {
                self.tries = self.tries.saturating_add(1);
                if self.tries > MAX_TRIES { return self.fail("the access point did not answer the authentication request") }
                let frame = self.auth_request(); self.send_at(now_us, frame);
                self.stats.auths += 1;
                self.next_timer_us = next;
            }
            Phase::Associating => {
                self.tries = self.tries.saturating_add(1);
                if self.tries > MAX_TRIES { return self.fail("the access point did not answer the association request") }
                let frame = self.assoc_request(); self.send_at(now_us, frame);
                self.stats.assocs += 1;
                self.next_timer_us = next;
            }
            Phase::Handshaking => {
                self.tries = self.tries.saturating_add(1);
                if self.tries > MAX_TRIES { return self.fail("the access point did not start the four-way handshake") }
                // repeat the association; the AP answers with a fresh message 1
                let frame = self.assoc_request(); self.send_at(now_us, frame);
                self.stats.assocs += 1;
                self.next_timer_us = now_us + HANDSHAKE_RETRY_US;
            }
            Phase::Leasing => {
                self.tries = self.tries.saturating_add(1);
                if self.tries > MAX_DHCP_TRIES { return self.fail("the access point's DHCP server did not answer") }
                self.send_dhcp(now_us, 1);
                self.next_timer_us = now_us + DHCP_RETRY_US;
            }
            Phase::Online | Phase::Failed => self.next_timer_us = u64::MAX,
        }
    }

    fn fail(&mut self, why: &str) {
        if self.phase != Phase::Failed {
            self.failure = Some(why.to_string());
            if self.log { eprintln!("[wifi-join] failed: {why}"); }
        }
        self.phase = Phase::Failed;
        self.next_timer_us = u64::MAX;
    }

    fn send_at(&mut self, at_us: u64, frame: Vec<u8>) { self.queue.push(AirFrame { at_us, frame }); }

    fn hdr(&mut self, fc: u16, a1: &[u8; 6], a2: &[u8; 6], a3: &[u8; 6]) -> Vec<u8> {
        let mut f = Vec::with_capacity(64);
        f.extend_from_slice(&fc.to_le_bytes());
        f.extend_from_slice(&[0, 0]);                                   // duration
        f.extend_from_slice(a1); f.extend_from_slice(a2); f.extend_from_slice(a3);
        f.extend_from_slice(&(self.seq << 4).to_le_bytes());
        self.seq = self.seq.wrapping_add(1);
        f
    }

    fn rates(f: &mut Vec<u8>) {
        f.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
        f.extend_from_slice(&[50, 4, 0x30, 0x48, 0x60, 0x6c]);
    }

    fn probe_request(&mut self) -> Vec<u8> {
        let ssid = self.cfg.ssid.clone().unwrap_or_default();
        let sta = self.sta;
        let mut f = self.hdr(4 << 4, &[0xff; 6], &sta, &[0xff; 6]);
        f.extend_from_slice(&[0u8; 8]);                                  // timestamp
        f.extend_from_slice(&100u16.to_le_bytes());                       // beacon interval
        f.extend_from_slice(&0x0001u16.to_le_bytes());                    // ESS
        f.push(0); f.push(ssid.len() as u8); f.extend_from_slice(ssid.as_bytes());
        Self::rates(&mut f);
        f
    }

    fn auth_request(&mut self) -> Vec<u8> {
        let Some(bssid) = self.ap.as_ref().map(|a| a.bssid) else { return Vec::new() };
        let sta = self.sta;
        let mut f = self.hdr(11 << 4, &bssid, &sta, &bssid);
        f.extend_from_slice(&[0, 0, 1, 0, 0, 0]);                        // open system, sequence 1, success
        f
    }

    fn assoc_request(&mut self) -> Vec<u8> {
        let Some(ap) = self.ap.clone() else { return Vec::new() };
        let sta = self.sta;
        let mut f = self.hdr(0 << 4, &ap.bssid, &sta, &ap.bssid);
        let capability = 0x0001u16 | if ap.secured() { 0x0010 } else { 0 } | 0x0400;   // ESS, privacy, short slot
        f.extend_from_slice(&capability.to_le_bytes());
        f.extend_from_slice(&10u16.to_le_bytes());                        // listen interval
        f.push(0); f.push(ap.ssid.len() as u8); f.extend_from_slice(ap.ssid.as_bytes());
        Self::rates(&mut f);
        if let Some(rsn) = &ap.rsn_ie { f.extend_from_slice(rsn); }
        f
    }

    /// Wrap an Ethernet frame in a data frame to the DS: addr1 is the AP, addr2 this station,
    /// addr3 the destination. Once the keys are installed the frame carries the protection bit, a
    /// CCMP header and MIC space, with the payload in the clear — the MAC would have encrypted it,
    /// exactly as the access-point side of the model frames what it sends down.
    pub(crate) fn wrap_eth(&mut self, eth: &[u8]) -> Option<Vec<u8>> {
        if eth.len() < 14 { return None; }
        let bssid = self.ap.as_ref().map(|a| a.bssid)?;
        let mut dst = [0u8; 6]; dst.copy_from_slice(&eth[0..6]);
        let mut fc = 0x0108u16;                                          // data, to the DS
        let protected = self.installed;
        if protected { fc |= 0x4000; }
        let sta = self.sta;
        let mut f = self.hdr(fc, &bssid, &sta, &dst);
        if protected {
            // CCMP header as the hardware leaves it after encrypting in place
            self.pn += 1;
            let pn = self.pn;
            let keyid = u8::from(dst[0] & 1 != 0);                       // group frames use the GTK
            f.extend_from_slice(&[pn as u8, (pn >> 8) as u8, 0, 0x20 | (keyid << 6),
                                  (pn >> 16) as u8, (pn >> 24) as u8, (pn >> 32) as u8, (pn >> 40) as u8]);
        }
        f.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);               // LLC/SNAP
        f.extend_from_slice(&eth[12..14]);
        f.extend_from_slice(&eth[14..]);
        if protected { f.extend_from_slice(&[0u8; 8]); }                 // MIC space
        self.stats.eth_tx += 1;
        Some(f)
    }

    /// The AP transmitted `f` at `now_us`. Returns Ethernet frames the host side has to answer
    /// (everything but the link's own management, EAPOL, DHCP and ARP, which are handled here).
    pub fn on_guest_tx(&mut self, f: &[u8], now_us: u64) -> Vec<Vec<u8>> {
        if f.len() < 24 { return Vec::new(); }
        let fc = u16::from_le_bytes([f[0], f[1]]);
        let (ty, st) = ((fc >> 2) & 3, (fc >> 4) & 0xf);
        match (ty, st) {
            (0, 8) | (0, 5) => { self.learn(f, st, now_us); Vec::new() }
            (0, 11) => { self.on_auth(f, now_us); Vec::new() }
            (0, 1) => { self.on_assoc(f, now_us); Vec::new() }
            (0, 10) | (0, 12) => { self.on_deauth(f, now_us); Vec::new() }
            (2, _) => self.on_data(f, fc, st, now_us),
            _ => Vec::new(),
        }
    }

    /// A beacon or probe response: what the scan needs to know about the AP.
    fn learn(&mut self, f: &[u8], _st: u16, now_us: u64) {
        self.stats.beacons += 1;
        if f.len() < 36 { return; }
        let mut bssid = [0u8; 6]; bssid.copy_from_slice(&f[16..22]);
        if bssid == [0; 6] { bssid.copy_from_slice(&f[10..16]); }
        if self.cfg.bssid.is_some_and(|want| want != bssid) { return; }
        let body = 36;                                                   // 24-byte header, 8 timestamp, 2 interval, 2 capability
        let capability = u16::from_le_bytes([f[body - 2], f[body - 1]]);
        let mut ssid = String::new();
        let mut channel = 0u8;
        let mut rsn = None;
        for (id, d) in ies(f, body) {
            match id {
                0 if ssid.is_empty() => ssid = String::from_utf8_lossy(d).into_owned(),
                3 if !d.is_empty() => channel = d[0],
                48 if rsn.is_none() => { let mut ie = vec![48, d.len() as u8]; ie.extend_from_slice(d); rsn = Some(ie); }
                _ => {}
            }
        }
        // An AP whose security the join cannot satisfy is not a candidate: pressing on would only
        // leave the guest waiting for a handshake this side cannot complete.
        let secured = capability & 0x0010 != 0;
        if secured && self.cfg.psk.is_none() { return; }
        if !secured && self.cfg.psk.is_some() { return; }
        if self.cfg.ssid.as_ref().is_some_and(|want| *want != ssid) { return; }
        self.stats.matched += 1;
        let seen = ApSeen { bssid, ssid: ssid.clone(), channel: if channel == 0 { 1 } else { channel }, capability, rsn_ie: rsn };
        let first = self.ap.as_ref().is_none_or(|a| a.bssid != bssid);
        self.ap = Some(seen);
        if self.phase != Phase::Scanning { return; }
        if first && self.log {
            eprintln!("[wifi-join] heard AP '{}' bssid {} channel {} ({})",
                      ssid, mac_str(&bssid), channel, if secured { "WPA2-PSK" } else { "open" });
        }
        if secured {
            let pmk = esp_periph::crypto::pbkdf2_sha1(self.cfg.psk.as_ref().map_or(&[][..], |p| p.as_bytes()),
                                                      ssid.as_bytes(), 4096, 32);
            let mut key = [0u8; 32]; key.copy_from_slice(&pmk);
            self.pmk = Some(key);
        }
        self.phase = Phase::Authenticating;
        self.tries = 0;
        let frame = self.auth_request(); self.send_at(now_us + 2_000, frame);
        self.stats.auths += 1;
        self.next_timer_us = now_us + RETRY_US;
    }

    fn addressed_to_me(&self, f: &[u8]) -> bool {
        let mut a1 = [0u8; 6]; a1.copy_from_slice(&f[4..10]);
        a1 == self.sta || a1 == [0xff; 6]
    }

    fn on_auth(&mut self, f: &[u8], now_us: u64) {
        if self.phase != Phase::Authenticating || f.len() < 30 || !self.addressed_to_me(f) { return; }
        if u16::from_le_bytes([f[24], f[25]]) != 0 { return; }            // open system only
        let status = u16::from_le_bytes([f[28], f[29]]);
        if status != 0 {
            return self.fail(&format!("the access point refused authentication (status {status})"));
        }
        self.phase = Phase::Associating;
        self.tries = 0;
        let frame = self.assoc_request(); self.send_at(now_us + 500, frame);
        self.stats.assocs += 1;
        self.next_timer_us = now_us + RETRY_US;
    }

    fn on_assoc(&mut self, f: &[u8], now_us: u64) {
        if !matches!(self.phase, Phase::Associating | Phase::Handshaking) || f.len() < 30 || !self.addressed_to_me(f) { return; }
        let status = u16::from_le_bytes([f[26], f[27]]);
        if status != 0 {
            return self.fail(&format!("the access point refused association (status {status})"));
        }
        if self.phase == Phase::Associating {
            if self.log { eprintln!("[wifi-join] associated"); }
            self.tries = 0;
            if self.ap.as_ref().is_some_and(|a| a.secured()) {
                self.phase = Phase::Handshaking;
                self.next_timer_us = now_us + HANDSHAKE_RETRY_US;
            } else {
                self.start_lease(now_us + 1_000);
            }
        }
    }

    fn on_deauth(&mut self, f: &[u8], now_us: u64) {
        if !self.addressed_to_me(f) || !matches!(self.phase, Phase::Handshaking | Phase::Leasing | Phase::Online) { return; }
        if self.log { eprintln!("[wifi-join] the access point ended the association, scanning again"); }
        self.installed = false;
        self.ptk = None;
        self.lease = None;
        self.offered = None;
        self.phase = Phase::Scanning;
        self.tries = 0;
        self.next_timer_us = now_us;
    }

    /// A data frame from the AP (from the DS): EAPOL, DHCP, ARP, or IPv4 the host side answers.
    fn on_data(&mut self, f: &[u8], fc: u16, st: u16, now_us: u64) -> Vec<Vec<u8>> {
        let from_ds = fc & 0x0200 != 0;
        if !from_ds || !self.addressed_to_me(f) { return Vec::new(); }
        let hdr = if st & 8 != 0 { 26 } else { 24 };
        if f.len() <= hdr + 8 { return Vec::new(); }
        // Protected frame: the MAC would have decrypted in place, so the descriptor holds plaintext
        // framed by an 8-byte CCMP header and 8 bytes of MIC space.
        let mut plain;
        let f: &[u8] = if fc & 0x4000 != 0 && f.len() > hdr + 16 {
            plain = Vec::with_capacity(f.len() - 16);
            plain.extend_from_slice(&f[..hdr]);
            plain.extend_from_slice(&f[hdr + 8..f.len() - 8]);
            plain[1] &= !0x40;
            &plain
        } else { f };
        if f[hdr] != 0xaa || f[hdr + 1] != 0xaa || f[hdr + 2] != 0x03 { return Vec::new(); }
        let ethertype = u16::from_be_bytes([f[hdr + 6], f[hdr + 7]]);
        let payload = &f[hdr + 8..];
        let mut dst = [0u8; 6]; dst.copy_from_slice(&f[4..10]);
        let mut src = [0u8; 6]; src.copy_from_slice(&f[16..22]);
        let eth = ethernet(&dst, &src, ethertype, payload);
        match ethertype {
            0x888e => { self.on_eapol(payload, now_us); Vec::new() }
            0x0806 => { self.on_arp(&eth, now_us); Vec::new() }
            0x0800 => {
                if self.on_udp_lease(&eth, now_us) || self.icmp_reply(&eth, now_us) { return Vec::new(); }
                self.stats.eth_rx += 1; vec![eth]
            }
            _ => { self.stats.eth_rx += 1; vec![eth] }
        }
    }

    // ---- WPA2 supplicant ------------------------------------------------------------------

    /// Build an EAPOL-Key frame; `mic` says whether the KCK covers it (messages 2 and 4).
    fn eapol(&mut self, key_info: u16, nonce: &[u8; 32], key_data: &[u8], mic: bool) -> Vec<u8> {
        let mut body = Vec::with_capacity(99 + key_data.len());
        body.push(2);                                                    // 802.1X-2004
        body.push(3);                                                    // EAPOL-Key
        body.extend_from_slice(&((95 + key_data.len()) as u16).to_be_bytes());
        body.push(2);                                                    // RSN key descriptor
        body.extend_from_slice(&key_info.to_be_bytes());
        body.extend_from_slice(&16u16.to_be_bytes());                    // key length (CCMP)
        body.extend_from_slice(&self.replay.to_be_bytes());
        body.extend_from_slice(nonce);
        body.extend_from_slice(&[0u8; 16]);                              // key IV
        body.extend_from_slice(&[0u8; 8]);                               // key RSC
        body.extend_from_slice(&[0u8; 8]);                               // key ID
        let mic_at = body.len();
        body.extend_from_slice(&[0u8; 16]);
        body.extend_from_slice(&(key_data.len() as u16).to_be_bytes());
        body.extend_from_slice(key_data);
        if mic {
            if let Some(kck) = self.ptk.as_ref().map(|p| &p[0..16]) {
                let m = esp_periph::crypto::hmac_sha1(kck, &body);
                body[mic_at..mic_at + 16].copy_from_slice(&m[..16]);
            }
        }
        let Some(bssid) = self.ap.as_ref().map(|a| a.bssid) else { return Vec::new() };
        let sta = self.sta;
        let mut f = self.hdr(0x0108, &bssid, &sta, &bssid);              // data, to the DS
        f[16..22].copy_from_slice(&bssid);
        f.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
        f.extend_from_slice(&body);
        f
    }

    /// EAPOL-Key from the authenticator: message 1 starts the supplicant, message 3 installs the
    /// pairwise key and asks for message 4.
    fn on_eapol(&mut self, body: &[u8], now_us: u64) {
        if body.len() < 99 || body[1] != 3 { return; }
        let n = 4 + u16::from_be_bytes([body[2], body[3]]) as usize;
        let Some(body) = body.get(..n).filter(|b| b.len() >= 99) else { return; };
        let key_data_len = u16::from_be_bytes([body[97], body[98]]) as usize;
        if 99 + key_data_len != body.len() { return; }
        let key_info = u16::from_be_bytes([body[5], body[6]]);
        let (mic_set, secure, ack, install) = (key_info & 0x0100 != 0, key_info & 0x0200 != 0, key_info & 0x0080 != 0, key_info & 0x0040 != 0);
        self.replay = u64::from_be_bytes(body[9..17].try_into().unwrap());
        match self.phase {
            Phase::Handshaking if ack && !mic_set => {
                // message 1: the authenticator's nonce; answer with the SNonce and a MIC over the
                // whole 802.1X frame, both keyed by the PTK both sides now derive
                let mut anonce = [0u8; 32]; anonce.copy_from_slice(&body[17..49]);
                self.anonce = anonce;
                let pmk = self.pmk.unwrap_or([0; 32]);
                let bssid = self.ap.as_ref().map_or([0u8; 6], |a| a.bssid);
                let sta = self.sta;
                let (lo_mac, hi_mac) = if bssid <= sta { (bssid, sta) } else { (sta, bssid) };
                let (sn, an) = (self.snonce, self.anonce);
                let (lo_n, hi_n) = if an <= sn { (an, sn) } else { (sn, an) };
                let mut data = Vec::with_capacity(76);
                data.extend_from_slice(&lo_mac); data.extend_from_slice(&hi_mac);
                data.extend_from_slice(&lo_n); data.extend_from_slice(&hi_n);
                let ptk = esp_periph::crypto::prf(&pmk, "Pairwise key expansion", &data, 384);
                self.ptk = Some(ptk);
                // message 2: SNonce plus the AP's own RSN IE, so the authenticator's check of the
                // security parameters matches what it advertised
                let key_data = self.ap.as_ref().and_then(|a| a.rsn_ie.clone()).unwrap_or_else(|| RSN_IE.to_vec());
                let snonce = self.snonce;
                let m2 = self.eapol(0x010a, &snonce, &key_data, true);
                self.send_at(now_us + 2_000, m2);
                self.stats.eapol2 += 1;
                self.next_timer_us = now_us + HANDSHAKE_RETRY_US;
                if self.log { eprintln!("[wifi-join] WPA2 message 1 received, message 2 sent"); }
            }
            Phase::Handshaking if mic_set && secure && install => {
                // message 3: verify the MIC before trusting the key data, then install and answer
                let Some(ptk) = self.ptk.clone() else { return };
                let mut probe = body.to_vec();
                let mut received = [0u8; 16]; received.copy_from_slice(&probe[81..97]);
                for b in probe[81..97].iter_mut() { *b = 0; }
                let calc = esp_periph::crypto::hmac_sha1(&ptk[0..16], &probe);
                if calc[..16] != received {
                    if self.log { eprintln!("[wifi-join] message 3 MIC mismatch, repeating the association"); }
                    self.phase = Phase::Associating;
                    self.tries = 0;
                    self.ptk = None;
                    self.next_timer_us = now_us;
                    return;
                }
                self.installed = true;
                self.stats.keys = true;
                let m4 = self.eapol(0x030a, &[0u8; 32], &[], true);
                self.send_at(now_us + 1_000, m4);
                self.stats.eapol4 += 1;
                if self.log { eprintln!("[wifi-join] WPA2 four-way handshake complete (supplicant side)"); }
                self.start_lease(now_us + 1_000);
            }
            _ => {}
        }
    }

    // ---- DHCP client ----------------------------------------------------------------------

    fn start_lease(&mut self, now_us: u64) {
        self.phase = Phase::Leasing;
        self.tries = 0;
        self.send_dhcp(now_us, 1);
        self.next_timer_us = now_us + DHCP_RETRY_US;
    }

    /// A BOOTP request from 0.0.0.0:68 to the broadcast address, as a client without a lease sends.
    fn send_dhcp(&mut self, now_us: u64, msg_type: u8) {
        self.xid = self.xid.wrapping_add(1);
        let mut b = vec![0u8; 240];
        b[0] = 1; b[1] = 1; b[2] = 6;                                    // BOOTREQUEST, ethernet, 6-byte MAC
        b[4..8].copy_from_slice(&self.xid.to_be_bytes());
        b[10] = 0x80;                                                    // broadcast the reply
        b[28..34].copy_from_slice(&self.sta);
        b[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
        b.extend_from_slice(&[53, 1, msg_type]);
        if msg_type == 3 {
            if let Some(ip) = self.offered { b.extend_from_slice(&[50, 4]); b.extend_from_slice(&ip); }
            if let Some(server) = self.server_id { b.extend_from_slice(&[54, 4]); b.extend_from_slice(&server); }
        }
        b.extend_from_slice(&[55, 4, 1, 3, 6, 15]);                      // mask, router, DNS, domain
        b.push(255);
        while b.len() < 300 { b.push(0); }
        let udp = udp_packet(&[0; 4], &[255; 4], 68, 67, &b);
        let ip = ip_packet(17, &[0; 4], &[255; 4], &udp);
        let frame = ethernet(&[0xff; 6], &self.sta, 0x0800, &ip);
        self.stats.discovers += u64::from(msg_type == 1);
        self.stats.requests += u64::from(msg_type == 3);
        if let Some(f) = self.wrap_eth(&frame) { self.send_at(now_us + 1_000, f); }
    }

    /// A DHCP reply from the guest's server, if this frame is one. Returns true when it was
    /// consumed here rather than handed to the host side.
    fn on_udp_lease(&mut self, eth: &[u8], now_us: u64) -> bool {
        if eth.len() < 14 + 28 || self.phase != Phase::Leasing { return false; }
        let p = &eth[14..];
        if p[9] != 17 { return false; }
        let ihl = ((p[0] & 0xf) as usize) * 4;
        if p.len() < ihl + 8 { return false; }
        let udp = &p[ihl..];
        if u16::from_be_bytes([udp[2], udp[3]]) != 68 { return false; }
        let d = &udp[8..];
        if d.len() < 240 || d[0] != 2 || d[4..8] != self.xid.to_be_bytes() { return false; }
        if d[28..34] != self.sta { return false; }
        let mut msg_type = 0u8;
        let mut mask = None; let mut router = None; let mut dns = None; let mut server = None;
        let mut i = 240;
        while i + 1 < d.len() {
            let (opt, len) = (d[i], d[i + 1] as usize);
            if opt == 255 { break; }
            if opt == 0 { i += 1; continue; }
            if i + 2 + len > d.len() { break; }
            let v = &d[i + 2..i + 2 + len];
            match (opt, len) {
                (53, 1) => msg_type = v[0],
                (1, 4) => mask = Some([v[0], v[1], v[2], v[3]]),
                (3, n) if n >= 4 => router = Some([v[0], v[1], v[2], v[3]]),
                (6, n) if n >= 4 => dns = Some([v[0], v[1], v[2], v[3]]),
                (54, 4) => server = Some([v[0], v[1], v[2], v[3]]),
                _ => {}
            }
            i += 2 + len;
        }
        let mut yiaddr = [0u8; 4]; yiaddr.copy_from_slice(&d[16..20]);
        match msg_type {
            2 => {
                if self.log { eprintln!("[wifi-join] DHCP offer {}", ip_str(&yiaddr)); }
                self.offered = Some(yiaddr);
                self.server_id = server;
                self.send_dhcp(now_us, 3);
                self.next_timer_us = now_us + DHCP_RETRY_US;
            }
            5 => {
                let lease = Lease { ip: yiaddr, mask: mask.unwrap_or([255, 255, 255, 0]), router, dns };
                self.lease = Some(lease);
                self.stats.lease = true;
                self.phase = Phase::Online;
                self.next_timer_us = u64::MAX;
                if self.log {
                    eprintln!("[wifi-join] lease {} mask {} router {} dns {}",
                              ip_str(&lease.ip), ip_str(&lease.mask),
                              lease.router.map_or("-".into(), |r| ip_str(&r)),
                              lease.dns.map_or("-".into(), |r| ip_str(&r)));
                    eprintln!("[wifi-join] joined '{}' as {}", self.ap.as_ref().map_or("?", |a| &a.ssid[..]), ip_str(&lease.ip));
                }
            }
            _ => {}
        }
        true
    }

    // ---- ARP and ICMP on the AP's subnet --------------------------------------------------

    /// Answer ARP for the address the guest leased to this station: the guest resolves us before
    /// it can answer anything the forward sends.
    fn on_arp(&mut self, eth: &[u8], now_us: u64) {
        let Some(lease) = self.lease else { return };
        let p = &eth[14..];
        if p.len() < 28 || u16::from_be_bytes([p[6], p[7]]) != 1 { return; }
        let target = [p[24], p[25], p[26], p[27]];
        if target != lease.ip { return; }
        let mut sender_hw = [0u8; 6]; sender_hw.copy_from_slice(&p[8..14]);
        let mut sender_ip = [0u8; 4]; sender_ip.copy_from_slice(&p[14..18]);
        let mut r = Vec::with_capacity(28);
        r.extend_from_slice(&[0, 1, 8, 0, 6, 4, 0, 2]);
        r.extend_from_slice(&self.sta); r.extend_from_slice(&lease.ip);
        r.extend_from_slice(&sender_hw); r.extend_from_slice(&sender_ip);
        self.stats.arps += 1;
        let frame = ethernet(&sender_hw, &self.sta, 0x0806, &r);
        if let Some(f) = self.wrap_eth(&frame) { self.send_at(now_us + 500, f); }
    }

    /// An ICMP echo request addressed to us, if this is one: answer it, so a firmware that pings
    /// its client sees a host rather than a dead address.
    fn icmp_reply(&mut self, eth: &[u8], now_us: u64) -> bool {
        let Some(lease) = self.lease else { return false };
        let p = &eth[14..];
        if p.len() < 28 || p[9] != 1 { return false; }
        let ihl = ((p[0] & 0xf) as usize) * 4;
        if p.len() < ihl + 8 || p[ihl] != 8 { return false; }
        let mut dst_ip = [0u8; 4]; dst_ip.copy_from_slice(&p[16..20]);
        if dst_ip != lease.ip { return false; }
        let mut src_ip = [0u8; 4]; src_ip.copy_from_slice(&p[12..16]);
        let mut icmp = p[ihl..].to_vec();
        icmp[0] = 0; icmp[2] = 0; icmp[3] = 0;
        let c = checksum(&icmp, 0).to_be_bytes();
        icmp[2] = c[0]; icmp[3] = c[1];
        let packet = ip_packet(1, &lease.ip, &src_ip, &icmp);
        let frame = ethernet(&self.ap.as_ref().map_or([0xff; 6], |a| a.bssid), &self.sta, 0x0800, &packet);
        if let Some(f) = self.wrap_eth(&frame) { self.send_at(now_us + 500, f); }
        true
    }

    /// End-of-run line for the join, or empty when no join was configured.
    pub fn report(&self) -> String {
        let ap = self.ap.as_ref().map_or_else(|| "-".to_string(),
            |a| format!("'{}' bssid {} channel {}", a.ssid, mac_str(&a.bssid), a.channel));
        let lease = self.lease.map_or_else(|| "-".to_string(), |l| {
            format!("{} via {}", ip_str(&l.ip), l.router.map_or("-".into(), |r| ip_str(&r)))
        });
        let why = self.failure.as_ref().map_or(String::new(), |w| format!(", failed: {w}"));
        format!("[emu] wifi-join: AP {ap}, state {:?}, lease {lease}, {} beacons ({} matched), {} probes, {} auths, {} assocs, \
                 {} EAPOL-2, {} EAPOL-4, keys {}, {} DHCP requests, {} ARP, {} frames from the guest, {} to it{why}",
                self.phase, self.stats.beacons, self.stats.matched, self.stats.probes, self.stats.auths,
                self.stats.assocs, self.stats.eapol2, self.stats.eapol4, self.stats.keys,
                self.stats.discovers + self.stats.requests, self.stats.arps, self.stats.eth_rx, self.stats.eth_tx)
    }
}

fn ip_str(a: &[u8; 4]) -> String { format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3]) }
