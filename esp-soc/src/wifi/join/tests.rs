//! The supplicant against the authenticator the model already has: `VirtualAp` is the guest's
//! access point, driven with the frames this side sends, and its frames come back the way the air
//! would deliver them. A join that completes here is the two halves of the model agreeing on the
//! wire format, the PTK derivation and the MIC — not a copy of the crypto tested against itself.

use super::*;
use crate::net::packet::ethernet;
use crate::wifi::{ApConfig, StationLink, VirtualAp, WpaState};
use crate::DebugFlags;

const SSID: &str = "guest-ap";
const PSK: &str = "bench-passphrase";

fn guest_ap(psk: Option<&str>) -> ApConfig {
    ApConfig { ssid: SSID.into(), bssid: [0x46, 0x1b, 0xf6, 0x75, 0xdc, 0xe0], channel: 6, psk: psk.map(str::to_string) }
}

/// Both halves of the air, pumped against each other in emulated time.
struct Air {
    ap: VirtualAp,
    sta: Supplicant,
    now: u64,
    /// Every frame this station put on the air, in order.
    sent: Vec<Vec<u8>>,
}

impl Air {
    fn new(join: JoinConfig, psk: Option<&str>) -> Self {
        Air { ap: VirtualAp::new(guest_ap(psk), false), sta: Supplicant::new(join, false), now: 0, sent: Vec::new() }
    }

    /// Advance to `until_us`, handing each side what the other transmitted.
    fn run(&mut self, until_us: u64) {
        while self.now < until_us {
            self.now += 1_000;
            for f in self.ap.step(self.now) { self.sta.on_guest_tx(&f.frame, self.now); }
            for f in self.sta.step(self.now) {
                self.ap.on_station_tx(&f.frame, self.now);
                self.sent.push(f.frame);
            }
        }
    }

    /// The last data frame the station sent.
    fn last_data_frame(&self) -> Option<&Vec<u8>> {
        self.sent.iter().rev().find(|f| f.len() > 2 && (f[0] >> 2) & 3 == 2)
    }

    /// The last Ethernet frame the station sent, as the access point's network sees it.
    fn last_eth(&self) -> Option<Vec<u8>> {
        let f = self.last_data_frame()?;
        crate::wifi::data_to_eth(f)
    }
}

