//! Both radio roles on one air: the access point the guest joins as a station (`--wifi`) and the
//! join to the access point the guest itself runs (`--wifi-join`). The guest's own access point is
//! modelled with the `VirtualAp` the join branch already has, so each half is driven with real
//! frames the way the air would carry them, and what the guest's MAC would accept is read off
//! `rx_match`. The flow these tests stand in for: a firmware in its setup-AP mode offers a scan, the
//! user picks the emulated access point from the list, saves it, and the guest comes back as that
//! access point's station — in the same run.

use super::*;
use crate::net::packet::{ethernet, ip_packet, udp_packet};
use crate::wifi::join::JoinConfig;
use crate::DebugFlags;

const HOST_SSID: &str = "bench";
const HOST_PSK: &str = "bench-password";
/// The guest's own access point, and its MAC — the address every frame it transmits carries.
const GUEST_MAC: [u8; 6] = [0x46, 0x1b, 0xf6, 0x75, 0xdc, 0xe0];

fn host_ap() -> ApConfig {
    ApConfig { ssid: HOST_SSID.into(), bssid: [0x02, 0x53, 0x49, 0x4d, 0x00, 0x01], channel: 6, psk: Some(HOST_PSK.into()) }
}

fn guest_ap() -> ApConfig {
    ApConfig { ssid: "guest-setup".into(), bssid: GUEST_MAC, channel: 11, psk: None }
}

/// A link with both roles on the air, and the guest's own access point as the source of what it
/// transmits in that role.
fn both_roles() -> (StationLink, VirtualAp) {
    let mut link = StationLink::default();
    link.attach(Some(VirtualAp::new(host_ap(), false)), Some(crate::net::VirtualNet::new(false)));
    link.attach_join(JoinConfig::default(), None, &DebugFlags::default());
    (link, VirtualAp::new(guest_ap(), false))
}

fn broadcast_mgmt(subtype: u16, from: [u8; 6]) -> Vec<u8> {
    let mut f = vec![0u8; 24];
    f[0] = (subtype << 4) as u8;
    f[4..10].copy_from_slice(&[0xff; 6]);
    f[10..16].copy_from_slice(&from);
    f[16..22].copy_from_slice(&[0xff; 6]);
    f
}

/// A scan probe request: broadcast, with the SSID the station is looking for (empty is a wildcard).
fn probe_request(from: [u8; 6], ssid: &str) -> Vec<u8> {
    let mut f = broadcast_mgmt(4, from);
    f.extend_from_slice(&[0u8; 8]);                                  // timestamp
    f.extend_from_slice(&100u16.to_le_bytes());                       // beacon interval
    f.extend_from_slice(&0x0001u16.to_le_bytes());                    // ESS
    f.push(0); f.push(ssid.len() as u8); f.extend_from_slice(ssid.as_bytes());
    f.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
    f
}

fn auth_request(bssid: [u8; 6], from: [u8; 6]) -> Vec<u8> {
    let mut f = Vec::with_capacity(30);
    f.extend_from_slice(&(11u16 << 4).to_le_bytes());
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(&bssid); f.extend_from_slice(&from); f.extend_from_slice(&bssid);
    f.extend_from_slice(&0u16.to_le_bytes());
    f.extend_from_slice(&[0, 0, 1, 0, 0, 0]);                        // open system, sequence 1, success
    f
}

fn assoc_request(bssid: [u8; 6], from: [u8; 6]) -> Vec<u8> {
    let mut f = Vec::with_capacity(64);
    f.extend_from_slice(&0u16.to_le_bytes());                        // association request
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(&bssid); f.extend_from_slice(&from); f.extend_from_slice(&bssid);
    f.extend_from_slice(&0u16.to_le_bytes());
    f.extend_from_slice(&0x0411u16.to_le_bytes());                    // ESS, short slot, no privacy
    f.extend_from_slice(&10u16.to_le_bytes());                        // listen interval
    f.push(0); f.push(HOST_SSID.len() as u8); f.extend_from_slice(HOST_SSID.as_bytes());
    f.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
    f
}

