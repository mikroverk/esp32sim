//! The virtual air interface: 802.11 frame helpers and a minimal access point the emulated MAC
//! "hears". The AP beacons, answers probe requests, and completes open-system authentication and
//! association; data frames are handed to the network backend (docs/networking-plan.md).

#[cfg(test)]
mod tests;
#[cfg(test)]
mod two_roles;
pub mod join;
pub use join::{ApSeen, JoinConfig, Lease, Phase as JoinPhase, Supplicant, STA_MAC};

pub fn mac_str(m: &[u8]) -> String { m.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(":") }

pub(super) fn ies(f: &[u8], body: usize) -> Vec<(u8, &[u8])> {
    let mut v = Vec::new(); let mut i = body;
    while i + 2 <= f.len() { let (id, l) = (f[i], f[i + 1] as usize); if i + 2 + l > f.len() { break; } v.push((id, &f[i + 2..i + 2 + l])); i += 2 + l; }
    v
}
fn mgmt_body_offset(subtype: u16) -> usize { match subtype { 8 | 5 => 24 + 12, 0 => 24 + 4, 1 => 24 + 6, 11 => 24 + 6, _ => 24 } }

/// One-line description of an 802.11 frame (frame control, addresses, SSID for management frames).
pub fn describe(f: &[u8]) -> String {
    if f.len() < 24 { return format!("{} bytes (short): {:02x?}", f.len(), f); }
    let fc = u16::from_le_bytes([f[0], f[1]]);
    let (ty, st) = ((fc >> 2) & 3, (fc >> 4) & 0xf);
    let kind = match (ty, st) {
        (0, 0) => "assoc-req", (0, 1) => "assoc-resp", (0, 4) => "probe-req", (0, 5) => "probe-resp", (0, 8) => "beacon",
        (0, 10) => "disassoc", (0, 11) => "auth", (0, 12) => "deauth", (0, 13) => "action",
        (1, 11) => "rts", (1, 12) => "cts", (1, 13) => "ack", (1, 10) => "ps-poll",
        (2, 0) => "data", (2, 4) => "null", (2, 8) => "qos-data", (2, 12) => "qos-null", _ => "?",
    };
    let mut s = format!("{} bytes {} ({}/{}) a1={} a2={} a3={}", f.len(), kind, ty, st, mac_str(&f[4..10]), mac_str(&f[10..16]), mac_str(&f[16..22]));
    if ty == 0 { for (id, d) in ies(f, mgmt_body_offset(st)) { if id == 0 { s += &format!(" ssid='{}'", String::from_utf8_lossy(d)); } } }
    if ty == 2 && f.len() >= 32 { let off = if st & 8 != 0 { 26 } else { 24 }; if f.len() >= off + 8 && f[off] == 0xaa { let et = u16::from_be_bytes([f[off + 6], f[off + 7]]); s += &format!(" ethertype={:#06x}", et); } }
    s
}

/// True for a beacon frame (management subtype 8).
pub fn is_beacon(f: &[u8]) -> bool { f.len() >= 2 && f[0] & 0x0c == 0 && (f[0] >> 4) & 0xf == 8 }

/// RSN information element advertised in beacons and echoed in handshake message 3: WPA2-PSK, CCMP.
pub const RSN_IE: &[u8] = &[48, 20, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 4, 1, 0, 0x00, 0x0f, 0xac, 2, 0, 0];

#[derive(Clone, Debug)]
pub struct ApConfig { pub ssid: String, pub bssid: [u8; 6], pub channel: u8, pub psk: Option<String> }

impl ApConfig {
    /// Parse the shared CLI/browser AP configuration. Unknown keys are errors so a
    /// misspelled passphrase option cannot silently configure an open network.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut cfg = Self {
            ssid: "esp32sim".into(), bssid: [0x02, 0x53, 0x49, 0x4d, 0x00, 0x01], channel: 6, psk: None,
        };
        if spec.is_empty() { return Ok(cfg); }
        for entry in spec.split(',') {
            let (key, value) = entry.split_once('=')
                .ok_or_else(|| format!("invalid WiFi option '{entry}': expected key=value"))?;
            match key {
                "ssid" => cfg.ssid = value.to_string(),
                "chan" | "channel" | "ch" => {
                    cfg.channel = value.parse::<u8>().ok().filter(|n| (1..=14).contains(n))
                        .ok_or_else(|| format!("invalid WiFi channel '{value}': expected 1 through 14"))?;
                }
                "psk" | "password" | "pass" => cfg.psk = Some(value.to_string()),
                "bssid" => cfg.bssid = parse_bssid(value)?,
                _ => return Err(format!("unknown WiFi option '{key}'")),
            }
        }
        Ok(cfg)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StaState { Idle, Authenticated, Associated }

/// A frame the AP puts on the air, with the emulated time (µs) it should reach the station.
pub struct AirFrame { pub at_us: u64, pub frame: Vec<u8> }

/// WPA2 four-way handshake state (AP side).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WpaState {
    #[default]
    Idle,
    AwaitingMessage2,
    AwaitingMessage4,
    Installed,
}

#[derive(Default)]
pub struct Wpa {
    pub pmk: [u8; 32],
    pub anonce: [u8; 32],
    pub gtk: [u8; 16],
    pub replay: u64,
    pub state: WpaState,
}

