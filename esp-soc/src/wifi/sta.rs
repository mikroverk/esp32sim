//! A virtual station that joins the guest's own access point (`--wifi-sta`, the guest in
//! `WIFI_MODE_AP`). It follows the guest's beacons, authenticates (open system) and associates,
//! takes the supplicant's side of the WPA2 four-way handshake, and is then a host on the guest's
//! network: it takes a lease from the guest's DHCP server, renews it and answers ARP for it. With
//! the Ethernet relay on, a host program is that host instead and the station only carries its
//! frames. Joined, it sends a keep-alive when it has been silent for a while, so the guest's access
//! point does not disassociate it as inactive.
//!
//! It mirrors `VirtualAp`: the same deterministic nonces, and data framed as CCMP the way the MAC
//! leaves it after decrypting in place (protected bit, 8-byte CCMP header, 8 bytes of MIC space).

use super::{AirFrame, parse_bssid};
use esp_periph::crypto;

/// How long a join step (authentication, association, the handshake) may wait for the access
/// point before the station starts again from its next beacon; also the DHCP retransmission time.
const RETRY_US: u64 = 1_000_000;
/// How long the station stays joined without a beacon from its access point: 30 intervals at the
/// default 102.4 ms. Then the access point is gone (stopped, or the guest switched modes) and the
/// station listens for it again, so it rejoins one that comes back.
const BEACON_LOSS_US: u64 = 3_072_000;
/// How long the joined station may send the access point nothing before it sends a keep-alive: an
/// access point disassociates a station it has not heard from for its inactive time (reason 4).
/// ESP-IDF v5.5.4's SoftAP takes 10 s as its shortest (`esp_wifi_set_inactive_time`, `esp_wifi.h`:
/// ESP_ERR_INVALID_ARG below 10). Set to 10 s, `examples/wifi-softap` disassociated a silent
/// station 12 s after its last frame and one sending every 11 s; one sending every 10 s stayed. 8 s
/// is under the shortest setting with 2 s to spare, and later than the end of the 6 s golden run.
const KEEPALIVE_US: u64 = 8_000_000;
/// After this many refusals in a row with the same status the station stops trying: the access
/// point will not take what it offers, and trying again every second only fills the guest's log.
const REFUSALS_MAX: u32 = 3;
/// The cipher suites of IEEE 802.11-2020 Table 9-149, with the 00-0F-AC OUI.
const TKIP: [u8; 4] = [0x00, 0x0f, 0xac, 2];
const CCMP: [u8; 4] = [0x00, 0x0f, 0xac, 4];

#[derive(Clone, Debug)]
pub struct StaConfig { pub ssid: String, pub psk: Option<String>, pub mac: [u8; 6] }