/// A data frame to the DS carrying `eth`, as the station of the emulated access point sends it.
fn data_to_ds(bssid: [u8; 6], from: [u8; 6], eth: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(32 + eth.len());
    f.extend_from_slice(&0x0108u16.to_le_bytes());                    // data, to the DS
    f.extend_from_slice(&[0, 0]);
    f.extend_from_slice(&bssid); f.extend_from_slice(&from); f.extend_from_slice(&eth[0..6]);
    f.extend_from_slice(&0u16.to_le_bytes());
    f.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
    f.extend_from_slice(&eth[12..14]);
    f.extend_from_slice(&eth[14..]);
    f
}

/// A BOOTP request from a client without an address, the way a station asks for its lease.
fn dhcp_message(mac: [u8; 6], msg_type: u8) -> Vec<u8> {
    let mut b = vec![0u8; 240];
    b[0] = 1; b[1] = 1; b[2] = 6;
    b[4..8].copy_from_slice(&0x5350_0001u32.to_be_bytes());
    b[10] = 0x80;
    b[28..34].copy_from_slice(&mac);
    b[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    b.extend_from_slice(&[53, 1, msg_type]);                          // DHCPDISCOVER or DHCPREQUEST
    b.push(255);
    while b.len() < 300 { b.push(0); }
    let udp = udp_packet(&[0; 4], &[255; 4], 68, 67, &b);
    let ip = ip_packet(17, &[0; 4], &[255; 4], &udp);
    ethernet(&[0xff; 6], &mac, 0x0800, &ip)
}

/// The next data frame the air delivers at or after `from_us`, skipping management and EAPOL.
fn next_data(link: &mut StationLink, from_us: u64) -> Option<Vec<u8>> {
    for t in (from_us..from_us + 100_000).step_by(1_000) {
        let Some(frame) = link.next_rx(t, false) else { continue };
        link.rx_delivered(0x1000);
        if (frame[0] >> 2) & 3 == 2 { return Some(frame) }
    }
    None
}

#[test]
fn both_radio_roles_are_on_the_air_each_with_its_own_network() {
    let mut link = both_roles().0;
    assert!(link.ap().is_some() && link.join().is_some(), "both roles at once");
    assert!(link.on_air());
    assert_eq!(link.channel(), Some(6), "the access point's channel is the air's");
    assert_eq!(link.ap_net().map(|n| n.client), Some(false), "behind the access point the network is the server");
    assert_eq!(link.join_net().map(|n| n.client), Some(true), "on the join it is the guest's DHCP client");
    assert!(link.net().is_some_and(|n| !n.client), "the host side talks to the access point's network");
    assert!(link.net_for_guest([10, 0, 2, 15]).is_some_and(|n| !n.client), "the station's address belongs to the access point");
    assert!(link.net_for_guest([192, 168, 4, 1]).is_some_and(|n| n.client), "a guest address on the join's subnet belongs to the join");
}

#[test]
fn a_guest_scanning_while_it_runs_its_own_access_point_sees_the_emulated_one() {
    let (mut link, mut guest) = both_roles();
    // The guest's own access point beacons: the join hears it, so the guest's MAC is in the AP role.
    for f in guest.step(200_000) { link.station_tx(&f.frame, 200_000); }
    assert!(link.join().is_some_and(|j| j.ap.is_some()), "the join heard the guest's own access point");

    // Its setup UI scans: a wildcard probe request on the air.
    let probe = probe_request(GUEST_MAC, "");
    link.station_tx(&probe, 300_000);
    // The join answers the guest's access point on its own link; the scan is what the access point
    // role answers, so walk the frames delivered from both until the probe response arrives.
    let mut found = None;
    for t in (302_000..=320_000).step_by(1_000) {
        let Some(frame) = link.next_rx(t, false) else { continue };
        let delivered = link.rx_match((frame[4] & 1) == 1);
        if (frame[0] >> 4) & 0xf == 5 {
            found = Some((frame, delivered));
            break;
        }
        link.rx_delivered(0x1000);
    }
    let (response, delivered) = found.expect("the access point answers a probe request");
    assert_eq!(&response[4..10], &GUEST_MAC, "addressed to the station that asked");
    let mut ssid = String::new();
    let mut channel = 0u8;
    let mut rsn = false;
    for (id, d) in ies(&response, 36) {
        match id {
            0 => ssid = String::from_utf8_lossy(d).into_owned(),
            3 => channel = d[0],
            48 => rsn = true,
            _ => {}
        }
    }
    assert_eq!(ssid, HOST_SSID, "the scan lists the SSID `--wifi` configured");
    assert_eq!(channel, host_ap().channel, "on the channel `--wifi` configured");
    assert!(rsn, "and with the security it configured");
    assert_eq!(delivered, 1 << 29, "a guest in the AP role accepts an address match alone");

    // The beacon set carries the same SSID, and it is delivered under the same verdict.
    let beacon = link.next_rx(310_000, false).expect("the access point beacons as well");
    assert!(is_beacon(&beacon), "{}", describe(&beacon));
    assert_eq!(link.rx_match(true), 1 << 29);
}

#[test]
fn a_frame_goes_to_the_link_its_addresses_belong_to() {
    let (mut link, mut guest) = both_roles();
    // A station of the emulated access point: authentication addressed to its BSSID.
    link.station_tx(&auth_request(host_ap().bssid, GUEST_MAC), 1_000);
    assert_eq!(link.ap().map(|ap| ap.state), Some(StaState::Authenticated), "the access point role heard it");
    assert!(link.join().is_some_and(|j| j.stats.beacons == 0), "the join saw nothing of it");

    // The guest as an access point: its beacon belongs to the join.
    for f in guest.step(200_000) { link.station_tx(&f.frame, 200_000); }
    assert!(link.join().is_some_and(|j| j.ap.is_some()), "the join heard the guest's access point");
    assert_eq!(link.ap().map(|ap| ap.state), Some(StaState::Authenticated), "and the access point role kept its station");
}

#[test]
fn the_station_of_the_access_point_takes_its_lease_from_the_emulated_network() {
    let (mut link, _guest) = both_roles();
    let bssid = host_ap().bssid;
    link.station_tx(&auth_request(bssid, GUEST_MAC), 1_000);
    link.station_tx(&assoc_request(bssid, GUEST_MAC), 2_000);
    assert_eq!(link.ap().map(|ap| ap.state), Some(StaState::Associated), "associated to the emulated access point");

    link.station_tx(&data_to_ds(bssid, GUEST_MAC, &dhcp_message(GUEST_MAC, 1)), 3_000);
    link.net_step(3_000);
    assert_eq!(link.eth_tx_pending(), 0, "the access point's network took the request at once");
    assert!(link.join_net().is_some_and(|n| n.unhandled == 0 && n.dhcp_acks == 0), "the join's network saw none of it");

    // The access point's own answers (authentication, association) come first; the lease is the
    // data frame behind them.
    let offer = next_data(&mut link, 4_000).expect("the offer comes back up the same link");
    assert_eq!(link.rx_match(false), (1 << 28) | (1 << 29), "the guest is a station here: both matches for its unicast");
    let eth = data_to_eth(&offer).expect("the offer is an Ethernet frame");
    let bootp = &eth[14 + 20 + 8..];
    assert_eq!(bootp[0], 2, "a BOOTP reply: {}", describe(&offer));
    assert_eq!(&bootp[16..20], &[10, 0, 2, 15], "the access point's network offers its station address");
    assert_eq!(link.rx_match(true), 1 << 28, "the guest is a station here: bit 28 for a group frame");

    // A lease is what the guest asked for: the request is answered with the access point's address.
    link.station_tx(&data_to_ds(bssid, GUEST_MAC, &dhcp_message(GUEST_MAC, 3)), 5_000);
    link.net_step(5_000);
    assert!(link.ap_net().is_some_and(|n| n.dhcp_acks == 1), "the emulated network leases it an address");
    assert!(link.join_net().is_some_and(|n| n.dhcp_acks == 0), "and the join's network still has no lease");
}

#[test]
fn after_the_reboot_the_guest_is_a_station_of_the_emulated_access_point() {
    let (mut link, mut guest) = both_roles();
    for f in guest.step(200_000) { link.station_tx(&f.frame, 200_000); }
    assert_eq!(link.rx_match(false), 1 << 29, "while its own access point beacons");

    let link = link.surviving_reboot();
    assert!(link.ap().is_some() && link.join().is_some(), "both roles survive the reboot");
    assert!(link.join().is_some_and(|j| j.ap.is_none()), "the join starts scanning for the guest's access point again");
    assert_eq!(link.rx_match(false), (1 << 28) | (1 << 29), "and the guest's MAC is that station's now");
    assert_eq!(link.rx_match(true), 1 << 28);
}