/// Six hexadecimal octets separated by colons.
fn parse_bssid(value: &str) -> Result<[u8; 6], String> {
    let invalid = || format!("invalid WiFi BSSID '{value}': expected six hexadecimal octets");
    let mut bssid = [0u8; 6];
    let mut octets = value.split(':');
    for byte in &mut bssid {
        let octet = octets.next().filter(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(invalid)?;
        *byte = u8::from_str_radix(octet, 16).map_err(|_| invalid())?;
    }
    if octets.next().is_some() { return Err(invalid()); }
    Ok(bssid)
}

/// A network around the station, heard on the air but not joinable: it answers probe requests,
/// so a scan lists it with its channel, signal and security (the `bssid`, `ssid`, `primary`,
/// `rssi` and `authmode` of IDF's `wifi_ap_record_t`, v5.5.5
/// components/esp_wifi/include/esp_wifi_types_generic.h:320-349; v4.4.8
/// components/esp_wifi/include/esp_wifi_types.h:191-210), and nothing else, so connecting to it
/// fails as to a network out of reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeaconAp { pub ssid: String, pub bssid: [u8; 6], pub channel: u8, pub rssi: i8, pub secure: bool }

/// The most beacon-only networks an access point carries.
pub const BEACONS_MAX: usize = 32;

impl BeaconAp {
    /// `--wifi-nearby`'s `key=value,...`: `ssid` (1 to 32 bytes, required), `chan` 1 through 14
    /// (default 1), `rssi` in dBm (default -60), `secure` 0 or 1 (default 1, WPA2-PSK in the scan),
    /// `bssid` (default: a locally administered address derived from the SSID, the same every run).
    pub fn parse(spec: &str) -> Result<BeaconAp, String> {
        let mut b = BeaconAp { ssid: String::new(), bssid: [0; 6], channel: 1, rssi: -60, secure: true };
        let mut bssid = None;
        for entry in spec.split(',') {
            let (key, value) = entry.split_once('=').ok_or_else(|| format!("invalid nearby-network option '{entry}': expected key=value"))?;
            match key {
                "ssid" => b.ssid = value.to_string(),
                "chan" | "channel" | "ch" => b.channel = value.parse::<u8>().ok().filter(|n| (1..=14).contains(n))
                    .ok_or_else(|| format!("invalid nearby-network channel '{value}': expected 1 through 14"))?,
                "rssi" => b.rssi = value.parse::<i8>().map_err(|_| format!("invalid nearby-network rssi '{value}': expected -128 through 127 dBm"))?,
                "secure" => b.secure = match value { "1" | "true" | "yes" => true, "0" | "false" | "no" => false,
                    _ => return Err(format!("invalid nearby-network secure '{value}': expected 0 or 1")) },
                "bssid" => bssid = Some(parse_bssid(value)?),
                _ => return Err(format!("unknown nearby-network option '{key}'")),
            }
        }
        if !(1..=32).contains(&b.ssid.len()) { return Err("a nearby network needs an ssid of 1 to 32 bytes".into()); }
        b.bssid = bssid.unwrap_or_else(|| {
            // FNV-1a over the SSID, in a locally administered unicast address (bit 1 of the first octet)
            let h = b.ssid.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, c| (h ^ u64::from(c)).wrapping_mul(0x100_0000_01b3));
            let x = h.to_le_bytes();
            [0x02, x[0], x[1], x[2], x[3], x[4]]
        });
        Ok(b)
    }

    /// Every `--wifi-nearby` spec, at most `BEACONS_MAX` of them, each on a BSSID of its own (two
    /// specs with one SSID and no `bssid` derive the same one).
    pub fn parse_all(specs: &[String]) -> Result<Vec<BeaconAp>, String> {
        if specs.len() > BEACONS_MAX { return Err(format!("more than {BEACONS_MAX} nearby networks")); }
        let list = specs.iter().map(|s| Self::parse(s)).collect::<Result<Vec<_>, _>>()?;
        for (i, b) in list.iter().enumerate() {
            if let Some(o) = list[..i].iter().find(|o| o.bssid == b.bssid) {
                return Err(format!("nearby networks '{}' and '{}' share the BSSID {}", o.ssid, b.ssid, mac_str(&b.bssid)));
            }
        }
        Ok(list)
    }
}

pub struct VirtualAp {
    pub cfg: ApConfig,
    pub wpa: Wpa,
    pub state: StaState,
    pub sta: [u8; 6],
    pub aid: u16,
    pub next_beacon_us: u64,
    pub beacon_interval_us: u64,
    pub seq: u16,
    pub queue: Vec<AirFrame>,
    pub log: bool,
    pub stats: (u64, u64, u64),   // beacons, probe responses, data frames from the station
    pub pn: u64,                  // CCMP packet number for frames we send
    /// The networks around, beacon-only and not joinable (`set_beacons`).
    pub beacons: Vec<BeaconAp>,
}