impl StaConfig {
    /// `ssid=NAME[,psk=PASSPHRASE][,mac=xx:xx:xx:xx:xx:xx]`; `password` and `pass` alias `psk`.
    /// Unknown keys are errors, as in `ApConfig::parse`.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut cfg = StaConfig { ssid: String::new(), psk: None, mac: [0x02, 0x53, 0x54, 0x41, 0x00, 0x01] };
        for entry in spec.split(',').filter(|e| !e.is_empty()) {
            let (key, value) = entry.split_once('=').ok_or_else(|| format!("invalid station option '{entry}': expected key=value"))?;
            match key {
                "ssid" => cfg.ssid = value.to_string(),
                "psk" | "password" | "pass" => {
                    // IEEE 802.11-2020 J.4.1: a passphrase is 8 to 63 ASCII characters
                    if !(8..=63).contains(&value.len()) || !value.is_ascii() { return Err("invalid station passphrase: expected 8 to 63 ASCII characters".into()); }
                    cfg.psk = Some(value.to_string());
                }
                "mac" => cfg.mac = parse_bssid(value)?,
                _ => return Err(format!("unknown station option '{key}'")),
            }
        }
        if cfg.ssid.is_empty() || cfg.ssid.len() > 32 { return Err("the station needs ssid=NAME, 1 to 32 bytes".into()); }
        Ok(cfg)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StaLink { Listening, Authenticating, Associating, Handshake, Joined }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Dhcp { Idle, Discovering, Requesting, Bound, Renewing, Rebinding }

pub struct VirtualSta {
    pub cfg: StaConfig,
    pub log: bool,
    pub state: StaLink,
    /// whether the station is the host on the guest's network (DHCP, ARP); off in relay mode
    pub host: bool,
    bssid: [u8; 6],
    channel: u8,
    seq: u16,
    pub queue: Vec<AirFrame>,
    pmk: Option<Vec<u8>>,
    snonce: [u8; 32],
    anonce: [u8; 32],
    ptk: Vec<u8>,
    keys: bool,
    pn: u64,
    /// when the current join step or DHCP exchange gives up and starts again
    deadline_us: Option<u64>,
    /// no join before this time: after a refusal, so one beacon does not start the next attempt
    rejoin_after_us: u64,
    dhcp: Dhcp,
    xid: u32,
    offered: [u8; 4],
    server: [u8; 4],
    /// the DHCP server's MAC, from its ACK: a renewal goes to it unicast
    server_mac: [u8; 6],
    pub lease: Option<[u8; 4]>,
    /// when the first REQUEST of the current exchange went out; the ACK's times count from it
    request_us: Option<u64>,
    /// when the lease reaches T1 (renew with the server), T2 (rebind with any) and its end, from
    /// the ACK's times and `request_us`; `u64::MAX` for a lease without an end
    t1_us: u64,
    t2_us: u64,
    lease_end_us: u64,
    /// when the station last sent the access point a frame, and how long it may then stay silent
    /// before a keep-alive: `KEEPALIVE_US`, or shorter for a BSS Max Idle Period. A frame counts
    /// from when it is due, though the air may hold it a little longer for a busy receive ring
    /// (`StationLink::next_rx`, at most `RX_STALL_US`); against 2 s of margin that does not matter
    last_tx_us: u64,
    keepalive_us: u64,
    /// the access point counts only protected frames as activity (BSS Max Idle Period element)
    protected_keepalive: bool,
    /// keep-alive frames sent, and leases renewed or rebound
    pub keepalives: u64,
    pub renewals: u64,
    /// joins completed, frames sent to the access point, frames taken from it
    pub stats: (u64, u64, u64),
    /// join attempts started, from a beacon
    pub attempts: u64,
    /// why the station is not joined, the last reason it had: a refusal, a network it cannot join
    pub problem: Option<String>,
    /// when the access point's last beacon came, and the timestamp it carried
    last_beacon_us: u64,
    last_tsf: u64,
    /// the station's own Ethernet frames (DHCP, ARP) since `take_sent`, with when they go out
    sent: Vec<(u64, Vec<u8>)>,
    /// the RSN element the station sends, in its association request and message 2: the access
    /// point's group cipher, CCMP pairwise, PSK
    rsn: Vec<u8>,
    /// the last refusal's status and how many came in a row with it; at `REFUSALS_MAX` it stops
    refusals: (u16, u32),
    /// whether the access point's group cipher is TKIP (WPA/WPA2 mixed mode): its group frames end
    /// in TKIP's Michael MIC and ICV, 12 bytes, where CCMP's MIC is 8
    group_tkip: bool,
}

impl VirtualSta {
    pub fn new(cfg: StaConfig, log: bool) -> Self {
        let pmk = cfg.psk.as_ref().map(|p| crypto::pbkdf2_sha1(p.as_bytes(), cfg.ssid.as_bytes(), 4096, 32));
        // deterministic nonce: the emulator must replay identically run to run, as VirtualAp's
        let seed = crypto::sha1(&[pmk.as_deref().unwrap_or(cfg.ssid.as_bytes()), &cfg.mac[..]].concat());
        let mut snonce = [0u8; 32];
        for (i, b) in snonce.iter_mut().enumerate() { *b = seed[i % 20] ^ (i as u8) ^ 0xa5; }
        let xid = u32::from_be_bytes([seed[0], seed[1], seed[2], seed[3]]);
        VirtualSta { cfg, log, state: StaLink::Listening, host: true, bssid: [0; 6], channel: 1, seq: 0, queue: Vec::new(), pmk,
                     snonce, anonce: [0; 32], ptk: Vec::new(), keys: false, pn: 0, deadline_us: None, rejoin_after_us: 0,
                     dhcp: Dhcp::Idle, xid, offered: [0; 4], server: [0; 4], server_mac: [0; 6], lease: None, request_us: None, t1_us: u64::MAX,
                     t2_us: u64::MAX, lease_end_us: u64::MAX, last_tx_us: 0, keepalive_us: KEEPALIVE_US, protected_keepalive: false,
                     keepalives: 0, renewals: 0, stats: (0, 0, 0), attempts: 0,
                     problem: None, last_beacon_us: 0, last_tsf: 0, sent: Vec::new(), rsn: Vec::new(), refusals: (0, 0), group_tkip: false }
    }

    /// The channel of the guest's access point, from its DS Parameter Set; 1 before a beacon.
    pub fn channel(&self) -> u8 { self.channel }
    /// The guest access point's BSSID, from its beacon; zero before the station heard one.
    pub fn bssid(&self) -> [u8; 6] { self.bssid }

    /// The guest rebooted: its access point and everything it knew of the station are gone.
    pub fn restart(&mut self) {
        let (cfg, log, host, stats, attempts) = (self.cfg.clone(), self.log, self.host, self.stats, self.attempts);
        let (keepalives, renewals) = (self.keepalives, self.renewals);
        *self = VirtualSta { host, stats, attempts, keepalives, renewals, ..VirtualSta::new(cfg, log) };
    }

    fn note(&self, s: &str) { if self.log { eprintln!("[wifi-sta] {s}"); } }

    /// Whether the station is the host on the guest's network (`StationLink::set_relay`). With the
    /// relay on, the host program has the station's MAC and does its own DHCP, and the guest's
    /// replies go to it, not here: the station gives up its own exchange and lease, so it does not
    /// renew or rediscover under the program's MAC. With the relay off again it starts DHCP anew.
    pub fn set_host(&mut self, host: bool) {
        if self.host && !host {
            if self.state == StaLink::Joined { self.deadline_us = None; }                       // only DHCP's while joined
            (self.dhcp, self.lease) = (Dhcp::Idle, None);
            (self.t1_us, self.t2_us, self.lease_end_us) = (u64::MAX, u64::MAX, u64::MAX);
        }
        self.host = host;
    }

    fn leave(&mut self, now_us: u64, why: &str) {
        self.note(why);
        self.problem = Some(why.to_string());
        self.state = StaLink::Listening;
        (self.keys, self.ptk, self.deadline_us, self.dhcp, self.lease) = (false, Vec::new(), None, Dhcp::Idle, None);
        (self.t1_us, self.t2_us, self.lease_end_us) = (u64::MAX, u64::MAX, u64::MAX);
        self.rejoin_after_us = now_us + RETRY_US;
        self.queue.clear();
    }