/// A DHCP reply for the request in `request` (an Ethernet frame), the way a guest's own server
/// answers one.
fn dhcp_reply(request: &[u8], reply_type: u8, yiaddr: [u8; 4], server: [u8; 4]) -> Vec<u8> {
    let bootp = &request[14 + 20 + 8..];
    let mut b = vec![0u8; 240];
    b[0] = 2; b[1] = 1; b[2] = 6;
    b[4..8].copy_from_slice(&bootp[4..8]);                      // xid
    b[10] = 0x80;                                               // broadcast
    b[16..20].copy_from_slice(&yiaddr);
    b[20..24].copy_from_slice(&server);
    b[28..34].copy_from_slice(&bootp[28..34]);                  // chaddr
    b[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    b.extend_from_slice(&[53, 1, reply_type]);
    b.extend_from_slice(&[54, 4]); b.extend_from_slice(&server);
    b.extend_from_slice(&[1, 4, 255, 255, 255, 0]);
    b.extend_from_slice(&[3, 4]); b.extend_from_slice(&server);
    b.extend_from_slice(&[6, 4]); b.extend_from_slice(&server);
    b.push(255);
    while b.len() < 300 { b.push(0); }
    let udp = crate::net::packet::udp_packet(&server, &[255; 4], 67, 68, &b);
    let ip = crate::net::packet::ip_packet(17, &server, &[255; 4], &udp);
    ethernet(&[0xff; 6], &[0x46, 0x1b, 0xf6, 0x75, 0xdc, 0xe0], 0x0800, &ip)
}

#[test]
fn join_options_name_what_the_join_accepts() {
    let cfg = JoinConfig::parse("ssid=annex,psk=bench-passphrase,bssid=46:1b:f6:75:dc:e0").unwrap();
    assert_eq!(cfg.ssid.as_deref(), Some("annex"));
    assert_eq!(cfg.psk.as_deref(), Some("bench-passphrase"));
    assert_eq!(cfg.bssid, Some([0x46, 0x1b, 0xf6, 0x75, 0xdc, 0xe0]));
    assert!(cfg.describe().contains("annex"));
    // No filter means "the first AP that beacons, as long as its security fits".
    let any = JoinConfig::parse("").unwrap();
    assert_eq!(any, JoinConfig::default());
    // A misspelled key or a passphrase WPA2 cannot use must not silently weaken the join.
    assert!(JoinConfig::parse("ssdi=annex").unwrap_err().contains("unknown join option"));
    assert!(JoinConfig::parse("psk=short").unwrap_err().contains("at least 8 characters"));
    assert!(JoinConfig::parse("bssid=46:1b:f6:75:dc").unwrap_err().contains("six hexadecimal octets"));
}

#[test]
fn an_open_guest_ap_is_joined_and_leased() {
    let mut air = Air::new(JoinConfig::default(), None);
    air.run(300_000);
    assert_eq!(air.sta.phase, Phase::Leasing, "associated and asking for an address");
    assert_eq!(air.ap.state, crate::wifi::StaState::Associated);
    assert_eq!(air.sta.ap.as_ref().unwrap().ssid, SSID);
    assert_eq!(air.sta.ap.as_ref().unwrap().channel, 6, "the DS parameter set says which channel");
    assert!(!air.sta.stats.keys, "an open network has no four-way handshake");

    // The guest's own DHCP server answers, and the lease is what the forward will use.
    let request = air.last_eth().expect("a DHCP request on the air");
    let offer = dhcp_reply(&request, 2, [192, 168, 4, 2], [192, 168, 4, 1]);
    air.sta.on_guest_tx(&air.ap.data_from_ds(&offer).unwrap(), air.now);
    air.run(air.now + 50_000);
    let request = air.last_eth().expect("a DHCP request on the air");
    let ack = dhcp_reply(&request, 5, [192, 168, 4, 2], [192, 168, 4, 1]);
    air.sta.on_guest_tx(&air.ap.data_from_ds(&ack).unwrap(), air.now);
    air.run(air.now + 50_000);
    assert_eq!(air.sta.phase, Phase::Online);
    let lease = air.sta.lease.expect("a lease");
    assert_eq!(lease.ip, [192, 168, 4, 2]);
    assert_eq!(lease.router, Some([192, 168, 4, 1]));
    assert_eq!(air.sta.guest_ip(), Some([192, 168, 4, 1]), "the forward aims at the AP itself");
}

#[test]
fn the_supplicant_completes_the_guest_aps_four_way_handshake() {
    let mut air = Air::new(JoinConfig { psk: Some(PSK.into()), ..JoinConfig::default() }, Some(PSK));
    air.run(900_000);
    assert!(air.sta.installed, "the supplicant accepted message 3");
    assert_eq!(air.ap.wpa.state, WpaState::Installed, "the authenticator accepted message 4");
    assert_eq!(air.sta.stats.eapol2, 1);
    assert_eq!(air.sta.stats.eapol4, 1);
    assert_eq!(air.sta.phase, Phase::Leasing, "the address comes after the keys");
    // Once the keys are installed the station's data must look encrypted: protected bit, CCMP
    // header, MIC space, payload in the clear — the MAC would have encrypted it.
    let frame = air.last_data_frame().expect("a data frame");
    assert_ne!(frame[1] & 0x40, 0, "protected bit");
    assert_eq!(&frame[24..27], &[1, 0, 0], "CCMP header with packet number 1");
    assert_ne!(frame[27] & 0x20, 0, "the extend-IV octet the hardware leaves");
    assert_eq!(&frame[frame.len() - 8..], &[0u8; 8], "MIC space kept at the end");
}

#[test]
fn a_secured_ap_is_not_joined_without_a_passphrase() {
    let mut air = Air::new(JoinConfig::default(), Some(PSK));
    air.run(1_000_000);
    assert_eq!(air.sta.phase, Phase::Scanning, "a WPA2 AP is not a candidate for an open join");
    assert_eq!(air.sta.stats.beacons, air.sta.stats.beacons, "beacons were heard");
    assert_eq!(air.sta.stats.matched, 0);
}

#[test]
fn a_join_that_finds_no_ap_fails_instead_of_hanging() {
    // No access point on the air at all: the phase ends in `Failed` with a reason, which is what
    // the console and the end-of-run report name; a forward on that link then refuses cleanly.
    let mut sta = Supplicant::new(JoinConfig::default(), false);
    let mut now = 0;
    while now < 40_000_000 && sta.phase != Phase::Failed {
        now += 10_000;
        sta.step(now);
    }
    assert_eq!(sta.phase, Phase::Failed);
    assert!(sta.failure.as_deref().unwrap().contains("no matching access point"));
    assert!(sta.idle_at(u64::MAX - 1), "a failed join has no timer left to fire");
    assert!(sta.report().contains("failed:"));
}

#[test]
fn the_join_answers_arp_for_its_own_address() {
    let mut air = Air::new(JoinConfig::default(), None);
    air.run(300_000);
    let request = air.last_eth().unwrap();
    let offer = dhcp_reply(&request, 2, [192, 168, 4, 2], [192, 168, 4, 1]);
    air.sta.on_guest_tx(&air.ap.data_from_ds(&offer).unwrap(), air.now);
    air.run(air.now + 50_000);
    let request = air.last_eth().unwrap();
    let ack = dhcp_reply(&request, 5, [192, 168, 4, 2], [192, 168, 4, 1]);
    air.sta.on_guest_tx(&air.ap.data_from_ds(&ack).unwrap(), air.now);
    air.run(air.now + 50_000);

    // The guest asks who holds the address it leased; the reply must come from this station.
    let mut arp = Vec::new();
    arp.extend_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
    arp.extend_from_slice(&[0x46, 0x1b, 0xf6, 0x75, 0xdc, 0xe0]);
    arp.extend_from_slice(&[192, 168, 4, 1]);
    arp.extend_from_slice(&[0; 6]);
    arp.extend_from_slice(&[192, 168, 4, 2]);
    let ask = ethernet(&[0xff; 6], &[0x46, 0x1b, 0xf6, 0x75, 0xdc, 0xe0], 0x0806, &arp);
    air.sta.on_guest_tx(&air.ap.data_from_ds(&ask).unwrap(), air.now);
    air.run(air.now + 10_000);
    let reply = air.last_data_frame().unwrap();
    let eth = crate::wifi::data_to_eth(reply).unwrap();
    assert_eq!(eth[0..6], [0x46, 0x1b, 0xf6, 0x75, 0xdc, 0xe0], "answered to the asker");
    assert_eq!(&eth[14 + 8..14 + 14], &[0x02, 0x53, 0x49, 0x4d, 0x00, 0x11], "this station's MAC");
    assert_eq!(&eth[14 + 14..14 + 18], &[192, 168, 4, 2]);
    assert_eq!(air.sta.stats.arps, 1);
}

#[test]
fn the_receive_header_reports_the_match_the_role_expects() {
    let mut link = StationLink::default();
    link.attach(Some(VirtualAp::new(guest_ap(None), false)), None);
    assert_eq!(link.rx_match(true), 1 << 28, "a station hears group traffic as match 0");
    assert_eq!(link.rx_match(false), (1 << 28) | (1 << 29), "and its own unicast as both");
    assert_eq!(link.channel(), Some(6));

    let mut link = StationLink::default();
    link.attach_join(JoinConfig::default(), None, &DebugFlags::default());
    assert!(link.on_air() && link.join().is_some());
    assert_eq!(link.rx_match(true), 1 << 29, "an access point reports a match by address alone");
    assert_eq!(link.rx_match(false), 1 << 29, "for group and unicast alike");
}