impl VirtualAp {
    pub fn new(cfg: ApConfig, log: bool) -> Self {
        let mut wpa = Wpa::default();
        if let Some(psk) = &cfg.psk {
            wpa.pmk.copy_from_slice(&esp_periph::crypto::pbkdf2_sha1(psk.as_bytes(), cfg.ssid.as_bytes(), 4096, 32));
            // deterministic nonces/GTK: the emulator must replay identically run to run
            let seed = esp_periph::crypto::sha1(&[&wpa.pmk[..], &cfg.bssid[..]].concat());
            for i in 0..32 { wpa.anonce[i] = seed[i % 20] ^ (i as u8); }
            for i in 0..16 { wpa.gtk[i] = seed[(i + 3) % 20] ^ 0x5a; }
        }
        VirtualAp { cfg, wpa, state: StaState::Idle, sta: [0; 6], aid: 1, next_beacon_us: 100_000, beacon_interval_us: 102_400, seq: 0,
                    queue: Vec::new(), pn: 0, log, stats: (0, 0, 0), beacons: Vec::new() }
    }
    fn hdr(&mut self, fc: u16, a1: &[u8; 6], a3: &[u8; 6]) -> Vec<u8> {
        let mut f = Vec::with_capacity(128);
        f.extend_from_slice(&fc.to_le_bytes()); f.extend_from_slice(&[0, 0]);   // fc, duration
        f.extend_from_slice(a1); f.extend_from_slice(&self.cfg.bssid); f.extend_from_slice(a3);
        f.extend_from_slice(&(self.seq << 4).to_le_bytes()); self.seq = self.seq.wrapping_add(1);
        f
    }
    fn capability(&self) -> u16 { 0x0001 | if self.cfg.psk.is_some() { 0x0010 } else { 0 } | 0x0400 }   // ESS, privacy, short slot
    fn common_ies(&self, f: &mut Vec<u8>) {
        f.push(0); f.push(self.cfg.ssid.len() as u8); f.extend_from_slice(self.cfg.ssid.as_bytes());
        f.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);      // supported rates 1 2 5.5 11 (basic) 6 9 12 18
        f.extend_from_slice(&[3, 1, self.cfg.channel]);                                     // DS parameter set
        f.extend_from_slice(&[5, 4, 0, 1, 0, 0]);                                           // TIM
        f.extend_from_slice(&[7, 6, b'S', b'E', b' ', 1, 13, 20]);                          // country
        f.extend_from_slice(&[50, 4, 0x30, 0x48, 0x60, 0x6c]);                              // extended rates 24 36 48 54
        if self.cfg.psk.is_some() { f.extend_from_slice(RSN_IE); }                          // RSN: WPA2-PSK, CCMP
    }
    fn beacon_like(&mut self, subtype: u16, dst: &[u8; 6], now_us: u64) -> Vec<u8> {
        let bssid = self.cfg.bssid;
        let mut f = self.hdr(subtype << 4, dst, &bssid);
        f.extend_from_slice(&now_us.to_le_bytes());                                         // timestamp
        f.extend_from_slice(&100u16.to_le_bytes());                                          // beacon interval (TU)
        f.extend_from_slice(&self.capability().to_le_bytes());
        self.common_ies(&mut f);
        f
    }
    /// A probe response from a network around, as its own BSSID: the fixed fields (timestamp,
    /// beacon interval, capability: ESS, Privacy when secured, Short Slot Time; IEEE 802.11-2020
    /// 9.3.3.10 and 9.4.1.4) and the SSID, Supported Rates, DSSS Parameter Set (its channel),
    /// Extended Supported Rates and, when secured, RSN elements (element ids 0, 1, 3, 50 and 48,
    /// IEEE 802.11-2020 Table 9-92); the rates are this access point's.
    fn beacon_ap_response(&mut self, b: &BeaconAp, dst: &[u8; 6], now_us: u64) -> Vec<u8> {
        let mut f = Vec::with_capacity(96);
        f.extend_from_slice(&(5u16 << 4).to_le_bytes()); f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(dst); f.extend_from_slice(&b.bssid); f.extend_from_slice(&b.bssid);
        f.extend_from_slice(&(self.seq << 4).to_le_bytes()); self.seq = self.seq.wrapping_add(1);
        f.extend_from_slice(&now_us.to_le_bytes());
        f.extend_from_slice(&100u16.to_le_bytes());
        let cap: u16 = 0x0001 | if b.secure { 0x0010 } else { 0 } | 0x0400;
        f.extend_from_slice(&cap.to_le_bytes());
        f.push(0); f.push(b.ssid.len() as u8); f.extend_from_slice(b.ssid.as_bytes());
        f.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
        f.extend_from_slice(&[3, 1, b.channel]);
        f.extend_from_slice(&[50, 4, 0x30, 0x48, 0x60, 0x6c]);
        if b.secure { f.extend_from_slice(RSN_IE); }
        f
    }
    /// The channel and signal (dBm) a frame from the air arrives with, for the chip's RX
    /// descriptor (`rssi` and `channel` of `wifi_pkt_rx_ctrl_t`, IDF v5.5.5
    /// components/esp_wifi/include/local/esp_wifi_types_native.h:36 and :59-60; v4.4.8
    /// components/esp_wifi/include/esp_wifi_types.h:387 and :408-409): a network around's own for
    /// a frame from its BSSID, this access point's channel and -40 dBm otherwise.
    pub fn rx_meta(&self, frame: &[u8]) -> (u8, i8) {
        if frame.len() >= 16 {
            if let Some(b) = self.beacons.iter().find(|b| frame[10..16] == b.bssid) { return (b.channel, b.rssi); }
        }
        (self.cfg.channel, -40)
    }
    fn send(&mut self, at_us: u64, frame: Vec<u8>) {
        if self.log { let d = describe(&frame); if d.contains("auth") || d.contains("assoc") || d.contains("888e") { eprintln!("[wifi] AP -> {}  hex={:02x?}", d, frame); } else { eprintln!("[wifi] AP -> {} (t+{} us)", d, at_us); } }
        self.queue.push(AirFrame { at_us, frame });
    }
    /// True when `step(now_us)` would return nothing and change nothing: no frame queued and the
    /// next beacon not yet due. Keep this next to `step`; it must follow every timer `step` has.
    #[inline]
    pub fn idle_at(&self, now_us: u64) -> bool { self.queue.is_empty() && now_us < self.next_beacon_us }
    /// Time-driven behaviour (beacons). Returns frames due at or before `now_us`.
    pub fn step(&mut self, now_us: u64) -> Vec<AirFrame> {
        let mgmt_pending = self.queue.iter().any(|a| !is_beacon(&a.frame));
        if now_us >= self.next_beacon_us && !mgmt_pending {
            self.next_beacon_us += self.beacon_interval_us;
            if self.next_beacon_us <= now_us { self.next_beacon_us = now_us + self.beacon_interval_us; }
            let b = self.beacon_like(8, &[0xff; 6], now_us); self.stats.0 += 1;
            self.queue.push(AirFrame { at_us: now_us, frame: b });
        }
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.queue).into_iter().partition(|a| a.at_us <= now_us);
        self.queue = later;
        due
    }
    /// The station transmitted `f` at `now_us`. Returns data frames (as 802.11) for the network backend.
    pub fn on_station_tx(&mut self, f: &[u8], now_us: u64) -> Option<Vec<u8>> {
        if f.len() < 24 { return None; }
        let fc = u16::from_le_bytes([f[0], f[1]]);
        let (ty, st) = ((fc >> 2) & 3, (fc >> 4) & 0xf);
        let mut a2 = [0u8; 6]; a2.copy_from_slice(&f[10..16]);
        let to_us = |a1: &[u8]| a1 == [0xff; 6] || a1 == self.cfg.bssid;
        match (ty, st) {
            (0, 4) => {                                                                      // probe request: for us or wildcard?
                let ssid_ok = ies(f, 24).iter().any(|(id, d)| *id == 0 && (d.is_empty() || *d == self.cfg.ssid.as_bytes()));
                if ssid_ok && to_us(&f[4..10]) { let r = self.beacon_like(5, &a2, now_us); self.stats.1 += 1; self.send(now_us + 1500, r); }
                // the networks around answer a wildcard probe, or one for their SSID, sent to the
                // broadcast address or their own with a wildcard BSSID or their own (IEEE
                // 802.11-2020 11.1.4.3.4)
                let asked: Vec<Vec<u8>> = ies(f, 24).into_iter().filter(|(id, _)| *id == 0).map(|(_, d)| d.to_vec()).collect();
                let addressed = |b: &BeaconAp| [&f[4..10], &f[16..22]].iter().all(|a| *a == [0xff; 6] || *a == b.bssid);
                let around: Vec<BeaconAp> = self.beacons.iter()
                    .filter(|b| addressed(b) && asked.iter().any(|s| s.is_empty() || *s == b.ssid.as_bytes())).cloned().collect();
                for (i, b) in around.iter().enumerate() {
                    let r = self.beacon_ap_response(b, &a2, now_us);
                    self.send(now_us + 1900 + 400 * i as u64, r);
                }
            }
            (0, 11) if f.len() >= 30 && to_us(&f[4..10]) => {                                // authentication (open system)
                let (alg, seq) = (u16::from_le_bytes([f[24], f[25]]), u16::from_le_bytes([f[26], f[27]]));
                if self.log { eprintln!("[wifi] station AUTH req alg={} seq={} status={} hex={:02x?}", alg, seq, u16::from_le_bytes([f[28],f[29]]), f); }
                if alg == 0 && seq == 1 {
                    self.sta = a2; self.state = StaState::Authenticated;
                    let mut r = self.hdr(11 << 4, &a2, &self.cfg.bssid.clone());
                    r.extend_from_slice(&[0, 0, 2, 0, 0, 0]);                                // open, seq 2, status success
                    self.send(now_us + 300, r);
                }
            }
            (0, 0) | (0, 2) if to_us(&f[4..10]) && self.state != StaState::Idle => {         // (re)association request
                self.state = StaState::Associated;
                let mut r = self.hdr(1 << 4, &a2, &self.cfg.bssid.clone());
                r.extend_from_slice(&self.capability().to_le_bytes()); r.extend_from_slice(&[0, 0]); r.extend_from_slice(&(0xc000 | self.aid).to_le_bytes());
                r.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]); r.extend_from_slice(&[50, 4, 0x30, 0x48, 0x60, 0x6c]);
                self.send(now_us + 300, r);
                if self.cfg.psk.is_some() {                       // WPA2: start the four-way handshake
                    self.wpa.replay += 1;
                    let m1 = self.eapol(0x008a, self.wpa.anonce, &[], None);
                    self.send(now_us + 30_000, m1);
                    self.wpa.state = WpaState::AwaitingMessage2;
                }
            }
            (0, 12) | (0, 10) if to_us(&f[4..10]) => { self.state = StaState::Idle; }
            (2, _) if self.state == StaState::Associated && f[4..10] == self.cfg.bssid => {  // data to the DS
                if st == 4 || st == 12 { return None; }                                       // null frames (power save)
                let hdr = if st & 8 != 0 { 26 } else { 24 };
                // Protected frame: the MAC would have encrypted in place, so the descriptor holds
                // plaintext framed by an 8-byte CCMP header and 8 bytes of MIC space. Take those off.
                let plain;
                let f: &[u8] = if fc & 0x4000 != 0 && f.len() > hdr + 16 {
                    let mut v = Vec::with_capacity(f.len() - 16);
                    v.extend_from_slice(&f[..hdr]);
                    v.extend_from_slice(&f[hdr + 8..f.len() - 8]);
                    v[1] &= !0x40;                                                             // clear the protected bit
                    plain = v; &plain
                } else { f };
                if f.len() > hdr + 8 && f[hdr] == 0xaa && f[hdr + 6] == 0x88 && f[hdr + 7] == 0x8e {
                    self.on_eapol(&f[hdr + 8..], now_us);
                    return None;
                }
                self.stats.2 += 1; return Some(f.to_vec());
            }
            _ => {}
        }
        None
    }
    /// Build an EAPOL-Key frame (802.1X over LLC/SNAP in an 802.11 data frame from the DS).
    fn eapol(&mut self, key_info: u16, nonce: [u8; 32], key_data: &[u8], mic_key: Option<&[u8]>) -> Vec<u8> {
        let mut body = Vec::with_capacity(99 + key_data.len());
        body.push(2);                                                    // 802.1X-2004
        body.push(3);                                                    // EAPOL-Key
        body.extend_from_slice(&((95 + key_data.len()) as u16).to_be_bytes());
        body.push(2);                                                    // RSN key descriptor
        body.extend_from_slice(&key_info.to_be_bytes());
        body.extend_from_slice(&16u16.to_be_bytes());                    // key length (CCMP)
        body.extend_from_slice(&self.wpa.replay.to_be_bytes());
        body.extend_from_slice(&nonce);
        body.extend_from_slice(&[0u8; 16]);                              // key IV
        body.extend_from_slice(&[0u8; 8]);                               // key RSC
        body.extend_from_slice(&[0u8; 8]);                               // key ID
        let mic_at = body.len();
        body.extend_from_slice(&[0u8; 16]);
        body.extend_from_slice(&(key_data.len() as u16).to_be_bytes());
        body.extend_from_slice(key_data);
        if let Some(kck) = mic_key {
            let m = esp_periph::crypto::hmac_sha1(kck, &body);
            body[mic_at..mic_at + 16].copy_from_slice(&m[..16]);
        }
        let sta = self.sta; let bssid = self.cfg.bssid;
        let mut f = self.hdr(0x0208, &sta, &bssid);                      // data, from-DS
        f[16..22].copy_from_slice(&bssid);
        f.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
        f.extend_from_slice(&body);
        f
    }

    /// Handle an EAPOL-Key frame from the station (messages 2 and 4 of the handshake).
    fn on_eapol(&mut self, body: &[u8], now_us: u64) {
        if body.len() < 99 || body[1] != 3 { return; }
        // the MIC covers exactly the 802.1X frame; the 802.11 payload can carry trailing bytes
        let n = 4 + u16::from_be_bytes([body[2], body[3]]) as usize;
        let Some(body) = body.get(..n).filter(|b| b.len() >= 99) else { return; };
        let key_data_len = u16::from_be_bytes([body[97], body[98]]) as usize;
        if 99 + key_data_len != body.len() { return; }
        let key_info = u16::from_be_bytes([body[5], body[6]]);
        let has_mic = key_info & 0x0100 != 0;
        let secure = key_info & 0x0200 != 0;
        if !has_mic { return; }
        if !secure && self.wpa.state == WpaState::AwaitingMessage2 {
            // message 2: take the SNonce and derive the pairwise key
            let mut snonce = [0u8; 32]; snonce.copy_from_slice(&body[17..49]);
            let (aa, spa) = (self.cfg.bssid, self.sta);
            let (lo_mac, hi_mac) = if aa <= spa { (aa, spa) } else { (spa, aa) };
            let (an, sn) = (self.wpa.anonce, snonce);
            let (lo_n, hi_n) = if an <= sn { (an, sn) } else { (sn, an) };
            let mut data = Vec::with_capacity(76);
            data.extend_from_slice(&lo_mac); data.extend_from_slice(&hi_mac);
            data.extend_from_slice(&lo_n); data.extend_from_slice(&hi_n);
            let ptk = esp_periph::crypto::prf(&self.wpa.pmk, "Pairwise key expansion", &data, 384);
            // self-check: recompute the station's own MIC over message 2. If this matches, the PMK,
            // the PTK derivation and the MIC scope are all right and any later failure is elsewhere.
            {
                let mut probe = body.to_vec();
                let mic_at = 81;
                let mut recv = [0u8; 16]; recv.copy_from_slice(&probe[mic_at..mic_at + 16]);
                for b in probe[mic_at..mic_at + 16].iter_mut() { *b = 0; }
                let calc = esp_periph::crypto::hmac_sha1(&ptk[0..16], &probe);
                if self.log {
                    eprintln!("[wifi] WPA2 msg2: PTK derived, station MIC {} (recv {:02x?} calc {:02x?})",
                              if calc[..16] == recv { "VERIFIED" } else { "MISMATCH" }, &recv[..4], &calc[..4]);
                }
            }

            // message 3: RSN IE + the group key, wrapped with the KEK
            let mut kd = Vec::new();
            kd.extend_from_slice(RSN_IE);
            kd.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);   // GTK KDE, key id 1
            kd.extend_from_slice(&self.wpa.gtk);
            if kd.len() % 8 != 0 { kd.push(0xdd); while kd.len() % 8 != 0 { kd.push(0); } }   // pad: one 0xDD then zeros
            let mut kek = [0u8; 16]; kek.copy_from_slice(&ptk[16..32]);
            let wrapped = esp_periph::crypto::aes_key_wrap(&kek, &kd);
            self.wpa.replay += 1;
            let anonce = self.wpa.anonce;
            let m3 = self.eapol(0x13ca, anonce, &wrapped, Some(&ptk[..16]));
            self.send(now_us + 2_000, m3);
            self.wpa.state = WpaState::AwaitingMessage4;
        } else if secure && self.wpa.state == WpaState::AwaitingMessage4 {
            self.wpa.state = WpaState::Installed;
            if self.log { eprintln!("[wifi] WPA2 four-way handshake complete"); }
        }
    }

    /// Wrap an Ethernet frame from the network backend into an 802.11 data frame from the DS.
    pub fn data_from_ds(&mut self, eth: &[u8]) -> Option<Vec<u8>> {
        if self.state != StaState::Associated || eth.len() < 14 { return None; }
        let mut dst = [0u8; 6]; dst.copy_from_slice(&eth[0..6]); let mut src = [0u8; 6]; src.copy_from_slice(&eth[6..12]);
        let bssid = self.cfg.bssid;
        // Once the keys are installed the frame must look encrypted: protected bit, CCMP header and
        // room for the MIC. The payload stays in the clear — as far as firmware is concerned the MAC
        // decrypted it in place.
        let protected = if self.wpa.state == WpaState::Installed { 0x4000 } else { 0 };
        let ccmp_hdr = protected != 0;
        let mut f = self.hdr((2 << 2) | 0x0200 | protected, &dst, &src);                        // data, from-DS
        f[16..22].copy_from_slice(&src); f[10..16].copy_from_slice(&bssid);
        if ccmp_hdr {
            // CCMP header, as the hardware would leave it after decrypting in place
            self.pn += 1;
            let pn = self.pn;
            let keyid = if dst[0] & 1 != 0 { 1 } else { 0 };                                    // group frames use the GTK
            f.extend_from_slice(&[pn as u8, (pn >> 8) as u8, 0, 0x20 | (keyid << 6),
                                  (pn >> 16) as u8, (pn >> 24) as u8, (pn >> 32) as u8, (pn >> 40) as u8]);
        }
        f.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]); f.extend_from_slice(&eth[12..14]);  // LLC/SNAP
        f.extend_from_slice(&eth[14..]);
        if ccmp_hdr { f.extend_from_slice(&[0u8; 8]); }                                         // MIC space
        Some(f)
    }
}