    /// The access point refused `what` with `status`: try again from the next beacon, unless it
    /// has refused the same way `REFUSALS_MAX` times in a row.
    fn refused(&mut self, now_us: u64, what: &str, status: u16) {
        self.refusals = if self.refusals.0 == status { (status, self.refusals.1 + 1) } else { (status, 1) };
        if self.refusals.1 >= REFUSALS_MAX {
            self.leave(now_us, &format!("{what} refused {REFUSALS_MAX} times with status {status}: not trying again"));
            self.rejoin_after_us = u64::MAX;
        } else { self.leave(now_us, &format!("{what} refused, status {status}")); }
    }

    fn hdr(&mut self, fc: u16, a1: [u8; 6], a3: [u8; 6]) -> Vec<u8> {
        let mut f = Vec::with_capacity(128);
        f.extend_from_slice(&fc.to_le_bytes()); f.extend_from_slice(&[0, 0]);   // fc, duration
        f.extend_from_slice(&a1); f.extend_from_slice(&self.cfg.mac); f.extend_from_slice(&a3);
        f.extend_from_slice(&(self.seq << 4).to_le_bytes()); self.seq = self.seq.wrapping_add(1);
        f
    }
    fn send(&mut self, at_us: u64, frame: Vec<u8>) {
        self.stats.1 += 1;
        self.last_tx_us = self.last_tx_us.max(at_us);
        self.queue.push(AirFrame { at_us, frame });
    }
    /// When the joined station's next keep-alive is due, if it sends nothing before.
    fn keepalive_due_us(&self) -> u64 { self.last_tx_us.saturating_add(self.keepalive_us) }

    /// True when `step(now_us)` would return nothing and change nothing.
    pub fn idle_at(&self, now_us: u64) -> bool {
        self.queue.iter().all(|a| a.at_us > now_us) && self.deadline_us.is_none_or(|d| d > now_us)
            && !(self.state == StaLink::Joined && (now_us >= self.keepalive_due_us() || self.host && self.dhcp == Dhcp::Idle))
            && (self.state == StaLink::Listening || now_us < self.last_beacon_us + BEACON_LOSS_US)
    }

    /// Time-driven behaviour: give up a join step, retransmit DHCP or move the lease on whose time
    /// is up, start DHCP once joined, send a keep-alive that is due, then return the frames due at
    /// or before `now_us`.
    pub fn step(&mut self, now_us: u64) -> Vec<AirFrame> {
        if self.state != StaLink::Listening && now_us >= self.last_beacon_us + BEACON_LOSS_US {
            self.leave(now_us, "no beacon from the access point for 3 s: listening for it again");
        }
        if self.deadline_us.is_some_and(|d| d <= now_us) {
            self.deadline_us = None;
            match (self.state, self.dhcp) {
                (StaLink::Joined, Dhcp::Discovering | Dhcp::Requesting) => self.dhcp_send(now_us),
                (StaLink::Joined, Dhcp::Bound | Dhcp::Renewing | Dhcp::Rebinding) => self.lease_timer(now_us),
                (StaLink::Joined, Dhcp::Idle) => {}
                (StaLink::Handshake, _) => self.leave(now_us, "the four-way handshake did not finish (wrong passphrase?): listening for the beacon again"),
                _ => self.leave(now_us, "no answer from the access point: listening for its beacon again"),
            }
        }
        if self.host && self.state == StaLink::Joined && self.dhcp == Dhcp::Idle {
            self.dhcp = Dhcp::Discovering;
            self.dhcp_send(now_us);
        }
        if self.state == StaLink::Joined && now_us >= self.keepalive_due_us() { self.keepalive(now_us); }
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.queue).into_iter().partition(|a| a.at_us <= now_us);
        self.queue = later;
        due
    }

    /// The guest's access point transmitted `f` at `now_us`. Returns an Ethernet frame it sent the
    /// station, for the station's host or the relay.
    pub fn on_ap_tx(&mut self, f: &[u8], now_us: u64) -> Option<Vec<u8>> {
        // a frame the guest sends ends in the 4 bytes of the FCS slot
        if f.len() < 28 { return None; }
        let f = &f[..f.len() - 4];
        let fc = u16::from_le_bytes([f[0], f[1]]);
        let (ty, st) = ((fc >> 2) & 3, (fc >> 4) & 0xf);
        let to_us = f[4..10] == self.cfg.mac;
        let from_ap = self.state != StaLink::Listening && f[10..16] == self.bssid;
        match (ty, st) {
            (0, 8) => { self.on_beacon(f, now_us); None }
            (0, 11) if to_us && from_ap && self.state == StaLink::Authenticating && f.len() >= 30 => {
                let (seq, status) = (u16::from_le_bytes([f[26], f[27]]), u16::from_le_bytes([f[28], f[29]]));
                if seq != 2 { return None; }
                if status != 0 { self.refused(now_us, "authentication", status); return None; }
                self.associate(now_us);
                None
            }
            (0, 1) if to_us && from_ap && self.state == StaLink::Associating && f.len() >= 30 => {
                let status = u16::from_le_bytes([f[26], f[27]]);
                if status != 0 { self.refused(now_us, "association", status); return None; }
                // a BSS Max Idle Period shortens the interval to half the period: the access point
                // hears from the station twice in each. It does not lengthen it, as an access point
                // may keep an inactive time of its own beside the element
                let idle = bss_max_idle(&f[30..]).filter(|&(period, _)| period > 0);
                self.keepalive_us = idle.map_or(KEEPALIVE_US, |(period, _)| KEEPALIVE_US.min(u64::from(period) * 1_024_000 / 2));
                self.protected_keepalive = idle.is_some_and(|(_, protected)| protected);
                if self.pmk.is_some() {
                    self.state = StaLink::Handshake;
                    self.deadline_us = Some(now_us + RETRY_US);
                } else { self.joined(); }
                None
            }
            // to the station, or to every station when the access point stops
            (0, 10) | (0, 12) if (to_us || f[4] & 1 != 0) && from_ap => {
                let reason = f.get(24..26).map_or(0, |r| u16::from_le_bytes([r[0], r[1]]));
                // reason 15: four-way handshake timeout (IEEE 802.11-2020 Table 9-49)
                let hint = if reason == 15 { " (four-way handshake timeout: wrong passphrase?)" } else { "" };
                self.leave(now_us, &format!("{} by the access point, reason {reason}{hint}", if st == 12 { "deauthenticated" } else { "disassociated" }));
                None
            }
            // data from the DS, to the station or to a group, not the null and QoS-null frames
            (2, _) if fc & 0x0300 == 0x0200 && st & 4 == 0 && from_ap && (to_us || f[4] & 1 != 0)
                && matches!(self.state, StaLink::Handshake | StaLink::Joined) => {
                let hdr = if st & 8 != 0 { 26 } else { 24 };
                // protected: the 8-byte CCMP or TKIP header, then the payload in the clear, then
                // CCMP's 8-byte MIC, or for a group frame under a TKIP group cipher TKIP's 12 bytes
                let trailer = if f[4] & 1 != 0 && self.group_tkip { 12 } else { 8 };
                let body = if fc & 0x4000 != 0 { f.get(hdr + 8..f.len().saturating_sub(trailer))? } else { f.get(hdr..)? };
                if body.len() < 8 || body[..6] != [0xaa, 0xaa, 0x03, 0, 0, 0] { return None; }
                if body[6..8] == [0x88, 0x8e] { self.on_eapol(&body[8..], now_us); return None; }
                if self.state != StaLink::Joined { return None; }
                // the access point repeats a station's broadcasts to its BSS; a station drops its own
                if f[4] & 1 != 0 && f[16..22] == self.cfg.mac { return None; }
                self.stats.2 += 1;
                let mut e = Vec::with_capacity(f.len());
                e.extend_from_slice(&f[4..10]);    // dst = addr1
                e.extend_from_slice(&f[16..22]);   // src = addr3
                e.extend_from_slice(&body[6..]);
                Some(e)
            }
            _ => None,
        }
    }

