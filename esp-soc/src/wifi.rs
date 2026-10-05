//! The virtual air interface: 802.11 frame helpers and a minimal access point the emulated MAC
//! "hears". The AP beacons, answers probe requests, and completes open-system authentication and
//! association; data frames are handed to the network backend (docs/networking-plan.md).

#[cfg(test)]
mod tests;

pub fn mac_str(m: &[u8]) -> String { m.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(":") }

fn ies(f: &[u8], body: usize) -> Vec<(u8, &[u8])> {
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
                "bssid" => {
                    let invalid = || format!("invalid WiFi BSSID '{value}': expected six hexadecimal octets");
                    let mut octets = value.split(':');
                    for byte in &mut cfg.bssid {
                        let octet = octets.next().filter(|s| s.len() == 2 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                            .ok_or_else(invalid)?;
                        *byte = u8::from_str_radix(octet, 16).map_err(|_| invalid())?;
                    }
                    if octets.next().is_some() { return Err(invalid()); }
                }
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
                    queue: Vec::new(), pn: 0, log, stats: (0, 0, 0) }
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
    fn send(&mut self, at_us: u64, frame: Vec<u8>) {
        if self.log { let d = describe(&frame); if d.contains("auth") || d.contains("assoc") || d.contains("888e") { eprintln!("[wifi] AP -> {}  hex={:02x?}", d, frame); } else { eprintln!("[wifi] AP -> {} (t+{} us)", d, at_us); } }
        self.queue.push(AirFrame { at_us, frame });
    }
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

/// The end-of-run lines for a chip's WiFi: what the station sent and received, what the access
/// point saw, and what the network behind it answered. Empty when the station never used the radio
/// and there is no network.
pub fn report(tx_frames: u64, rx_frames: u64, rx_dropped: u64, ap: Option<&VirtualAp>, net: Option<&crate::net::VirtualNet>) -> String {
    let mut s = String::new();
    if tx_frames + rx_frames > 0 { s += &format!("[emu] wifi: {} frames sent by the station, {} received ({} dropped: no descriptor){}\n", tx_frames, rx_frames, rx_dropped, ap.map_or(String::new(), |ap| format!("; AP: {} beacons, {} probe responses, {} data frames from the station, state {:?}", ap.stats.0, ap.stats.1, ap.stats.2, ap.state))); }
    if let Some(n) = net { s += &format!("[emu] net: {} DHCP leases, {} ARP replies, {} DNS answers, {} NTP answers, {} TCP refused, {} pings, {} frames ignored\n", n.dhcp_acks, n.arp_replies, n.dns_answers, n.ntp_answers, n.tcp_rejects, n.pings, n.unhandled);
        if let Some(t) = &n.nat { s += &format!("[emu] nat: {} TCP connections ({} failed), {} UDP flows ({} evicted, {} send errors), {} bytes out, {} bytes in\n", t.tcp_opened, t.tcp_refused, t.udp_flows, t.udp_evicted, t.udp_send_errors, t.bytes_to_host, t.bytes_to_guest); } }
    s
}