/// 802.11 data frame (to the DS) -> Ethernet frame.
pub fn data_to_eth(f: &[u8]) -> Option<Vec<u8>> {
    if f.len() < 2 { return None; }
    let fc = u16::from_le_bytes([f[0], f[1]]); let st = (fc >> 4) & 0xf;
    let hdr = if st & 8 != 0 { 26 } else { 24 };
    if f.len() < hdr + 8 || f[hdr] != 0xaa { return None; }
    let mut e = Vec::with_capacity(f.len());
    e.extend_from_slice(&f[16..22]);   // dst = addr3
    e.extend_from_slice(&f[10..16]);   // src = addr2
    e.extend_from_slice(&f[hdr + 6..hdr + 8]);
    e.extend_from_slice(&f[hdr + 8..]);
    Some(e)
}

/// FCS (CRC-32) as the MAC appends it.
pub fn fcs(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data { crc ^= b as u32; for _ in 0..8 { crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 }; } }
    !crc
}

/// Which side of the air a frame belongs to: the virtual access point the guest joins as a
/// station, or the join to the access point the guest runs. The two roles are on the air at the
/// same time, so everything a frame needs to reach — the link that owns it and the network behind
/// that link — is tagged with its side.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Side {
    #[default]
    Ap,
    Join,
}