    fn on_beacon(&mut self, f: &[u8], now_us: u64) {
        if f.len() < 36 { return; }
        let tsf = u64::from_le_bytes(f[24..32].try_into().unwrap());
        if self.state != StaLink::Listening {
            if f[10..16] != self.bssid { return; }
            // a timestamp lower than the last one's is another run of the access point's clock,
            // as an access point that restarts with its TSF reset sends (IEEE 802.11-2020 11.1.3).
            // The classic MAC model's TSF counts from the MAC's start, so there an access point
            // that stops and starts is caught by its disassociation or by the beacon loss instead
            if tsf < self.last_tsf { self.leave(now_us, "the access point started again: listening for it"); return; }
            (self.last_beacon_us, self.last_tsf) = (now_us, tsf);
            return;
        }
        if now_us < self.rejoin_after_us { return; }
        let (mut ssid, mut channel, mut rsn, mut i) = (None, None, None, 36);
        while i + 2 <= f.len() {
            let (id, len) = (f[i], f[i + 1] as usize);
            let Some(data) = f.get(i + 2..i + 2 + len) else { break };
            match id { 0 if ssid.is_none() => ssid = Some(data), 3 if len == 1 => channel = Some(data[0]), 48 => rsn = Some(data), _ => {} }
            i += 2 + len;
        }
        if ssid != Some(self.cfg.ssid.as_bytes()) { return; }
        let privacy = u16::from_le_bytes([f[34], f[35]]) & 0x0010 != 0;     // capability, Privacy
        let refusal = match (privacy, rsn.map(rsn_suites)) {
            (false, _) if self.pmk.is_some() => Some("is open and the station has a passphrase".to_string()),
            (false, _) => None,
            (true, None) => Some("is WEP or WPA, which the station does not take".to_string()),
            (true, Some(None)) => Some("has an RSN element the station cannot read".to_string()),
            (true, Some(Some(r))) => {
                if !r.akms.contains(&2) {
                    Some(if r.akms.contains(&8) { "is WPA3-SAE only, which the station does not take" } else { "offers no PSK key management" }.to_string())
                } else if r.mfpr { Some("requires protected management frames, which the station does not take".to_string()) }
                else if !r.pairwise.contains(&CCMP) { Some("offers no CCMP pairwise cipher, which the station needs".to_string()) }
                else if r.group != CCMP && r.group != TKIP { Some("uses a group cipher the station does not take".to_string()) }
                else if self.pmk.is_none() { Some("is secured and the station has no passphrase".to_string()) }
                else {
                    // the access point's group cipher, CCMP pairwise, PSK, no capabilities: the
                    // MAC hands frames over in the clear, so only the negotiation matters
                    self.rsn = [&[48, 20, 1, 0][..], &r.group, &[1, 0], &CCMP, &[1, 0], &[0x00, 0x0f, 0xac, 2], &[0, 0]].concat();
                    self.group_tkip = r.group == TKIP;
                    None
                }
            }
        };
        if let Some(why) = refusal {
            let why = format!("'{}' {why}: not joining", self.cfg.ssid);
            if self.problem.as_deref() != Some(why.as_str()) { self.note(&why); }
            self.problem = Some(why);
            self.rejoin_after_us = now_us + RETRY_US;
            return;
        }
        (self.last_beacon_us, self.last_tsf) = (now_us, tsf);
        self.attempts += 1;
        self.bssid.copy_from_slice(&f[10..16]);
        self.channel = channel.unwrap_or(self.channel);
        self.note(&format!("beacon of '{}' from {:02x?}, channel {}: authenticating", self.cfg.ssid, self.bssid, self.channel));
        let bssid = self.bssid;
        let mut a = self.hdr(11 << 4, bssid, bssid);
        a.extend_from_slice(&[0, 0, 1, 0, 0, 0]);                             // open system, sequence 1, status 0
        self.send(now_us + 500, a);
        self.state = StaLink::Authenticating;
        self.deadline_us = Some(now_us + RETRY_US);
    }