/// The station's side of the air, the same behind every chip's MAC. Both radio roles can be on the
/// air at once: the access point the guest joins as a station (`--wifi`) and the join to the access
/// point the guest itself runs (`--wifi-join`). That is the first-boot flow of firmware configured
/// through its own soft AP: the guest offers a scan, the user picks the emulated access point from
/// the list and saves it, and after the reboot the same run has the guest join it as a station.
///
/// Each role keeps its own network, because they are not the same link: behind the access point the
/// network is a server of DHCP, ARP, DNS and NTP, while on the join it is a client of the guest's
/// DHCP server and only the forward answers. The pacing of received frames and the counts for the
/// end-of-run report are shared — one radio hears one air. Each chip's bus moves frames between this
/// and its own descriptors; the descriptor and receive-header layouts are the chip's. The state is
/// private: the bus attaches the access point and the join through `SocBus::attach_wifi` and
/// `SocBus::attach_wifi_join`, which also refresh the chip's own scheduling of WiFi work.
#[derive(Default)]
pub struct StationLink {
    /// The virtual access point (`--wifi`).
    ap: Option<VirtualAp>,
    /// The network behind the access point: a server of DHCP, ARP, DNS and NTP.
    ap_net: Option<crate::net::VirtualNet>,
    /// The inverted air (`--wifi-join`): the guest runs the access point and this side is the
    /// station joining it.
    join: Option<join::Supplicant>,
    /// The join's network: a client of the guest's DHCP server, where only the forward answers.
    join_net: Option<crate::net::VirtualNet>,
    /// the host relays raw Ethernet (`set_ethernet_relay`) instead of the virtual network answering
    relay: bool,
    /// Ethernet frames the station sent, tagged with the side whose network they are for
    eth_tx: Vec<(Side, Vec<u8>)>,
    /// Ethernet frames for the station, from one of the two networks
    eth_rx: Vec<(Side, Vec<u8>)>,
    /// when the last frame went into the receive ring
    last_rx_us: u64,
    /// the descriptor that frame went into, 0 before the first one
    last_rx_desc: u32,
    /// which side delivered the frame in the ring last: the receive header reports the address
    /// match the guest's current role expects, and that follows the side the frame came from
    last_rx_side: Side,
    /// when the networks last read the host's sockets
    net_polled_us: u64,
    /// frames the station sent and received
    tx_frames: u64,
    rx_frames: u64,
    /// received frames with no usable descriptor
    rx_dropped: u64,
    /// relayed frames that were too long or found the relay queue full
    tx_dropped: u64,
}

/// A receive descriptor's `has_data` bit (word 0 bit 30), the same on the S3, C3 and C6 MACs: the
/// hardware filled it and software has not recycled it yet.
pub const RX_DESC_HAS_DATA: u32 = 1 << 30;
/// The longest Ethernet frame the relay takes or gives, without FCS, and the relay queues' depth.
const RELAY_FRAME_MAX: usize = 1518;
const RELAY_QUEUE: usize = 64;
/// Two received frames are never closer than a frame's airtime.
const RX_GAP_US: u64 = 400;
/// How long the air waits for software to recycle the last descriptor before it delivers anyway.
const RX_STALL_US: u64 = 50_000;
/// Frames from the station are handled the moment they are sent, but reading the host sockets
/// means syscalls: doing that every scheduling round costs more than emulating the CPU. This is
/// well under any timeout the guest's TCP stack cares about.
const NET_POLL_US: u64 = 500;

impl StationLink {
    /// Put an access point and the network behind it on the air, replacing any access point there
    /// was; the join to the guest's own access point stays, so both roles can be up at once. The
    /// queues, pacing and counts start again; the relay mode stays.
    pub fn attach(&mut self, ap: Option<VirtualAp>, net: Option<crate::net::VirtualNet>) {
        let (join, join_net) = (self.join.take(), self.join_net.take());
        *self = StationLink { ap, ap_net: net, join, join_net, relay: self.relay, ..Default::default() };
    }
    /// Put this side of the air on the guest's soft AP (`--wifi-join`) and the network backend
    /// behind it, replacing any join there was; a virtual access point the guest can join at the
    /// same time stays. The join's network is a client of the guest's DHCP server rather than a
    /// server of its own, and the forward rides that link.
    pub fn attach_join(&mut self, cfg: join::JoinConfig, nat: Option<crate::nat::Nat>, debug: &crate::DebugFlags) {
        let mut net = crate::net::VirtualNet::new(debug.has("net"));
        net.nat = nat;
        net.client = true;
        let supplicant = join::Supplicant::new(cfg, debug.has("wifi-frames"));
        net.client_mac = supplicant.sta_mac();
        let (ap, ap_net) = (self.ap.take(), self.ap_net.take());
        *self = StationLink { ap, ap_net, join: Some(supplicant), join_net: Some(net), relay: self.relay, ..Default::default() };
    }
    /// `attach` with a fresh access point for `cfg` and network, logging as `debug` says.
    pub fn attach_new(&mut self, cfg: ApConfig, nat: Option<crate::nat::Nat>, debug: &crate::DebugFlags) {
        let mut net = crate::net::VirtualNet::new(debug.has("net"));
        net.nat = nat;
        self.attach(Some(VirtualAp::new(cfg, debug.has("wifi-frames"))), Some(net));
    }
    /// What a guest reboot keeps: the host's access point, the join, their networks and the relay
    /// mode. The queues, the pacing and the counts start again, and a join hears the guest's access
    /// point afresh — it beacons again from scratch, so the association it had is gone. That is also
    /// how the receive header learns the guest changed roles: the guest is a station of the
    /// emulated access point now, not an access point itself.
    pub fn surviving_reboot(mut self) -> Self {
        for net in [&mut self.ap_net, &mut self.join_net].into_iter().flatten() {
            if let Some(fwd) = &mut net.hostfwd { fwd.reset_flows(); }
        }
        if let Some(j) = self.join.as_mut() { j.restart(); }
        StationLink { ap: self.ap, ap_net: self.ap_net, join: self.join, join_net: self.join_net, relay: self.relay, ..Default::default() }
    }

    pub fn ap(&self) -> Option<&VirtualAp> { self.ap.as_ref() }
    pub fn ap_mut(&mut self) -> Option<&mut VirtualAp> { self.ap.as_mut() }
    pub fn join(&self) -> Option<&join::Supplicant> { self.join.as_ref() }
    /// The network behind the virtual access point, when the guest is its station.
    pub fn ap_net(&self) -> Option<&crate::net::VirtualNet> { self.ap_net.as_ref() }
    /// The network of the join, when the guest runs the access point.
    pub fn join_net(&self) -> Option<&crate::net::VirtualNet> { self.join_net.as_ref() }
    /// The network the host side talks to: the access point's when there is one, the join's
    /// otherwise.
    pub fn net(&self) -> Option<&crate::net::VirtualNet> { self.ap_net().or_else(|| self.join_net()) }
    pub fn net_mut(&mut self) -> Option<&mut crate::net::VirtualNet> {
        if self.ap_net.is_some() { self.ap_net.as_mut() } else { self.join_net.as_mut() }
    }
    /// The network that owns the guest address `ip`: the access point's for its own subnet (the
    /// station's address and the gateway's), the join's for anything else — the address the guest's
    /// DHCP server leased it, and the guest's own addresses on that subnet. A host-side peer that
    /// names the guest address it wants (`--hostfwd`) picks its link with this.
    pub fn net_for_guest(&mut self, ip: [u8; 4]) -> Option<&mut crate::net::VirtualNet> {
        if self.ap_net.as_ref().is_some_and(|n| n.owns(ip)) { return self.ap_net.as_mut() }
        self.join_net.as_mut().or(self.ap_net.as_mut())
    }
    /// True while there is an air to schedule: a virtual access point, or the join to the guest's.
    pub fn on_air(&self) -> bool { self.ap.is_some() || self.join.is_some() }
    /// The channel the air is on, once it is on one.
    pub fn channel(&self) -> Option<u8> { self.ap.as_ref().map(|ap| ap.cfg.channel).or_else(|| self.join.as_ref().and_then(|j| j.channel())) }
    /// The address-filter bits the receive descriptor reports in its header word 0 (`rssi` is added
    /// by the chip). A station hears group traffic as "match 0" (bit 28) and frames addressed to it
    /// as both matches (bits 28 and 29) — measured on S3 silicon (`wDev_IndicateFrame` drops a
    /// unicast frame that claims only bit 28). An access point hears management frames from every
    /// station it has not seen yet, and its hardware reports those as matched by address alone:
    /// bit 29, without bit 28. Firmware in the AP role drops a frame that sets both, so a guest that
    /// runs an access point cannot be handed the station convention — measured with a softAP image,
    /// where bit 29 alone gets probe, authentication and association answered and both bits get
    /// nothing at all.
    ///
    /// With both roles on the air the guest's own mode decides, not the frame's origin: while the
    /// guest's access point beacons — it is the AP the join is attached to, or the frame came from
    /// that link — its MAC reports bit 29 alone, so the probe responses a scan in its setup UI needs
    /// are accepted. After the guest reboots as a station of the emulated access point the join
    /// hears no access point any more and the station convention applies again.
    pub fn rx_match(&self, group: bool) -> u32 {
        if self.guest_runs_ap() { return 1 << 29; }
        if group { 1 << 28 } else { (1 << 28) | (1 << 29) }
    }
    /// APSTA has two local addresses: classify unicast by the addressed interface.
    pub fn rx_match_frame(&self, frame: &[u8]) -> u32 {
        let group = frame.get(4).is_some_and(|b| b & 1 != 0);
        if !group && self.join.as_ref().and_then(|j| j.ap.as_ref()).is_some_and(|ap| frame.get(4..10) != Some(&ap.bssid)) {
            return (1 << 28) | (1 << 29);
        }
        self.rx_match(group)
    }
    /// True while the guest's own access point is on the air. A run with only the join is always
    /// that case — the guest is the AP by construction. With the access point role up as well the
    /// frame's link and the guest's beacons say so: a join-only frame comes from the guest as an
    /// access point, and once the join has heard the guest beacon since the last reboot it still is
    /// one. A guest that rebooted into a station of the emulated access point has cleared that.
    fn guest_runs_ap(&self) -> bool {
        let Some(j) = self.join.as_ref() else { return false };
        self.ap.is_none() || self.last_rx_side == Side::Join || j.ap.is_some()
    }
    /// Whether frames are being logged.
    pub fn log(&self) -> bool { self.ap.as_ref().is_some_and(|ap| ap.log) || self.join.as_ref().is_some_and(|j| j.log) }
    pub fn relay(&self) -> bool { self.relay }
    /// The descriptor the last received frame went into, 0 before the first one.
    pub fn last_rx_desc(&self) -> u32 { self.last_rx_desc }
    /// Ethernet frames the station sent that the network or the relay has not taken yet.
    pub fn eth_tx_pending(&self) -> usize { self.eth_tx.len() }
    pub fn tx_frames(&self) -> u64 { self.tx_frames }
    pub fn rx_frames(&self) -> u64 { self.rx_frames }
    pub fn rx_dropped(&self) -> u64 { self.rx_dropped }
    pub fn tx_dropped(&self) -> u64 { self.tx_dropped }
    /// The networks around (beacon-only, not joinable), replacing the ones there were; `None`
    /// without an access point. A network with the access point's own SSID or BSSID would be that
    /// access point, so it is left out: the ones left out come back with the reason.
    pub fn set_beacons(&mut self, beacons: Vec<BeaconAp>) -> Option<Vec<(BeaconAp, &'static str)>> {
        let ap = self.ap.as_mut()?;
        let mut left_out = Vec::new();
        ap.beacons = beacons.into_iter().filter_map(|b| {
            let why = if b.ssid == ap.cfg.ssid { "the access point's SSID" } else if b.bssid == ap.cfg.bssid { "the access point's BSSID" } else { return Some(b) };
            left_out.push((b, why));
            None
        }).collect();
        Some(left_out)
    }