    fn associate(&mut self, now_us: u64) {
        let bssid = self.bssid;
        let mut r = self.hdr(0, bssid, bssid);
        // ESS, short preamble, short slot time; privacy with a passphrase (IEEE 802.11-2020 9.4.1.4)
        let capability: u16 = 0x0421 | if self.pmk.is_some() { 0x0010 } else { 0 };
        r.extend_from_slice(&capability.to_le_bytes()); r.extend_from_slice(&10u16.to_le_bytes());   // listen interval
        r.push(0); r.push(self.cfg.ssid.len() as u8); r.extend_from_slice(self.cfg.ssid.as_bytes());
        r.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);           // supported rates
        r.extend_from_slice(&[50, 4, 0x30, 0x48, 0x60, 0x6c]);                                     // extended rates
        if self.pmk.is_some() { let rsn = self.rsn.clone(); r.extend_from_slice(&rsn); }          // WPA2-PSK, CCMP pairwise
        self.send(now_us + 500, r);
        self.state = StaLink::Associating;
        self.deadline_us = Some(now_us + RETRY_US);
    }

    fn joined(&mut self) {
        self.state = StaLink::Joined;
        self.problem = None;
        self.refusals = (0, 0);
        self.deadline_us = None;
        self.stats.0 += 1;
        self.note(&format!("joined '{}'", self.cfg.ssid));
    }

    /// An EAPOL-Key frame from the authenticator: messages 1 and 3 of the four-way handshake
    /// (IEEE 802.11-2020 12.7.6). Only key descriptor version 2 (HMAC-SHA1, AES key wrap), as
    /// WPA2-PSK with CCMP uses.
    fn on_eapol(&mut self, b: &[u8], now_us: u64) {
        if b.len() < 99 || b[1] != 3 || self.state != StaLink::Handshake { return; }
        let n = 4 + u16::from_be_bytes([b[2], b[3]]) as usize;
        let Some(b) = b.get(..n).filter(|b| b.len() >= 99) else { return };
        let Some(pmk) = self.pmk.clone() else { return };
        let key_info = u16::from_be_bytes([b[5], b[6]]);
        if key_info & 7 != 2 || key_info & 0x0008 == 0 || key_info & 0x0080 == 0 { return; }   // version 2, pairwise, ack
        let mut replay = [0u8; 8]; replay.copy_from_slice(&b[9..17]);
        let mut nonce = [0u8; 32]; nonce.copy_from_slice(&b[17..49]);
        if key_info & 0x0100 == 0 {                                                         // message 1: no MIC
            self.anonce = nonce;
            let (aa, spa) = (self.bssid, self.cfg.mac);
            let (lo, hi) = if aa <= spa { (aa, spa) } else { (spa, aa) };
            let (ln, hn) = if self.anonce <= self.snonce { (self.anonce, self.snonce) } else { (self.snonce, self.anonce) };
            self.ptk = crypto::prf(&pmk, "Pairwise key expansion", &[&lo[..], &hi[..], &ln[..], &hn[..]].concat(), 384);
            let m2 = self.key_frame(0x010a, &replay, self.snonce, &self.rsn.clone());     // MIC, pairwise, version 2
            self.send_data(now_us + 200, self.bssid, 0x888e, &m2);
            self.note("handshake: message 1 in, message 2 out");
        } else if key_info & 0x0340 == 0x0340 && !self.ptk.is_empty() {                    // message 3: MIC, secure, install
            let mut probe = b.to_vec();
            let mic: Vec<u8> = probe[81..97].to_vec();
            probe[81..97].fill(0);
            if crypto::hmac_sha1(&self.ptk[..16], &probe)[..16] != mic[..] || nonce != self.anonce {
                self.note("handshake: message 3 failed its MIC or changed the ANonce (wrong passphrase?): ignored");
                return;
            }
            let m4 = self.key_frame(0x030a, &replay, [0; 32], &[]);                         // MIC, secure, pairwise, version 2
            self.send_data(now_us + 200, self.bssid, 0x888e, &m4);
            self.keys = true;
            self.note("handshake: message 3 in, message 4 out, keys installed");
            self.joined();
        }
    }

    fn key_frame(&self, key_info: u16, replay: &[u8; 8], nonce: [u8; 32], key_data: &[u8]) -> Vec<u8> {
        let mut b = vec![2, 3];                                                              // 802.1X-2004, EAPOL-Key
        b.extend_from_slice(&((95 + key_data.len()) as u16).to_be_bytes());
        b.push(2); b.extend_from_slice(&key_info.to_be_bytes()); b.extend_from_slice(&0u16.to_be_bytes());
        b.extend_from_slice(replay); b.extend_from_slice(&nonce);
        b.extend_from_slice(&[0; 16 + 8 + 8 + 16]);                                         // IV, RSC, ID, MIC
        b.extend_from_slice(&(key_data.len() as u16).to_be_bytes()); b.extend_from_slice(key_data);
        let mic = crypto::hmac_sha1(&self.ptk[..16], &b);
        b[81..97].copy_from_slice(&mic[..16]);
        b
    }

    /// `payload` of `ethertype` to `dst` through the access point, as a data frame to the DS.
    fn send_data(&mut self, at_us: u64, dst: [u8; 6], ethertype: u16, payload: &[u8]) {
        let f = self.data_frame(dst, ethertype, payload);
        self.send(at_us, f);
        if self.host && ethertype != 0x888e {
            let mut e = Vec::with_capacity(14 + payload.len());
            e.extend_from_slice(&dst); e.extend_from_slice(&self.cfg.mac); e.extend_from_slice(&ethertype.to_be_bytes()); e.extend_from_slice(payload);
            self.sent.push((at_us, e));
        }
    }
    /// The 802.11 frame of `send_data`, without the copy for the capture.
    fn data_frame(&mut self, dst: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
        let bssid = self.bssid;
        let protected = if self.keys { 0x4000 } else { 0 };
        let mut f = self.hdr(0x0108 | protected, bssid, dst);                               // data, to-DS
        if self.keys {
            self.pn += 1;
            let pn = self.pn;
            f.extend_from_slice(&[pn as u8, (pn >> 8) as u8, 0, 0x20, (pn >> 16) as u8, (pn >> 24) as u8, (pn >> 32) as u8, (pn >> 40) as u8]);
        }
        f.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]); f.extend_from_slice(&ethertype.to_be_bytes());
        f.extend_from_slice(payload);
        if self.keys { f.extend_from_slice(&[0; 8]); }                                      // MIC space
        f
    }

    /// The joined station has sent the access point nothing for `keepalive_us`: a null data frame
    /// to the DS (type 2, subtype 4). It has no body to protect, so where the access point requires
    /// protected keep-alives the station sends a data frame of ethertype 0x0800 with no payload
    /// instead, protected with the pairwise key, as wpa_supplicant 2.11 does (`wnm_bss_keep_alive`
    /// in `wpa_supplicant/events.c`). An open network has no key, and gets the null frame. Neither
    /// carries anything for the guest's network, so neither goes into the capture.
    fn keepalive(&mut self, now_us: u64) {
        self.keepalives += 1;
        let bssid = self.bssid;
        let f = if self.protected_keepalive && self.keys { self.data_frame(bssid, 0x0800, &[]) }
                else { self.hdr(0x0148, bssid, bssid) };                                    // null, to-DS
        self.send(now_us, f);
    }

    /// The Ethernet frames the station's own host sent since the last call (DHCP, ARP), for the
    /// capture; empty in relay mode, where the relay's frames are captured as they come in.
    pub fn take_sent(&mut self) -> Vec<(u64, Vec<u8>)> { std::mem::take(&mut self.sent) }

    /// An Ethernet frame from the relay to the access point's network, once joined.
    pub fn data_to_ds(&mut self, eth: &[u8], now_us: u64) -> bool {
        if self.state != StaLink::Joined || eth.len() < 14 { return false; }
        let mut dst = [0u8; 6]; dst.copy_from_slice(&eth[..6]);
        self.send_data(now_us, dst, u16::from_be_bytes([eth[12], eth[13]]), &eth[14..]);
        true
    }

    /// The station's own host: DHCP and ARP, for an Ethernet frame the access point sent it.
    pub fn host_rx(&mut self, eth: &[u8], now_us: u64) {
        if eth.len() < 14 { return; }
        let payload = &eth[14..];
        match u16::from_be_bytes([eth[12], eth[13]]) {
            0x0806 => self.arp(payload, now_us),
            0x0800 => self.dhcp_rx(payload, [eth[6], eth[7], eth[8], eth[9], eth[10], eth[11]], now_us),
            _ => {}
        }
    }

    fn arp(&mut self, p: &[u8], now_us: u64) {
        let Some(ip) = self.lease else { return };
        if p.len() < 28 || p[..8] != [0, 1, 8, 0, 6, 4, 0, 1] || p[24..28] != ip { return; }
        let mut r = vec![0, 1, 8, 0, 6, 4, 0, 2];
        r.extend_from_slice(&self.cfg.mac); r.extend_from_slice(&ip); r.extend_from_slice(&p[8..18]);
        let mut dst = [0u8; 6]; dst.copy_from_slice(&p[8..14]);
        self.send_data(now_us + 200, dst, 0x0806, &r);
    }

    /// The DHCP message of the current state: DISCOVER, REQUEST for the offer (options 50 and 54),
    /// or REQUEST for the lease held, with `ciaddr` set and neither option (RFC 2131 4.3.2): unicast
    /// to the server while renewing, broadcast while rebinding. Then the retransmission time: 1 s
    /// while discovering or requesting; while renewing or rebinding one half of the time left to
    /// T2 or to the lease's end, at least 60 s (RFC 2131 4.4.5), and at most that time.
    ///
    /// The lease counts from when the original REQUEST was sent (RFC 2131 4.4.1, and 4.4.5 for a
    /// renewal). A retransmission keeps the exchange's `xid`, and the rebinding REQUESTs keep the
    /// renewing ones' (`lease_timer` takes a new one only at T1), so an ACK may answer any REQUEST
    /// of the exchange: the station keeps the first one's time. The server counts from a REQUEST it
    /// received, never from before the first went out, so the station's lease does not end after
    /// the server's. A rebinding ACK so counts from T1: with a T2 past twice T1 (options 58 and 59;
    /// ESP-IDF v5.5.4's server sends neither) its new T1 has already passed, and the station renews
    /// again at once, counting two renewals.
    fn dhcp_send(&mut self, now_us: u64) {
        let (request, held) = (self.dhcp != Dhcp::Discovering, matches!(self.dhcp, Dhcp::Renewing | Dhcp::Rebinding));
        let ciaddr = if held { self.lease.unwrap_or([0; 4]) } else { [0; 4] };
        let mut m = vec![1, 1, 6, 0];                                                        // BOOTREQUEST, Ethernet
        m.extend_from_slice(&self.xid.to_be_bytes());
        m.extend_from_slice(&[0, 0, if held { 0 } else { 0x80 }, 0]);                        // secs, broadcast flag
        m.extend_from_slice(&ciaddr); m.extend_from_slice(&[0; 12]); m.extend_from_slice(&self.cfg.mac); m.extend_from_slice(&[0; 10 + 64 + 128]);
        m.extend_from_slice(&[0x63, 0x82, 0x53, 0x63, 53, 1, if request { 3 } else { 1 }]);  // RFC 2131 magic, DISCOVER/REQUEST
        if self.dhcp == Dhcp::Requesting { m.extend_from_slice(&[50, 4]); m.extend_from_slice(&self.offered); m.extend_from_slice(&[54, 4]); m.extend_from_slice(&self.server); }
        m.extend_from_slice(&[55, 3, 1, 3, 6, 255]);                                         // mask, router, DNS
        let mut udp = vec![0, 68, 0, 67, 0, 0, 0, 0];
        udp[4..6].copy_from_slice(&((8 + m.len()) as u16).to_be_bytes()); udp.extend_from_slice(&m);
        let unicast = self.dhcp == Dhcp::Renewing && self.server != [0; 4] && self.server_mac != [0; 6];
        let (ip_dst, eth_dst) = if unicast { (self.server, self.server_mac) } else { ([255; 4], [0xff; 6]) };
        let ip = ipv4(17, ciaddr, ip_dst, &udp);
        self.request_us = if request { Some(self.request_us.unwrap_or(now_us + 200)) } else { None };
        self.send_data(now_us + 200, eth_dst, 0x0800, &ip);
        let until = match self.dhcp { Dhcp::Renewing => self.t2_us, Dhcp::Rebinding => self.lease_end_us, _ => now_us + RETRY_US };
        self.deadline_us = Some(until.min(now_us + (until.saturating_sub(now_us) / 2).max(60_000_000)));
    }

    /// A lease timer is up (RFC 2131 4.4.5): at T1 the station renews with its server, at T2 it
    /// rebinds with any server, and at the lease's end it gives the address up, so ARP no longer
    /// answers for it, and starts again with DISCOVER. Each step goes out at once.
    fn lease_timer(&mut self, now_us: u64) {
        if self.dhcp == Dhcp::Bound { self.xid = self.xid.wrapping_add(1); }                  // a new exchange
        if now_us >= self.lease_end_us {
            self.note(&format!("the lease of {} ended: discovering again", dotted(self.lease.unwrap_or([0; 4]))));
            (self.dhcp, self.lease) = (Dhcp::Discovering, None);
            (self.t1_us, self.t2_us, self.lease_end_us) = (u64::MAX, u64::MAX, u64::MAX);
        } else if now_us >= self.t2_us {
            if self.dhcp != Dhcp::Rebinding { self.note("T2: rebinding the lease"); }
            self.dhcp = Dhcp::Rebinding;
        } else if now_us >= self.t1_us {
            if self.dhcp != Dhcp::Renewing { self.note("T1: renewing the lease"); }
            self.dhcp = Dhcp::Renewing;
        }
        self.dhcp_send(now_us);
    }

    /// An ACK binds `ip` with the times it gives: the lease (option 51) in seconds, T1 (58) and
    /// T2 (59), by default 0.5 and 0.875 of the lease (RFC 2131 4.4.5); ESP-IDF v5.5.4's DHCP server
    /// sends 51 and neither of the others. Without 51, or with 0xffffffff, the lease has no end and
    /// is never renewed. The times count from `sent_us`, when the exchange's first REQUEST went out.
    fn bind(&mut self, ip: [u8; 4], times: [Option<u32>; 3], sent_us: u64) {
        // a lease, T1 or T2 under 1 s is taken as 1 s: one of 0 would have the station ask again
        // in every round
        let us = |t: Option<u32>| t.map(|s| (u64::from(s) * 1_000_000).max(RETRY_US));
        (self.t1_us, self.t2_us, self.lease_end_us) = match times[0] {
            None | Some(u32::MAX) => (u64::MAX, u64::MAX, u64::MAX),
            Some(l) => {
                // a server's T2 past the lease or T1 past T2 is held to it
                let l = (u64::from(l) * 1_000_000).max(RETRY_US);
                let t2 = us(times[2]).unwrap_or(l / 8 * 7).min(l);
                (sent_us.saturating_add(us(times[1]).unwrap_or(l / 2).min(t2)), sent_us.saturating_add(t2), sent_us.saturating_add(l))
            }
        };
        self.request_us = None;
        if self.lease.is_some() { self.renewals += 1; }
        (self.dhcp, self.lease) = (Dhcp::Bound, Some(ip));
        self.deadline_us = (self.t1_us != u64::MAX).then_some(self.t1_us);
        self.note(&format!("leased {}{}", dotted(ip), times[0].filter(|&l| l != u32::MAX).map_or(String::new(), |l| format!(" for {l} s"))));
    }

    fn dhcp_rx(&mut self, p: &[u8], src_mac: [u8; 6], now_us: u64) {
        if p.len() < 20 || p[9] != 17 { return; }
        let ihl = (p[0] & 15) as usize * 4;
        let Some(udp) = p.get(ihl..) else { return };
        if udp.len() < 8 || udp[2..4] != [0, 68] { return; }
        let m = &udp[8..];
        if m.len() < 240 || m[0] != 2 || m[4..8] != self.xid.to_be_bytes() || m[236..240] != [0x63, 0x82, 0x53, 0x63] { return; }
        let yiaddr = [m[16], m[17], m[18], m[19]];
        let (mut kind, mut server, mut times, mut i) = (0, None, [None; 3], 240);
        while i < m.len() && m[i] != 255 {
            if m[i] == 0 { i += 1; continue; }
            let Some(&len) = m.get(i + 1) else { break };
            let Some(v) = m.get(i + 2..i + 2 + len as usize) else { break };
            match (m[i], len) {
                (53, 1) => kind = v[0],
                (54, 4) => server = Some([v[0], v[1], v[2], v[3]]),
                (51 | 58 | 59, 4) => times[match m[i] { 51 => 0, 58 => 1, _ => 2 }] = Some(u32::from_be_bytes([v[0], v[1], v[2], v[3]])),
                _ => {}
            }
            i += 2 + len as usize;
        }
        match (kind, self.dhcp) {
            (2, Dhcp::Discovering) => {                                                      // OFFER
                (self.offered, self.server) = (yiaddr, server.unwrap_or([0; 4]));
                self.dhcp = Dhcp::Requesting;
                self.dhcp_send(now_us);
            }
            (5, Dhcp::Requesting | Dhcp::Renewing | Dhcp::Rebinding) => {                    // ACK
                if let Some(s) = server { self.server = s; }
                self.server_mac = src_mac;
                self.bind(yiaddr, times, self.request_us.unwrap_or(now_us));
            }
            (6, Dhcp::Requesting) => { self.dhcp = Dhcp::Discovering; self.dhcp_send(now_us); }   // NAK
            (6, Dhcp::Renewing | Dhcp::Rebinding) => {
                self.note(&format!("the server refused to renew {}: discovering again", dotted(self.lease.unwrap_or([0; 4]))));
                (self.dhcp, self.lease) = (Dhcp::Discovering, None);
                (self.t1_us, self.t2_us, self.lease_end_us) = (u64::MAX, u64::MAX, u64::MAX);
                self.dhcp_send(now_us);
            }
            _ => {}
        }
    }

    /// T1, T2 and the lease's end, in emulated µs.
    #[cfg(test)]
    pub(super) fn lease_times(&self) -> (u64, u64, u64) { (self.t1_us, self.t2_us, self.lease_end_us) }

    /// The end-of-run line.
    pub fn report(&self) -> String {
        let problem = match (&self.problem, self.state) { (Some(p), StaLink::Listening) => format!("; {p}"), _ => String::new() };
        let mut more = String::new();
        if self.keepalives > 0 { more += &format!(", {} keep-alives", self.keepalives); }
        if self.renewals > 0 { more += &format!(", {} lease renewals", self.renewals); }
        format!("[emu] wifi-sta: '{}' {:?}, {} joins in {} attempts, lease {}, {} frames to the access point, {} data frames from it{more}{problem}",
            self.cfg.ssid, self.state, self.stats.0, self.attempts, self.lease.map_or("none".into(), dotted), self.stats.1, self.stats.2)
    }
}