    /// The side of the air a frame the station transmits belongs to, by its addresses. The emulated
    /// access point's BSSID in addr1 (or in addr3, for a data frame to the DS) means the guest is
    /// that access point's station. Everything else — what an access point transmits to its clients
    /// — belongs to the join. A broadcast management frame is the one both sides hear: a scan probe
    /// request goes to the access point, which answers it, and means nothing to the join.
    fn tx_side(&self, f: &[u8]) -> Option<Side> {
        if f.len() < 24 { return None; }
        let fc = u16::from_le_bytes([f[0], f[1]]);
        let (ty, st) = ((fc >> 2) & 3, (fc >> 4) & 0xf);
        let a1 = &f[4..10];
        if let Some(ap) = &self.ap {
            if a1 == ap.cfg.bssid || (ty == 2 && f.len() >= 22 && f[16..22] == ap.cfg.bssid) { return Some(Side::Ap); }
        }
        if self.join.is_some() {
            if ty == 0 && st == 4 && a1 == [0xff; 6] && self.ap.is_some() { return Some(Side::Ap); }
            return Some(Side::Join);
        }
        self.ap.as_ref().map(|_| Side::Ap)
    }

    /// The station sent `frame` at `now_us`: the link that owns it hears it, and what that link
    /// passes on as data goes to its network or to the relay.
    pub fn station_tx(&mut self, frame: &[u8], now_us: u64) {
        self.tx_frames += 1;
        let Some(side) = self.tx_side(frame) else { return };
        let eths = match side {
            Side::Join => self.join.as_mut().map(|j| j.on_guest_tx(frame, now_us)).unwrap_or_default(),
            Side::Ap => match self.ap.as_mut().and_then(|ap| ap.on_station_tx(frame, now_us)).and_then(|data| data_to_eth(&data)) {
                Some(eth) => vec![eth],
                None => Vec::new(),
            },
        };
        for eth in eths {
            if !self.relay || (eth.len() <= RELAY_FRAME_MAX && self.eth_tx.len() < RELAY_QUEUE) { self.eth_tx.push((side, eth)); } else { self.tx_dropped += 1; }
        }
    }

    /// True while the last received frame's airtime lasts: no frame is due, whatever the ring holds.
    pub fn in_rx_gap(&self, now_us: u64) -> bool { now_us.wrapping_sub(self.last_rx_us) < RX_GAP_US }

    /// True when `next_rx` has nothing to deliver at `now_us`, whatever the ring holds: within the
    /// airtime gap, or with both links idle and nothing from either network. `next_rx` would change
    /// nothing then, so skipping it is exact. The bus asks this every scheduling round while WiFi is
    /// on, before it reads the ring.
    #[inline]
    pub fn nothing_due(&self, now_us: u64) -> bool {
        if self.in_rx_gap(now_us) { return true; }
        if !self.eth_rx.is_empty() { return false; }
        let ap_idle = self.ap.as_ref().is_none_or(|ap| ap.idle_at(now_us));
        let join_idle = self.join.as_ref().is_none_or(|j| j.idle_at(now_us));
        ap_idle && join_idle
    }

    /// The frames the access point and its network have due at `now_us`: its own timers and
    /// responses, and what the network sends wrapped as frames from the DS. Sorted as they go into
    /// the ring, management before beacons so a connect exchange is not crowded out by beacon
    /// traffic. Frames that are not due stay in the access point's queue.
    fn due_from_ap(&mut self, now_us: u64) -> Vec<AirFrame> {
        // The join's frames are not this side's to deliver: they stay queued for their own link.
        let mut ap_frames = Vec::new();
        if !self.relay {
            let (take, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.eth_rx).into_iter().partition(|(s, _)| *s == Side::Ap);
            self.eth_rx = rest;
            ap_frames = take.into_iter().map(|(_, f)| f).collect();
        }
        let relay = self.relay;
        let Some(ap) = self.ap.as_mut() else { return Vec::new() };
        let mut due = ap.step(now_us);
        // In relay mode the host has the frames and the access point passes one on at a time, only
        // when it has nothing of its own to say.
        if relay && due.is_empty() {
            if let Some(i) = self.eth_rx.iter().position(|(s, _)| *s == Side::Ap) { ap_frames.push(self.eth_rx.remove(i).1); }
        }
        for e in ap_frames { if let Some(f) = ap.data_from_ds(&e) { due.push(AirFrame { at_us: now_us, frame: f }); } }
        due.sort_by_key(|a| (is_beacon(&a.frame), a.at_us));
        due
    }

    /// The frames the join and its network have due at `now_us`: its timers and the answered
    /// management and DHCP exchanges, and what the forward's flows carry, wrapped for the guest's
    /// access point.
    fn due_from_join(&mut self, now_us: u64) -> Vec<AirFrame> {
        let (eth_in, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.eth_rx).into_iter().partition(|(s, _)| *s == Side::Join);
        self.eth_rx = rest;
        let eth_in: Vec<Vec<u8>> = eth_in.into_iter().map(|(_, f)| f).collect();
        let Some(j) = self.join.as_mut() else { return Vec::new() };
        let mut due = j.step(now_us);
        for e in eth_in { if let Some(f) = j.wrap_eth(&e) { due.push(AirFrame { at_us: now_us, frame: f }); } }
        due.sort_by_key(|a| a.at_us);
        due
    }

    /// The next frame for the station's receive ring, if one is due: from whichever link has the
    /// earlier one, management before beacons, then what the networks send. `ring_busy` says the
    /// last delivered descriptor still has data. The S3's library was seen to indicate a frame up
    /// the 802.11 stack only while the descriptor ring is shallow: with several filled descriptors
    /// pending it switches to batch block-recycle and drops them. So the air waits until software
    /// has recycled the last one, which is what a real radio sees at low traffic, but not for ever:
    /// after `RX_STALL_US` the frame goes anyway, as a real ring would overflow. The C3 and C6 are
    /// paced the same way; their station goldens pass with it, but it was not measured on their
    /// silicon. The caller delivers the frame and then reports `rx_delivered` or `drop_rx`.
    pub fn next_rx(&mut self, now_us: u64, ring_busy: bool) -> Option<Vec<u8>> {
        if self.in_rx_gap(now_us) { return None; }
        if ring_busy && now_us.wrapping_sub(self.last_rx_us) < RX_STALL_US { return None; }
        let mut ap = self.due_from_ap(now_us);
        let mut join = self.due_from_join(now_us);
        let take_join = match (ap.first(), join.first()) {
            (Some(a), Some(j)) => (j.at_us, false) < (a.at_us, is_beacon(&a.frame)),
            (None, Some(_)) => true,
            _ => false,
        };
        let (side, frame) = if take_join {
            let first = join.remove(0);
            (Side::Join, first.frame)
        } else {
            let first = ap.first().map(|a| a.frame.clone())?;
            ap.remove(0);
            (Side::Ap, first)
        };
        if let Some(a) = self.ap.as_mut() { a.queue.extend(ap); }
        if let Some(j) = self.join.as_mut() { j.requeue(join); }
        self.last_rx_us = now_us;
        self.last_rx_side = side;
        Some(frame)
    }

    /// A frame went into the descriptor at `desc`.
    pub fn rx_delivered(&mut self, desc: u32) { self.last_rx_desc = desc; self.rx_frames += 1; }
    /// A frame found no usable descriptor.
    pub fn drop_rx(&mut self) { self.rx_dropped += 1; }

    fn net_mut_side(&mut self, side: Side) -> Option<&mut crate::net::VirtualNet> {
        match side {
            Side::Ap => self.ap_net.as_mut(),
            Side::Join => self.join_net.as_mut(),
        }
    }

    /// Each network answers what its own link sent at once and reads the host's sockets every
    /// `NET_POLL_US`. Nothing to do in relay mode: the host has the frames.
    pub fn net_step(&mut self, now_us: u64) {
        if self.relay { return; }
        if let (Some(ap), Some(fwd)) = (&self.ap, self.ap_net.as_mut().and_then(|n| n.hostfwd.as_mut())) {
            fwd.set_station_mac(ap.sta);
        }
        if let (Some(join), Some(fwd)) = (&self.join, self.join_net.as_mut().and_then(|n| n.hostfwd.as_mut())) {
            if let (Some(ap), Some(lease)) = (&join.ap, join.lease) {
                fwd.set_client_link(ap.bssid, lease.ip, join.sta_mac());
            }
        }

        let due = now_us.wrapping_sub(self.net_polled_us) >= NET_POLL_US;
        if self.eth_tx.is_empty() && !due { return; }
        if due { self.net_polled_us = now_us; }
        for (side, e) in std::mem::take(&mut self.eth_tx) {
            let Some(net) = self.net_mut_side(side) else { continue };
            let out = net.handle(&e, now_us);
            self.eth_rx.extend(out.into_iter().map(|f| (side, f)));
        }
        for side in [Side::Ap, Side::Join] {
            let Some(net) = self.net_mut_side(side) else { continue };
            let out = net.poll(now_us);
            self.eth_rx.extend(out.into_iter().map(|f| (side, f)));
        }
    }

    /// Switch the host's Ethernet relay on or off; a change empties both queues.
    pub fn set_relay(&mut self, enabled: bool) {
        if self.relay != enabled { self.eth_tx.clear(); self.eth_rx.clear(); self.relay = enabled; }
    }

    /// The frames the station sent since the last call, in relay mode.
    pub fn take_relay_frames(&mut self) -> Vec<Vec<u8>> {
        if !self.relay { return Vec::new(); }
        let (ap, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.eth_tx).into_iter().partition(|(s, _)| *s == Side::Ap);
        self.eth_tx = rest;
        ap.into_iter().map(|(_, f)| f).collect()
    }

    /// A frame from the host for the station, in relay mode.
    pub fn receive_relay_frame(&mut self, frame: &[u8]) -> Result<(), String> {
        if !self.relay || self.ap.is_none() { return Err("Ethernet relay requires relay mode and a virtual AP".into()); }
        if !(14..=RELAY_FRAME_MAX).contains(&frame.len()) { return Err(format!("Ethernet frame must be 14..={RELAY_FRAME_MAX} bytes without FCS")); }
        if self.eth_rx.len() >= RELAY_QUEUE { return Err("Ethernet receive queue full".into()); }
        self.eth_rx.push((Side::Ap, frame.to_vec()));
        Ok(())
    }

    /// The end-of-run lines, one per line without a final newline: what the station sent and
    /// received, what the access point saw, what the join did, and what each network answered.
    /// Empty when the station never used the radio and there is no network. A run with one role
    /// names the networks as it always did; with both roles on the air the join's line says so.
    pub fn report(&self) -> String {
        let mut lines = Vec::new();
        if self.tx_frames + self.rx_frames > 0 { lines.push(format!("[emu] wifi: {} frames sent by the station, {} received ({} dropped: no descriptor){}", self.tx_frames, self.rx_frames, self.rx_dropped, self.ap().map_or(String::new(), |ap| format!("; AP: {} beacons, {} probe responses, {} data frames from the station, state {:?}", ap.stats.0, ap.stats.1, ap.stats.2, ap.state)))); }
        if let Some(j) = &self.join { lines.push(j.report()); }
        let both = self.ap.is_some() && self.join.is_some();
        for side in [Side::Ap, Side::Join] {
            let net = match side { Side::Ap => self.ap_net(), Side::Join => self.join_net() };
            let Some(n) = net else { continue };
            let label = if both && side == Side::Join { " (join)" } else { "" };
            lines.push(format!("[emu] net{label}: {} DHCP leases, {} ARP replies, {} DNS answers, {} NTP answers, {} TCP refused, {} pings, {} frames ignored", n.dhcp_acks, n.arp_replies, n.dns_answers, n.ntp_answers, n.tcp_rejects, n.pings, n.unhandled));
            if let Some(t) = &n.nat { lines.push(format!("[emu] nat{label}: {} TCP connections ({} failed), {} UDP flows ({} evicted, {} send errors), {} bytes out, {} bytes in", t.tcp_opened, t.tcp_refused, t.udp_flows, t.udp_evicted, t.udp_send_errors, t.bytes_to_host, t.bytes_to_guest)); }
            if let Some(f) = &n.hostfwd { if let Some(line) = f.report() { lines.push(line); } }
        }
        lines.join("\n")
    }
}