/// The BSS Max Idle Period element (ID 90, IEEE 802.11-2020 9.4.2.79) among an association
/// response's elements: the period in units of 1000 TU, and whether the access point counts only
/// protected frames as activity (Idle Options bit 0, "protected keep-alive required").
fn bss_max_idle(ies: &[u8]) -> Option<(u16, bool)> {
    let mut i = 0;
    while let (Some(&id), Some(&len)) = (ies.get(i), ies.get(i + 1)) {
        let data = ies.get(i + 2..i + 2 + len as usize)?;
        if id == 90 && len >= 3 { return Some((u16::from_le_bytes([data[0], data[1]]), data[2] & 1 != 0)); }
        i += 2 + len as usize;
    }
    None
}

/// What an RSN element offers (IEEE 802.11-2020 9.4.2.24): the group cipher suite, the pairwise
/// ones, the AKM suite types with the 00-0F-AC OUI, and whether it requires protected management
/// frames (MFPR, RSN Capabilities bit 6).
struct Rsn { group: [u8; 4], pairwise: Vec<[u8; 4]>, akms: Vec<u8>, mfpr: bool }

/// `None` when the element is too short for what it says it holds.
fn rsn_suites(rsn: &[u8]) -> Option<Rsn> {
    let group: [u8; 4] = rsn.get(2..6)?.try_into().ok()?;
    let n = u16::from_le_bytes(rsn.get(6..8)?.try_into().ok()?) as usize;
    let pairwise = rsn.get(8..8 + 4 * n)?.chunks(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
    let at = 8 + 4 * n;
    let count = u16::from_le_bytes(rsn.get(at..at + 2)?.try_into().ok()?) as usize;
    let list = rsn.get(at + 2..at + 2 + 4 * count)?;
    let akms = list.chunks(4).filter(|s| s[..3] == [0x00, 0x0f, 0xac]).map(|s| s[3]).collect();
    let caps = rsn.get(at + 2 + 4 * count..at + 4 + 4 * count).map_or(0, |c| u16::from_le_bytes([c[0], c[1]]));
    Some(Rsn { group, pairwise, akms, mfpr: caps & 0x0040 != 0 })
}

fn dotted(ip: [u8; 4]) -> String { format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]) }

fn ipv4(protocol: u8, src: [u8; 4], dst: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut h = vec![0x45, 0, 0, 0, 0, 0, 0, 0, 64, protocol, 0, 0];
    h[2..4].copy_from_slice(&((20 + payload.len()) as u16).to_be_bytes());
    h.extend_from_slice(&src); h.extend_from_slice(&dst);
    let mut sum: u32 = h.chunks(2).map(|c| u32::from(u16::from_be_bytes([c[0], c[1]]))).sum();
    while sum >> 16 != 0 { sum = (sum & 0xffff) + (sum >> 16); }
    h[10..12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
    h.extend_from_slice(payload);
    h
}
