use super::*;

fn config() -> ApConfig {
    ApConfig { ssid: "esp32sim".into(), bssid: [2, 0x53, 0x49, 0x4d, 0, 1], channel: 6, psk: None, epoch_s: None }
}

fn station_frame(fc: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0; 24];
    frame[..2].copy_from_slice(&fc.to_le_bytes());
    frame[4..10].copy_from_slice(&config().bssid);
    frame[10..16].copy_from_slice(&[2, 3, 4, 5, 6, 7]);
    frame.extend_from_slice(payload);
    frame
}

#[test]
fn auth_rejects_every_truncated_fixed_body() {
    let valid = station_frame(11 << 4, &[0, 0, 1, 0, 0, 0]);
    for len in 0..valid.len() {
        let mut ap = VirtualAp::new(config(), true);
        assert!(ap.on_station_tx(&valid[..len], 0).is_none());
        assert_eq!(ap.state, StaState::Idle, "length {len}");
        assert!(ap.queue.is_empty(), "length {len}");
    }
    let mut ap = VirtualAp::new(config(), false);
    assert!(ap.on_station_tx(&valid, 0).is_none());
    assert_eq!(ap.state, StaState::Authenticated);
    assert_eq!(ap.queue.len(), 1);
}

fn eapol_message4() -> Vec<u8> {
    let mut body = vec![0; 99];
    body[0] = 2;
    body[1] = 3;
    body[2..4].copy_from_slice(&95u16.to_be_bytes());
    body[4] = 2;
    body[5..7].copy_from_slice(&0x0300u16.to_be_bytes());
    body
}

fn send_eapol(ap: &mut VirtualAp, body: &[u8]) {
    ap.state = StaState::Associated;
    let mut payload = vec![0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e];
    payload.extend_from_slice(body);
    assert!(ap.on_station_tx(&station_frame(0x0108, &payload), 0).is_none());
}

#[test]
fn eapol_rejects_declared_lengths_shorter_than_key_header() {
    for len in 0..95u16 {
        let mut body = eapol_message4();
        body[2..4].copy_from_slice(&len.to_be_bytes());
        // Exercise the message-2 slices as well as the message-4 state transition.
        for (state, key_info) in [(WpaState::AwaitingMessage2, 0x0100u16), (WpaState::AwaitingMessage4, 0x0300)] {
            body[5..7].copy_from_slice(&key_info.to_be_bytes());
            let mut ap = VirtualAp::new(config(), false);
            ap.wpa.state = state;
            send_eapol(&mut ap, &body);
            assert_eq!(ap.wpa.state, state, "declared length {len}");
            assert!(ap.queue.is_empty());
        }
    }
}

#[test]
fn eapol_rejects_truncated_declared_payload_and_key_data() {
    for (declared, key_data) in [(96u16, 0u16), (95, 1), (u16::MAX, 0)] {
        let mut body = eapol_message4();
        body[2..4].copy_from_slice(&declared.to_be_bytes());
        body[97..99].copy_from_slice(&key_data.to_be_bytes());
        let mut ap = VirtualAp::new(config(), false);
        ap.wpa.state = WpaState::AwaitingMessage4;
        send_eapol(&mut ap, &body);
        assert_eq!(ap.wpa.state, WpaState::AwaitingMessage4);
        assert!(ap.queue.is_empty());
    }
}

#[test]
fn eapol_allows_bytes_after_declared_payload() {
    let mut body = eapol_message4();
    body.extend_from_slice(&[0xff; 8]);
    let mut ap = VirtualAp::new(config(), false);
    ap.wpa.state = WpaState::AwaitingMessage4;
    send_eapol(&mut ap, &body);
    assert_eq!(ap.wpa.state, WpaState::Installed);
}

#[test]
fn wpa_handshake_installs_keys_only_after_message4() {
    let mut cfg = config();
    cfg.psk = Some("esp32sim-pass".into());
    let mut ap = VirtualAp::new(cfg, false);
    assert_eq!(ap.wpa.state, WpaState::Idle);
    ap.on_station_tx(&station_frame(11 << 4, &[0, 0, 1, 0, 0, 0]), 0);
    ap.on_station_tx(&station_frame(0, &[1, 0, 0, 0]), 0);
    assert_eq!(ap.wpa.state, WpaState::AwaitingMessage2);
    let m1 = &ap.queue.last().unwrap().frame[32..];
    assert_eq!(&m1[5..7], &0x008au16.to_be_bytes());
    assert_eq!(&m1[81..97], &[0; 16]);
    ap.queue.clear();

    let mut m2 = eapol_message4();
    m2[5..7].copy_from_slice(&0x010au16.to_be_bytes());
    m2[17..49].fill(0x5a);
    send_eapol(&mut ap, &m2);
    assert_eq!(ap.wpa.state, WpaState::AwaitingMessage4);
    let m3 = &ap.queue.last().unwrap().frame[32..];
    assert_eq!(&m3[5..7], &0x13cau16.to_be_bytes());
    assert_ne!(&m3[81..97], &[0; 16]);
    let ethernet = [0u8; 14];
    assert_eq!(ap.data_from_ds(&ethernet).unwrap()[1] & 0x40, 0);

    send_eapol(&mut ap, &eapol_message4());
    assert_eq!(ap.wpa.state, WpaState::Installed);
    let protected = ap.data_from_ds(&ethernet).unwrap();
    assert_eq!(protected[1] & 0x40, 0x40);
    assert_eq!(protected.len(), 24 + 8 + 8 + 8); // MAC, CCMP, LLC/SNAP and MIC
}

#[test]
fn data_to_eth_rejects_every_truncated_header() {
    let mut payload = vec![0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0];
    payload.extend_from_slice(&[1, 2, 3]);
    for fc in [0x0108, 0x0188] {
        let mut frame = station_frame(fc, &[]);
        if fc == 0x0188 { frame.extend_from_slice(&[0; 2]); }
        let header_len = frame.len() + 8;
        frame.extend_from_slice(&payload);
        for len in 0..header_len {
            assert!(data_to_eth(&frame[..len]).is_none(), "length {len}");
        }
        assert_eq!(data_to_eth(&frame).unwrap()[12..], [0x08, 0, 1, 2, 3]);
    }
}

#[test]
fn ap_configuration_keeps_defaults_and_recognizes_aliases() {
    let cfg = ApConfig::parse("").unwrap();
    assert_eq!(cfg.ssid, "esp32sim");
    assert_eq!(cfg.bssid, [2, 0x53, 0x49, 0x4d, 0, 1]);
    assert_eq!(cfg.channel, 6);
    assert!(cfg.psk.is_none());
    for channel in ["chan", "channel", "ch"] {
        for psk in ["psk", "password", "pass"] {
            let spec = format!("ssid=test,{channel}=11,{psk}=s=ecret,bssid=02:ab:CD:00:12:ff");
            let cfg = ApConfig::parse(&spec).unwrap();
            assert_eq!(cfg.ssid, "test");
            assert_eq!(cfg.channel, 11);
            assert_eq!(cfg.psk.as_deref(), Some("s=ecret"));
            assert_eq!(cfg.bssid, [2, 0xab, 0xcd, 0, 0x12, 0xff]);
        }
    }
}

#[test]
fn ap_configuration_rejects_unknown_options_and_invalid_values() {
    for spec in [
        "passwd=secret", "pass", "ssid=x,", "=x", "channel=0", "channel=15",
        "channel=256", "channel=-1", "channel=no", "bssid=02:53:49:4d:00",
        "bssid=02:53:49:4d:00:01:02", "bssid=02:xx:53:49:4d:00:01",
        "bssid=02:53:49:4d:00:GG", "bssid=2:53:49:4d:00:01", "bssid=+2:53:49:4d:00:01",
    ] {
        assert!(ApConfig::parse(spec).is_err(), "accepted {spec}");
    }
}

/// What `next_rx` may change: the access point's timers, queue and counts, and the link's queues
/// and pacing. `nothing_due` must only say so when none of them would change.
fn link_state(l: &StationLink) -> (u64, usize, (u64, u64, u64), u16, usize, u64) {
    let ap = l.ap().unwrap();
    (ap.next_beacon_us, ap.queue.len(), ap.stats, ap.seq, l.eth_rx.len(), l.last_rx_us)
}

#[test]
fn nothing_due_means_next_rx_delivers_and_changes_nothing() {
    for (case, relay) in [("network", false), ("relay", true)] {
        for t in (0..400_000).step_by(997) {
            // fresh, a frame queued for later, a frame from the network, and a frame just delivered
            for setup in 0..4 {
                let mut l = StationLink::default();
                l.attach(Some(VirtualAp::new(config(), false)), None);
                l.set_relay(relay);
                match setup {
                    1 => l.ap_mut().unwrap().queue.push(AirFrame { at_us: t + 1_000, frame: vec![0x40, 0, 0, 0] }),
                    2 => l.eth_rx.push(vec![0; 60]),
                    3 => { l.last_rx_us = t.saturating_sub(100); l.ap_mut().unwrap().queue.push(AirFrame { at_us: 0, frame: vec![0x40, 0, 0, 0] }); }
                    _ => {}
                }
                if !l.nothing_due(t) { continue; }
                let before = link_state(&l);
                assert_eq!(l.next_rx(t, false), None, "{case}, setup {setup}, t {t}: nothing_due but a frame came");
                assert_eq!(link_state(&l), before, "{case}, setup {setup}, t {t}: nothing_due but next_rx changed state");
            }
        }
    }
    // and it does not hide what is due: the first beacon, a queued frame, a frame from the network
    let mut l = StationLink::default();
    l.attach(Some(VirtualAp::new(config(), false)), None);
    let first_beacon = l.ap().unwrap().next_beacon_us;
    assert!(l.nothing_due(first_beacon - 1) && !l.nothing_due(first_beacon));
    l.ap_mut().unwrap().queue.push(AirFrame { at_us: 0, frame: vec![0x40, 0, 0, 0] });
    assert!(!l.nothing_due(1_000));
    let mut l = StationLink::default();
    l.attach(Some(VirtualAp::new(config(), false)), None);
    l.eth_rx.push(vec![0; 60]);
    assert!(!l.nothing_due(1_000));
}

/// A link with an access point and frames queued at it: `frames` are (at_us, frame type byte).
fn link_with_queue(frames: &[(u64, u8)]) -> StationLink {
    let mut l = StationLink::default();
    let mut ap = VirtualAp::new(config(), false);
    ap.next_beacon_us = u64::MAX;   // only the queued frames
    for &(at_us, fc) in frames { ap.queue.push(AirFrame { at_us, frame: vec![fc, 0, 0, 0] }); }
    l.attach(Some(ap), None);
    l
}

#[test]
fn next_rx_delivers_every_due_frame_one_gap_apart_management_first() {
    // two beacons (0x80) and a probe response (0x50), all due at once
    let mut l = link_with_queue(&[(10, 0x80), (20, 0x80), (30, 0x50)]);
    let mut got = Vec::new();
    let mut t = 1_000;
    while got.len() < 3 && t < 10_000 {
        if let Some(f) = l.next_rx(t, false) { got.push((t, f[0])); l.rx_delivered(0x1000); }
        t += 100;
    }
    assert_eq!(got.iter().map(|g| g.1).collect::<Vec<_>>(), [0x50, 0x80, 0x80], "management first, then beacons in time order, none lost");
    assert!(got.windows(2).all(|w| w[1].0 - w[0].0 >= RX_GAP_US), "one airtime gap apart: {got:?}");
    assert_eq!(l.rx_frames(), 3);
}

#[test]
fn a_busy_ring_holds_the_air_for_50_ms_only() {
    let mut l = link_with_queue(&[(0, 0x80), (0, 0x80)]);
    assert!(l.next_rx(1_000, false).is_some());
    l.rx_delivered(0x1000);
    assert_eq!(l.next_rx(1_000 + RX_GAP_US, true), None, "software has not recycled the last descriptor");
    assert_eq!(l.next_rx(1_000 + 50_000 - 1, true), None, "still within the stall time");
    assert!(l.next_rx(1_000 + 50_000, true).is_some(), "after 50 ms the frame goes anyway");
}

#[test]
fn a_reboot_keeps_the_host_side_only() {
    let mut l = link_with_queue(&[(0, 0x80)]);
    let ap = l.aps.pop();
    l.attach(ap, Some(crate::net::VirtualNet::new(false)));
    l.set_relay(true);
    l.eth_tx.push(vec![1]); l.eth_rx.push(vec![2]);
    assert!(l.next_rx(1_000, false).is_some());
    l.rx_delivered(0x1000); l.drop_rx(); l.station_tx(&[0; 24], 2_000);
    let l = l.surviving_reboot();
    assert!(l.ap().is_some() && l.net().is_some() && l.relay(), "the access point, network and relay mode stay");
    assert_eq!((l.eth_tx.len(), l.eth_rx.len(), l.last_rx_us, l.last_rx_desc, l.net_polled_us), (0, 0, 0, 0, 0), "queues and pacing start again");
    assert_eq!((l.tx_frames(), l.rx_frames(), l.rx_dropped(), l.tx_dropped()), (0, 0, 0, 0), "counts start again");
}

// ------------------------------------------------------------------ networks around the station

fn nearby() -> Vec<BeaconAp> {
    vec![
        BeaconAp { ssid: "Home".into(), bssid: [0x02, 1, 2, 3, 4, 5], channel: 1, rssi: -60, secure: true },
        BeaconAp { ssid: "Cafe".into(), bssid: [0x02, 9, 8, 7, 6, 5], channel: 11, rssi: -75, secure: false },
    ]
}

/// A probe request from the station to the broadcast address and BSSID, with the given elements.
fn probe_request(elements: &[u8]) -> Vec<u8> {
    let mut f = station_frame(4 << 4, elements);
    f[4..10].copy_from_slice(&[0xff; 6]);
    f[16..22].copy_from_slice(&[0xff; 6]);
    f
}

/// (BSSID, capability, the DSSS element's channel, an RSN element present) of a probe response.
fn response_summary(r: &[u8]) -> ([u8; 6], u16, u8, bool) {
    let mut bssid = [0u8; 6];
    bssid.copy_from_slice(&r[16..22]);
    let cap = u16::from_le_bytes([r[34], r[35]]);
    let elements = ies(r, 36);
    let chan = elements.iter().find(|(id, _)| *id == 3).map(|(_, d)| d[0]).expect("DSSS Parameter Set");
    (bssid, cap, chan, elements.iter().any(|(id, _)| *id == 48))
}

#[test]
fn nearby_networks_answer_a_wildcard_probe_after_the_access_point() {
    let mut ap = VirtualAp::new(config(), false);
    ap.beacons = nearby();
    assert!(ap.on_station_tx(&probe_request(&[0, 0]), 1000).is_none());
    assert_eq!(ap.queue.len(), 3);
    let (own, cap, chan, rsn) = response_summary(&ap.queue[0].frame);
    assert_eq!((own, cap & 0x11, chan, rsn), (config().bssid, 0x01, 6, false), "the open access point first: ESS, no Privacy");
    let (b, cap, chan, rsn) = response_summary(&ap.queue[1].frame);
    assert_eq!((b, cap & 0x11, chan, rsn), ([0x02, 1, 2, 3, 4, 5], 0x11, 1, true), "Home: Privacy and RSN, channel 1");
    let (b, cap, chan, rsn) = response_summary(&ap.queue[2].frame);
    assert_eq!((b, cap & 0x11, chan, rsn), ([0x02, 9, 8, 7, 6, 5], 0x01, 11, false), "Cafe: open, channel 11");
    assert!(ap.queue[0].at_us < ap.queue[1].at_us && ap.queue[1].at_us < ap.queue[2].at_us, "one after the other");
    for q in &ap.queue {
        assert_eq!(&q.frame[4..10], &[2, 3, 4, 5, 6, 7], "to the station");
        assert_eq!(&q.frame[10..16], &q.frame[16..22], "from its own BSSID");
    }
}

#[test]
fn a_directed_probe_gets_only_the_named_nearby_network() {
    let mut ap = VirtualAp::new(config(), false);
    ap.beacons = nearby();
    assert!(ap.on_station_tx(&probe_request(&[0, 4, b'C', b'a', b'f', b'e']), 1000).is_none());
    assert_eq!(ap.queue.len(), 1, "not the access point, not Home");
    assert_eq!(response_summary(&ap.queue[0].frame).0, [0x02, 9, 8, 7, 6, 5]);
    ap.queue.clear();
    assert!(ap.on_station_tx(&probe_request(&[0, 5, b'O', b't', b'h', b'e', b'r']), 2000).is_none());
    assert!(ap.queue.is_empty(), "nobody answers for an unknown SSID");
}

#[test]
fn frames_from_a_nearby_network_carry_its_channel_and_signal() {
    let mut ap = VirtualAp::new(config(), false);
    ap.beacons = nearby();
    let mut from_home = vec![0u8; 24];
    from_home[10..16].copy_from_slice(&[0x02, 1, 2, 3, 4, 5]);
    let mut from_ap = vec![0u8; 24];
    from_ap[10..16].copy_from_slice(&config().bssid);
    assert_eq!(ap.rx_meta(&from_home), (1, -60));
    assert_eq!(ap.rx_meta(&from_ap), (6, -40));
    assert_eq!(ap.rx_meta(&[0; 8]), (6, -40), "too short for an address: the access point's");
}

#[test]
fn authentication_to_a_nearby_network_goes_unanswered() {
    let mut ap = VirtualAp::new(config(), false);
    ap.beacons = nearby();
    let mut auth = station_frame(11 << 4, &[0, 0, 1, 0, 0, 0]);
    auth[4..10].copy_from_slice(&[0x02, 1, 2, 3, 4, 5]);
    auth[16..22].copy_from_slice(&[0x02, 1, 2, 3, 4, 5]);
    assert!(ap.on_station_tx(&auth, 0).is_none());
    assert_eq!(ap.state, StaState::Idle);
    assert!(ap.queue.is_empty(), "nothing answers: the network is out of reach");
}

#[test]
fn nearby_specs_parse_with_defaults_and_the_access_points_own_ssid_or_bssid_is_left_out() {
    let home = BeaconAp::parse("ssid=Home").unwrap();
    assert_eq!((home.channel, home.rssi, home.secure, home.bssid[0]), (1, -60, true, 0x02), "defaults; a locally administered unicast BSSID");
    assert_eq!(home.bssid, BeaconAp::parse("ssid=Home,chan=3").unwrap().bssid, "the same BSSID every time");
    assert_ne!(home.bssid, BeaconAp::parse("ssid=Cafe").unwrap().bssid);
    let cafe = BeaconAp::parse("ssid=Cafe,ch=11,rssi=-75,secure=0,bssid=02:09:08:07:06:05").unwrap();
    assert_eq!(cafe, BeaconAp { ssid: "Cafe".into(), bssid: [0x02, 9, 8, 7, 6, 5], channel: 11, rssi: -75, secure: false });
    for bad in ["chan=1", "ssid=", "ssid=Home,chan=15", "ssid=Home,rssi=low", "ssid=Home,secure=maybe", "ssid=Home,bssid=02:09", "ssid=Home,psk=x", "ssid=Home,chan"] {
        assert!(BeaconAp::parse(bad).is_err(), "{bad}");
    }
    assert!(BeaconAp::parse_all(&vec!["ssid=x".to_string(); 33]).is_err(), "more than 32");
    let mut link = StationLink::default();
    assert!(link.set_beacons(nearby()).is_none(), "no access point yet");
    link.attach(Some(VirtualAp::new(config(), false)), None);
    let mut list = nearby();
    list.push(BeaconAp { ssid: "esp32sim".into(), ..home.clone() });
    list.push(BeaconAp { ssid: "Office".into(), bssid: config().bssid, ..home });
    let left_out = link.set_beacons(list).unwrap();
    assert_eq!(link.ap().unwrap().beacons, nearby(), "the access point's own SSID or BSSID is that access point");
    assert_eq!(left_out.iter().map(|(b, why)| (b.ssid.as_str(), *why)).collect::<Vec<_>>(),
        [("esp32sim", "the access point's SSID"), ("Office", "the access point's BSSID")]);
}

/// Two networks around on one BSSID would both answer from it: `parse_all` refuses them, whether
/// the BSSID is derived from one SSID twice or given twice.
#[test]
fn nearby_networks_need_a_bssid_each() {
    let specs = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(BeaconAp::parse_all(&specs(&["ssid=Home", "ssid=Home,chan=6"])).unwrap_err().contains("share the BSSID"));
    assert!(BeaconAp::parse_all(&specs(&["ssid=Home,bssid=02:00:00:00:00:42", "ssid=Cafe,bssid=02:00:00:00:00:42"])).unwrap_err()
        .contains("'Home' and 'Cafe' share the BSSID 02:00:00:00:00:42"));
    assert_eq!(BeaconAp::parse_all(&specs(&["ssid=Home,bssid=02:00:00:00:00:42", "ssid=Home,bssid=02:00:00:00:00:43"])).unwrap().len(), 2,
        "one SSID on two BSSIDs is two networks");
}

/// A network around answers a probe request sent to the broadcast address or its own, with a
/// wildcard BSSID or its own (IEEE 802.11-2020 11.1.4.3.4), and no other.
#[test]
fn a_nearby_network_answers_only_probes_addressed_to_it() {
    let home = [0x02, 1, 2, 3, 4, 5];
    for (a1, a3, answers) in [([0xff; 6], [0xff; 6], true), (home, [0xff; 6], true), ([0xff; 6], home, true), (home, home, true),
        ([0x02, 9, 8, 7, 6, 5], [0xff; 6], false), ([0xff; 6], [0x02, 9, 8, 7, 6, 5], false)] {
        let mut ap = VirtualAp::new(config(), false);
        ap.beacons = vec![nearby().remove(0)];
        let mut probe = probe_request(&[0, 4, b'H', b'o', b'm', b'e']);
        probe[4..10].copy_from_slice(&a1);
        probe[16..22].copy_from_slice(&a3);
        assert!(ap.on_station_tx(&probe, 1000).is_none());
        assert_eq!(ap.queue.len(), usize::from(answers), "a1 {a1:02x?} a3 {a3:02x?}");
    }
}

/// The browser build gives the network a NAT after the access point is up; without a network
/// there is none to give one to.
#[test]
fn set_nat_needs_the_network_and_keeps_the_access_point() {
    let mut link = StationLink::default();
    assert!(link.set_nat(None, true).is_err());
    link.attach_new(ApConfig::parse("ssid=esp32sim").unwrap(), None, &crate::DebugFlags::default());
    let (transport, _handle) = crate::nat::QueueTransport::new();
    link.set_nat(Some(crate::nat::Nat::with_transport(false, Box::new(transport))), true).unwrap();
    let net = link.net().unwrap();
    assert!(net.nat.is_some() && net.local_ntp);
    assert_eq!(net.nat.as_ref().unwrap().resolver, [10, 0, 2, 3], "the queue's resolver is the subnet's");
    link.set_nat(None, false).unwrap();
    assert!(link.net().unwrap().nat.is_none() && !link.net().unwrap().local_ntp);
    assert_eq!(link.ap().unwrap().cfg.ssid, "esp32sim");
}

/// A new NAT (or none) aborts the old one's flows, so a queue's page hears of each.
#[test]
fn set_nat_aborts_the_old_nats_flows() {
    let mut link = StationLink::default();
    link.attach_new(ApConfig::parse("ssid=esp32sim").unwrap(), None, &crate::DebugFlags::default());
    let (transport, handle) = crate::nat::QueueTransport::new();
    link.set_nat(Some(crate::nat::Nat::with_transport(false, Box::new(transport))), false).unwrap();
    let mut syn = vec![0u8; 20];
    syn[..2].copy_from_slice(&1234u16.to_be_bytes());
    syn[2..4].copy_from_slice(&80u16.to_be_bytes());
    syn[12] = 0x50; syn[13] = 0x02;
    let nat = link.net_mut().unwrap().nat.as_mut().unwrap();
    assert!(nat.tcp_in(&[2; 6], &[10, 0, 2, 15], &[93, 184, 215, 14], &syn, 0).is_empty());
    let mut ops = Vec::new();
    assert_eq!(handle.take_ops(&mut ops), 1, "TCP_CONNECT");
    let id = u32::from_le_bytes(ops[4..8].try_into().unwrap());
    link.set_nat(None, false).unwrap();
    assert_eq!(handle.take_ops(&mut ops), 1);
    assert_eq!((ops[0], ops[1], u32::from_le_bytes(ops[4..8].try_into().unwrap())), (crate::nat::queue::op::TCP_CLOSE, 1, id), "aborted");
}

// ------------------------------------------------------------------ several joinable access points

const STA: [u8; 6] = [2, 3, 4, 5, 6, 7];

/// An open network and a WPA2-PSK one, as `--wifi ssid=esp32sim-open --wifi ssid=esp32sim,psk=…`,
/// with the network behind them.
fn two_ap_link() -> StationLink {
    let cfgs = ApConfig::parse_all(&["ssid=esp32sim-open".into(), "ssid=esp32sim,psk=esp32sim-pass".into()]).unwrap();
    let mut l = StationLink::default();
    let mut it = cfgs.into_iter();
    l.attach(Some(VirtualAp::new(it.next().unwrap(), false)), Some(crate::net::VirtualNet::new(false)));
    for cfg in it { l.add_ap(cfg, &crate::DebugFlags::default()).unwrap(); }
    l
}

/// A management or data frame from the station to `bssid`.
fn frame_to(fc: u16, bssid: [u8; 6], payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0; 24];
    f[..2].copy_from_slice(&fc.to_le_bytes());
    f[4..10].copy_from_slice(&bssid);
    f[10..16].copy_from_slice(&STA);
    f[16..22].copy_from_slice(&bssid);
    f.extend_from_slice(payload);
    f
}

/// Every frame the air delivers from `from_us` to `to_us`, one per airtime gap.
fn drain(l: &mut StationLink, from_us: u64, to_us: u64) -> Vec<Vec<u8>> {
    let mut got = Vec::new();
    let mut t = from_us;
    while t < to_us {
        l.net_step(t);
        if let Some(f) = l.next_rx(t, false) { got.push(f); l.rx_delivered(0x1000); }
        t += 100;
    }
    got
}

/// Open-system authentication and association with `bssid`, then what the air brings for 50 ms.
fn join(l: &mut StationLink, bssid: [u8; 6], t: u64) -> Vec<Vec<u8>> {
    l.station_tx(&frame_to(11 << 4, bssid, &[0, 0, 1, 0, 0, 0]), t);
    l.station_tx(&frame_to(0, bssid, &[1, 0, 0, 0]), t + 1_000);
    drain(l, t + 1_000, t + 50_000)
}

fn dhcp_discover() -> Vec<u8> {
    let mut d = vec![0u8; 240];
    d[0] = 1; d[1] = 1; d[2] = 6;
    d[4..8].copy_from_slice(&[1, 2, 3, 4]);
    d[28..34].copy_from_slice(&STA);
    d[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    d.extend_from_slice(&[53, 1, 1, 255]);
    let udp = crate::net::packet::udp_packet(&[0; 4], &[255; 4], 68, 67, &d);
    let ip = crate::net::packet::ip_packet(17, &[0; 4], &[255; 4], &udp);
    let mut payload = vec![0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00];
    payload.extend_from_slice(&ip);
    payload
}

#[test]
fn several_access_points_parse_with_distinct_bssids_and_no_duplicates() {
    let cfgs = ApConfig::parse_all(&["ssid=a".into(), "ssid=b,psk=12345678".into(), "ssid=c,bssid=02:00:00:00:00:09".into()]).unwrap();
    assert_eq!(cfgs[0].bssid, [0x02, 0x53, 0x49, 0x4d, 0x00, 0x01], "the first keeps today's default");
    assert_eq!(cfgs[1].bssid, derived_bssid("b"), "a later one derives its own from the SSID");
    assert_eq!(cfgs[2].bssid, [2, 0, 0, 0, 0, 9]);
    assert_eq!(ApConfig::parse_all(&["ssid=a".into()]).unwrap()[0].bssid, ApConfig::parse("ssid=a").unwrap().bssid, "one spec parses as before");
    assert!(ApConfig::parse_all(&["ssid=a".into(), "ssid=a,psk=12345678".into()]).is_err(), "one SSID twice");
    assert!(ApConfig::parse_all(&["ssid=a".into(), "ssid=b,bssid=02:53:49:4d:00:01".into()]).is_err(), "one BSSID twice");
    let mut l = two_ap_link();
    assert!(l.add_ap(ApConfig::parse("ssid=esp32sim").unwrap(), &crate::DebugFlags::default()).is_err());
    assert!(StationLink::default().add_ap(ApConfig::parse("").unwrap(), &crate::DebugFlags::default()).is_err(), "needs the first one");
}

#[test]
fn a_scan_hears_both_access_points() {
    let mut l = two_ap_link();
    let (open, psk) = (l.aps()[0].cfg.bssid, l.aps()[1].cfg.bssid);
    // beacons from both within the first 300 ms
    let beacons: Vec<[u8; 6]> = drain(&mut l, 0, 300_000).iter().filter(|f| is_beacon(f)).map(|f| f[10..16].try_into().unwrap()).collect();
    assert!(beacons.contains(&open) && beacons.contains(&psk), "{beacons:x?}");
    // and a wildcard probe gets a response from each, with its own security
    l.station_tx(&probe_request(&[0, 0]), 400_000);
    let resp: Vec<_> = drain(&mut l, 400_000, 420_000).into_iter().filter(|f| f[0] == 0x50).map(|f| response_summary(&f)).collect();
    assert_eq!(resp.len(), 2, "{resp:x?}");
    assert!(resp.iter().any(|r| r.0 == open && !r.3) && resp.iter().any(|r| r.0 == psk && r.3));
    // a frame from the second carries its channel
    let mut f = vec![0x80, 0, 0, 0]; f.resize(24, 0); f[10..16].copy_from_slice(&psk);
    assert_eq!(l.rx_meta(&f), (6, -40));
}

#[test]
fn the_open_network_associates_and_leases_without_a_handshake() {
    let mut l = two_ap_link();
    let open = l.aps()[0].cfg.bssid;
    let got = join(&mut l, open, 0);
    assert_eq!(l.aps()[0].state, StaState::Associated);
    assert_eq!(l.aps()[1].state, StaState::Idle);
    assert_eq!(l.aps()[0].wpa.state, WpaState::Idle, "no four-way handshake on an open network");
    assert!(!got.iter().any(|f| describe(f).contains("888e")), "no EAPOL frames");
    l.station_tx(&frame_to(0x0108, open, &dhcp_discover()), 60_000);
    let offer = drain(&mut l, 60_000, 70_000).into_iter().find(|f| f[0] == 0x08 && f[10..16] == open);
    assert!(offer.is_some(), "a DHCP offer from the open access point");
}

#[test]
fn the_psk_network_runs_the_wpa2_handshake() {
    let mut l = two_ap_link();
    let psk = l.aps()[1].cfg.bssid;
    let got = join(&mut l, psk, 0);
    assert_eq!(l.aps()[1].state, StaState::Associated);
    assert_eq!(l.aps()[1].wpa.state, WpaState::AwaitingMessage2);
    assert!(got.iter().any(|f| f[10..16] == psk && describe(f).contains("888e")), "message 1 from the PSK access point");
    let mut m2 = eapol_message4();
    m2[5..7].copy_from_slice(&0x010au16.to_be_bytes());
    let mut payload = vec![0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e];
    payload.extend_from_slice(&m2);
    l.station_tx(&frame_to(0x0108, psk, &payload), 60_000);
    assert_eq!(l.aps()[1].wpa.state, WpaState::AwaitingMessage4);
    let mut payload = vec![0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e];
    payload.extend_from_slice(&eapol_message4());
    l.station_tx(&frame_to(0x0108, psk, &payload), 70_000);
    assert_eq!(l.aps()[1].wpa.state, WpaState::Installed);
    assert_eq!(l.aps()[0].state, StaState::Idle, "the open one never heard the station");
}

#[test]
fn associating_with_another_access_point_leaves_the_first() {
    let mut l = two_ap_link();
    let (open, psk) = (l.aps()[0].cfg.bssid, l.aps()[1].cfg.bssid);
    join(&mut l, psk, 0);
    assert_eq!(l.aps()[1].state, StaState::Associated);
    join(&mut l, open, 100_000);
    assert_eq!((l.aps()[0].state, l.aps()[1].state), (StaState::Associated, StaState::Idle));
    assert_eq!(l.aps()[1].wpa.state, WpaState::Idle);
    // data to the one left is not passed on; to the joined one it reaches the network
    l.station_tx(&frame_to(0x0108, psk, &dhcp_discover()), 200_000);
    assert_eq!(l.eth_tx_pending(), 0);
    l.station_tx(&frame_to(0x0108, open, &dhcp_discover()), 200_100);
    assert_eq!(l.eth_tx_pending(), 1);
    // and back again
    join(&mut l, psk, 300_000);
    assert_eq!((l.aps()[0].state, l.aps()[1].state), (StaState::Idle, StaState::Associated));
    // both survive a reboot
    let l = l.surviving_reboot();
    assert_eq!(l.aps().len(), 2);
}

/// A capture tap that keeps frames for the page: each record as (direction byte, frame).
fn live_tap() -> crate::netcap::NetTap {
    let tap = crate::netcap::NetTap::default();
    tap.lock().unwrap().enable_live();
    tap
}
fn taken(tap: &crate::netcap::NetTap) -> Vec<(u8, Vec<u8>)> {
    tap.lock().unwrap().take_live().into_iter().map(|m| (m[1], m[10..].to_vec())).collect()
}
/// `payload` in a data frame to `bssid`, ending in the 4-byte FCS slot as the MAC hands it over.
fn data_with_fcs(bssid: [u8; 6], payload: &[u8]) -> Vec<u8> {
    let mut f = frame_to(0x0108, bssid, payload);
    f.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    f
}

#[test]
fn the_tap_records_a_dhcp_exchange_both_ways_without_the_fcs() {
    let mut l = two_ap_link();
    let tap = live_tap();
    l.set_tap(tap.clone());
    let open = l.aps()[0].cfg.bssid;
    join(&mut l, open, 0);
    assert!(taken(&tap).is_empty(), "management frames are not Ethernet frames");
    l.station_tx(&data_with_fcs(open, &dhcp_discover()), 60_000);
    drain(&mut l, 60_000, 70_000);
    let got = taken(&tap);
    let up: Vec<_> = got.iter().filter(|(d, _)| *d == crate::netcap::Dir::FromStation as u8).collect();
    let down: Vec<_> = got.iter().filter(|(d, _)| *d == crate::netcap::Dir::ToStation as u8).collect();
    assert_eq!((up.len(), down.len()), (1, 1), "one discover, one offer: {got:x?}");
    let discover = &up[0].1;
    assert_eq!(discover[..6], open, "addr3 of the data frame");
    assert_eq!(discover[6..12], STA);
    assert_eq!(discover[12..14], [0x08, 0x00]);
    assert_eq!(discover.len(), 14 + dhcp_discover().len() - 8, "the FCS slot is left out");
    assert_eq!(down[0].1[..6], STA, "the offer is for the station");
    // the tap survives a reboot and a new attach
    let mut l = l.surviving_reboot();
    l.attach(Some(VirtualAp::new(config(), false)), None);
    assert!(l.tap.is_some());
}

#[test]
fn in_relay_mode_the_tap_skips_dropped_frames_and_records_delivered_ones() {
    let mut l = two_ap_link();
    l.set_relay(true);
    let tap = live_tap();
    l.set_tap(tap.clone());
    let open = l.aps()[0].cfg.bssid;
    join(&mut l, open, 0);
    taken(&tap);
    // too long for the relay: dropped, so not captured
    let mut big = dhcp_discover();
    big.resize(RELAY_FRAME_MAX + 64, 0);
    l.station_tx(&data_with_fcs(open, &big), 60_000);
    assert!(l.take_relay_frames().is_empty());
    assert!(taken(&tap).is_empty(), "a dropped frame is not captured");
    l.station_tx(&data_with_fcs(open, &dhcp_discover()), 60_100);
    assert_eq!(l.take_relay_frames().len(), 1);
    assert_eq!(taken(&tap).len(), 1, "a relayed frame is");
    // a frame from the host is captured when it is delivered, not when it is queued
    let mut eth = vec![0u8; 60];
    eth[..6].copy_from_slice(&STA); eth[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 2]); eth[12..14].copy_from_slice(&[0x08, 0x00]);
    l.receive_relay_frame(&eth).unwrap();
    assert!(taken(&tap).is_empty(), "queued, not yet delivered");
    let rx = drain(&mut l, 70_000, 80_000);
    assert!(rx.iter().any(|f| f[0] == 0x08), "delivered to the station");
    assert_eq!(taken(&tap), vec![(crate::netcap::Dir::ToStation as u8, eth)]);
}

#[test]
fn leaving_an_access_point_drops_what_it_still_had_queued_for_the_station() {
    let mut l = two_ap_link();
    let (open, psk) = (l.aps()[0].cfg.bssid, l.aps()[1].cfg.bssid);
    // the association with the PSK network queues EAPOL message 1 a little later
    l.station_tx(&frame_to(11 << 4, psk, &[0, 0, 1, 0, 0, 0]), 0);
    l.station_tx(&frame_to(0, psk, &[1, 0, 0, 0]), 1_000);
    assert!(l.aps()[1].queue.iter().any(|a| !is_beacon(&a.frame)), "something queued for the station");
    // the station associates with the open one before it is delivered
    l.station_tx(&frame_to(11 << 4, open, &[0, 0, 1, 0, 0, 0]), 1_100);
    l.station_tx(&frame_to(0, open, &[1, 0, 0, 0]), 1_200);
    assert!(l.aps()[1].queue.iter().all(|a| is_beacon(&a.frame)), "only beacons left");
    let got = drain(&mut l, 1_200, 100_000);
    assert!(!got.iter().any(|f| !is_beacon(f) && f[10..16] == psk), "nothing more from the PSK network to the station");
}

#[test]
fn epoch_belongs_in_the_first_wifi_spec_only() {
    assert_eq!(ApConfig::parse_all(&["ssid=a,epoch=1000".into(), "ssid=b".into()]).unwrap()[0].epoch_s, Some(1000));
    let e = ApConfig::parse_all(&["ssid=a".into(), "ssid=b,epoch=1000".into()]).unwrap_err();
    assert!(e.contains("epoch"), "{e}");
}

fn station(spec: &str) -> sta::VirtualSta { sta::VirtualSta::new(sta::StaConfig::parse(spec).unwrap(), false) }

/// What the guest's MAC hands the air from its TX queue: the frame and the 4 bytes of its FCS slot.
fn with_fcs_slot(mut frame: Vec<u8>) -> Vec<u8> { frame.extend_from_slice(&[0; 4]); frame }

#[test]
fn station_options_parse_and_refuse() {
    let cfg = sta::StaConfig::parse("ssid=lab,psk=lab-pass,mac=02:00:00:00:00:09").unwrap();
    assert_eq!((cfg.ssid.as_str(), cfg.psk.as_deref(), cfg.mac), ("lab", Some("lab-pass"), [2, 0, 0, 0, 0, 9]));
    assert_eq!(sta::StaConfig::parse("ssid=lab,password=lab-pass").unwrap().psk.as_deref(), Some("lab-pass"));
    for (spec, why) in [("", "needs ssid"), ("psk=lab-pass", "needs ssid"), ("ssid=lab,psk=short", "8 to 63"),
                        ("ssid=lab,chan=6", "unknown station option 'chan'"), ("ssid=lab,mac=02:00", "expected six hexadecimal octets")] {
        let e = sta::StaConfig::parse(spec).unwrap_err();
        assert!(e.contains(why), "{spec}: {e}");
    }
}

/// The station against `VirtualAp` standing in for the guest's access point, with `VirtualNet`'s
/// DHCP server behind it: each side checks the other's half of the handshake.
fn station_joins(ap_psk: Option<&str>, sta_spec: &str, until_us: u64) -> (sta::VirtualSta, VirtualAp) {
    station_joins_with(ap_psk, sta_spec, until_us, |_| {})
}

/// `station_joins`, with `edit` applied to each beacon the station hears.
fn station_joins_with(ap_psk: Option<&str>, sta_spec: &str, until_us: u64, edit: impl Fn(&mut Vec<u8>)) -> (sta::VirtualSta, VirtualAp) {
    let mut ap = VirtualAp::new(ApConfig { psk: ap_psk.map(Into::into), ..config() }, false);
    let mut sta = station(sta_spec);
    let mut net = crate::net::VirtualNet::new(false);
    for now in (0..until_us).step_by(100) {
        for a in ap.step(now) {
            let mut f = a.frame;
            if is_beacon(&f) { edit(&mut f); }
            if let Some(eth) = sta.on_ap_tx(&with_fcs_slot(f), now) { sta.host_rx(&eth, now); }
        }
        for a in sta.step(now) {
            let Some(data) = ap.on_station_tx(&with_fcs_slot(a.frame), now) else { continue };
            let Some(eth) = data_to_eth(&data) else { continue };
            net.sta_mac = sta.cfg.mac;
            for reply in net.handle(&eth, now) {
                if let Some(f) = ap.data_from_ds(&reply) { ap.queue.push(AirFrame { at_us: now, frame: f }); }
            }
        }
        if sta.lease.is_some() { break; }
    }
    (sta, ap)
}

#[test]
fn station_joins_an_open_access_point_and_takes_a_lease() {
    let (sta, ap) = station_joins(None, "ssid=esp32sim", 2_000_000);
    assert_eq!((sta.state, ap.state), (sta::StaLink::Joined, StaState::Associated));
    assert_eq!(sta.lease, Some([10, 0, 2, 15]));
    assert_eq!(sta.channel(), 6);
}

#[test]
fn station_completes_the_four_way_handshake_with_the_authenticator() {
    let (sta, ap) = station_joins(Some("esp32sim-pass"), "ssid=esp32sim,psk=esp32sim-pass", 2_000_000);
    assert_eq!(ap.wpa.state, WpaState::Installed);
    assert_eq!((sta.state, sta.lease), (sta::StaLink::Joined, Some([10, 0, 2, 15])));
}

#[test]
fn station_with_the_wrong_passphrase_never_joins_and_starts_again() {
    let (sta, ap) = station_joins(Some("esp32sim-pass"), "ssid=esp32sim,psk=not-the-pass", 3_500_000);
    assert_ne!(ap.wpa.state, WpaState::Installed);
    assert_ne!(sta.state, sta::StaLink::Joined);
    assert_eq!((sta.stats.0, sta.lease), (0, None));
    // message 3 fails its MIC, the handshake times out, and the next beacon starts another attempt
    assert!(sta.attempts >= 2, "{} attempts", sta.attempts);
}

#[test]
fn station_does_not_join_an_open_network_with_a_passphrase() {
    let (sta, _) = station_joins(None, "ssid=esp32sim,psk=esp32sim-pass", 1_000_000);
    assert_eq!(sta.state, sta::StaLink::Listening);
}

#[test]
fn station_listens_again_after_a_deauthentication() {
    let (mut sta, mut ap) = station_joins(None, "ssid=esp32sim", 2_000_000);
    assert!(sta.lease.is_some());
    let mut deauth = ap.beacon_like(12, &sta.cfg.mac, 0)[..24].to_vec();
    deauth.extend_from_slice(&3u16.to_le_bytes());                              // leaving
    assert!(sta.on_ap_tx(&with_fcs_slot(deauth), 2_000_000).is_none());
    assert_eq!((sta.state, sta.lease), (sta::StaLink::Listening, None));
}

/// Through `StationLink` as the bus uses it, with the relay on: the station joins, takes no lease
/// of its own, and carries the host's Ethernet frames both ways.
#[test]
fn station_link_carries_the_relay_once_joined() {
    let mut link = StationLink::default();
    link.set_relay(true);
    link.attach_station(station("ssid=esp32sim")).unwrap();
    let mut ap = VirtualAp::new(config(), false);
    // the host is the station: without four-address frames it sends and takes the station's MAC
    let host = sta::StaConfig::parse("ssid=esp32sim").unwrap().mac;
    let mut to_host = Vec::new();
    let mut from_host = 0;
    for now in (0..1_000_000).step_by(100) {
        for a in ap.step(now) { link.station_tx(&with_fcs_slot(a.frame), now); }
        if link.station().is_some_and(|s| s.state == sta::StaLink::Joined) && from_host == 0 {
            let mut eth = vec![0xff; 6];
            eth.extend_from_slice(&host); eth.extend_from_slice(&[0x08, 0x06]); eth.extend_from_slice(&[0; 28]);
            link.receive_relay_frame(&eth).unwrap();
            let mut down = host.to_vec();
            down.extend_from_slice(&config().bssid); down.extend_from_slice(&[0x08, 0x06]); down.extend_from_slice(&[0; 28]);
            let f = ap.data_from_ds(&down).unwrap();
            ap.queue.push(AirFrame { at_us: now, frame: f });
            from_host = 1;
        }
        if let Some(f) = link.next_rx(now, false) {
            if let Some(data) = ap.on_station_tx(&with_fcs_slot(f), now) {
                if data_to_eth(&data).is_some_and(|e| e[..6] == [0xff; 6] && e[12..14] == [0x08, 0x06]) { from_host = 2; }
            }
        }
        to_host.extend(link.take_relay_frames());
    }
    assert_eq!(from_host, 2, "the host's frame did not reach the access point");
    assert_eq!(to_host.len(), 1, "the access point's frame did not reach the host");
    assert_eq!(&to_host[0][..6], &host[..]);
    assert_eq!(link.station().unwrap().lease, None);
}

/// A beacon of `config()`'s access point with `edit` applied to it, through `VirtualAp`.
fn beacon(psk: bool, tsf_us: u64, edit: impl Fn(&mut Vec<u8>)) -> Vec<u8> {
    let mut ap = VirtualAp::new(ApConfig { psk: psk.then(|| "esp32sim-pass".into()), ..config() }, false);
    let mut f = ap.beacon_like(8, &[0xff; 6], tsf_us);
    edit(&mut f);
    with_fcs_slot(f)
}

/// The offset of the RSN element in `config()`'s secured beacon.
fn rsn_at(f: &[u8]) -> usize { (36..f.len()).find(|&i| f[i..].starts_with(RSN_IE)).unwrap() }

#[test]
fn station_leaves_networks_it_cannot_join_alone_and_says_why() {
    for (psk, edit, why) in [
        (true, (|f: &mut Vec<u8>| { let i = rsn_at(f); f[i + 19] = 8; }) as fn(&mut Vec<u8>), "WPA3-SAE only"),
        (true, |f| { let i = rsn_at(f); f[i + 20] |= 0x40; }, "requires protected management frames"),
        (true, |f| { let i = rsn_at(f); f.drain(i..i + RSN_IE.len()); }, "is WEP or WPA"),
        (false, |_| {}, "is open and the station has a passphrase"),
    ] {
        let mut sta = station("ssid=esp32sim,psk=esp32sim-pass");
        assert!(sta.on_ap_tx(&beacon(psk, 0, edit), 100).is_none());
        assert_eq!((sta.state, sta.attempts), (sta::StaLink::Listening, 0), "{why}");
        assert!(sta.report().contains(why), "{why}: {}", sta.report());
        assert!(sta.step(200).is_empty(), "{why}");
    }
    let mut open = station("ssid=esp32sim");
    open.on_ap_tx(&beacon(true, 0, |_| {}), 100);
    assert!(open.report().contains("is secured and the station has no passphrase"), "{}", open.report());
}

#[test]
fn station_drops_its_own_broadcasts_repeated_by_the_access_point() {
    let (mut sta, mut ap) = station_joins(None, "ssid=esp32sim", 2_000_000);
    let mine = sta.cfg.mac;
    let broadcast = |src: [u8; 6]| { let mut e = vec![0xff; 6]; e.extend_from_slice(&src); e.extend_from_slice(&[0x08, 0x06]); e.extend_from_slice(&[0; 28]); e };
    let repeated = ap.data_from_ds(&broadcast(mine)).unwrap();
    assert!(sta.on_ap_tx(&with_fcs_slot(repeated), 2_000_000).is_none());
    let other = ap.data_from_ds(&broadcast([2, 0, 0, 0, 0, 7])).unwrap();
    assert!(sta.on_ap_tx(&with_fcs_slot(other), 2_000_000).is_some());
}

#[test]
fn station_listens_again_when_the_access_point_stops_or_starts_again() {
    // a deauthentication to every station
    let (mut sta, mut ap) = station_joins(None, "ssid=esp32sim", 2_000_000);
    let mut deauth = ap.beacon_like(12, &[0xff; 6], 0)[..24].to_vec();
    deauth.extend_from_slice(&3u16.to_le_bytes());
    sta.on_ap_tx(&with_fcs_slot(deauth), 2_000_000);
    assert_eq!(sta.state, sta::StaLink::Listening);
    // no beacon for 3 s
    let (mut sta, _) = station_joins(None, "ssid=esp32sim", 2_000_000);
    sta.step(4_000_000);
    assert!(sta.step(5_500_000).is_empty());
    assert_eq!(sta.state, sta::StaLink::Listening);
    assert!(sta.report().contains("no beacon from the access point"), "{}", sta.report());
    // a beacon whose timestamp went back
    let (mut sta, _) = station_joins(None, "ssid=esp32sim", 2_000_000);
    sta.on_ap_tx(&beacon(false, 1_900_000, |_| {}), 1_950_000);
    assert_eq!(sta.state, sta::StaLink::Joined);
    sta.on_ap_tx(&beacon(false, 1_000, |_| {}), 1_960_000);
    assert_eq!(sta.state, sta::StaLink::Listening);
    assert!(sta.report().contains("started again"), "{}", sta.report());
}

#[test]
fn station_link_refuses_what_it_cannot_carry_and_captures_both_ways() {
    let mut link = StationLink::default();
    link.attach_station(station("ssid=esp32sim")).unwrap();
    assert!(link.add_ap(config(), &crate::DebugFlags::default()).is_err(), "an access point beside the station");
    let tap: crate::netcap::NetTap = std::sync::Arc::new(std::sync::Mutex::new(crate::netcap::NetCapture::default()));
    tap.lock().unwrap().enable_live();
    link.set_tap(tap.clone());
    let mut ap = VirtualAp::new(config(), false);
    let mut net = crate::net::VirtualNet::new(false);
    for now in (0..1_000_000).step_by(100) {
        for a in ap.step(now) { link.station_tx(&with_fcs_slot(a.frame), now); }
        let Some(f) = link.next_rx(now, false) else { continue };
        let Some(eth) = ap.on_station_tx(&with_fcs_slot(f), now).and_then(|d| data_to_eth(&d)) else { continue };
        net.sta_mac = link.station().unwrap().cfg.mac;
        for reply in net.handle(&eth, now) { if let Some(f) = ap.data_from_ds(&reply) { ap.queue.push(AirFrame { at_us: now, frame: f }); } }
    }
    assert_eq!(link.station().unwrap().lease, Some([10, 0, 2, 15]));
    let frames = tap.lock().unwrap().take_live();
    let (to_guest, from_guest) = (frames.iter().filter(|m| m[1] == 1).count(), frames.iter().filter(|m| m[1] == 0).count());
    assert!(to_guest >= 2 && from_guest >= 2, "DISCOVER/REQUEST out, OFFER/ACK in: {to_guest} out, {from_guest} in");
    // each IPv4 frame whole, both ways: its Ethernet header and the datagram's total length
    for m in &frames {
        let eth = &m[10..];
        if eth[12..14] != [0x08, 0x00] { continue; }
        assert_eq!(eth.len(), 14 + u16::from_be_bytes([eth[16], eth[17]]) as usize, "direction {}", m[1]);
    }
    // the relay takes no frame before the station has joined
    let mut relay = StationLink::default();
    relay.set_relay(true);
    relay.attach_station(station("ssid=esp32sim")).unwrap();
    let eth = [vec![0xff; 6], vec![2, 0, 0, 0, 0, 1], vec![0x08, 0x06], vec![0; 28]].concat();
    assert!(relay.receive_relay_frame(&eth).unwrap_err().contains("has not joined"));
}

#[test]
fn station_takes_the_access_points_group_cipher_and_needs_ccmp_pairwise() {
    // WPA/WPA2 mixed mode as ESP-IDF announces it: a TKIP group cipher, CCMP among the pairwise
    let tkip_group = |f: &mut Vec<u8>| { let i = rsn_at(f); f[i + 7] = 2; };
    let mut sta = station("ssid=esp32sim,psk=esp32sim-pass");
    sta.on_ap_tx(&beacon(true, 0, tkip_group), 100);
    assert_eq!((sta.state, sta.attempts), (sta::StaLink::Authenticating, 1), "{}", sta.report());
    let mut auth = beacon(true, 0, |_| {})[..24].to_vec();
    auth[0] = 0xb0; auth[4..10].copy_from_slice(&sta.cfg.mac);
    auth.extend_from_slice(&[0, 0, 2, 0, 0, 0]);
    sta.on_ap_tx(&with_fcs_slot(auth), 200);
    let assoc = sta.step(1_000).into_iter().map(|a| a.frame).find(|f| f[0] == 0x00).expect("an association request");
    let i = assoc.windows(2).position(|w| w == [48, 20]).expect("an RSN element");
    assert_eq!(&assoc[i + 4..i + 8], &[0x00, 0x0f, 0xac, 2], "the access point's TKIP group cipher");
    assert_eq!(&assoc[i + 10..i + 14], &[0x00, 0x0f, 0xac, 4], "CCMP pairwise");
    // TKIP alone among the pairwise ciphers
    let mut sta = station("ssid=esp32sim,psk=esp32sim-pass");
    sta.on_ap_tx(&beacon(true, 0, |f| { let i = rsn_at(f); f[i + 13] = 2; }), 100);
    assert_eq!(sta.attempts, 0);
    assert!(sta.report().contains("offers no CCMP pairwise cipher"), "{}", sta.report());
}

#[test]
fn station_stops_after_three_refusals_with_the_same_status() {
    let mut sta = station("ssid=esp32sim");
    let mut now = 0;
    for _ in 0..5 {
        now += 2_000_000;
        sta.on_ap_tx(&beacon(false, now, |_| {}), now);
        let mut auth = beacon(false, 0, |_| {})[..24].to_vec();
        auth[0] = 0xb0; auth[4..10].copy_from_slice(&sta.cfg.mac);
        auth.extend_from_slice(&[0, 0, 2, 0, 17, 0]);                       // status 17: AP is full
        sta.on_ap_tx(&with_fcs_slot(auth), now + 100);
    }
    assert_eq!(sta.attempts, 3);
    assert!(sta.report().contains("authentication refused 3 times with status 17: not trying again"), "{}", sta.report());
}

#[test]
fn station_takes_tkip_group_frames_without_their_longer_trailer() {
    // WPA/WPA2 mixed mode: a TKIP group cipher, so a group frame ends in 12 bytes, not 8
    let tkip_group = |f: &mut Vec<u8>| { let i = rsn_at(f); f[i + 7] = 2; };
    let (mut sta, mut ap) = station_joins_with(Some("esp32sim-pass"), "ssid=esp32sim,psk=esp32sim-pass", 2_000_000, tkip_group);
    assert_eq!(sta.lease, Some([10, 0, 2, 15]));
    let eth = [vec![0xff; 6], vec![2, 0, 0, 0, 0, 7], vec![0x08, 0x06], vec![0x11; 28]].concat();
    let mut f = ap.data_from_ds(&eth).unwrap();          // CCMP-framed: header, payload, 8 bytes of MIC
    f.extend_from_slice(&[0; 4]);                        // TKIP's 4 more: Michael MIC and ICV are 12
    assert_eq!(sta.on_ap_tx(&with_fcs_slot(f), 2_000_000), Some(eth.clone()));
    // a unicast frame keeps CCMP's 8
    let mut to_sta = eth.clone();
    to_sta[..6].copy_from_slice(&sta.cfg.mac);
    let f = ap.data_from_ds(&to_sta).unwrap();
    assert_eq!(sta.on_ap_tx(&with_fcs_slot(f), 2_000_000), Some(to_sta));
}

/// A frame the relay took while the station was joined, and that the station can no longer carry
/// because it has left the access point since, is counted, not lost without a word.
#[test]
fn station_link_counts_relayed_frames_a_leaving_station_drops() {
    let mut link = StationLink::default();
    link.set_relay(true);
    link.attach_station(station("ssid=esp32sim")).unwrap();
    let mut ap = VirtualAp::new(config(), false);
    let mut now = 0;
    while !link.station().is_some_and(|s| s.state == sta::StaLink::Joined) {
        assert!(now < 1_000_000, "the station did not join");
        for a in ap.step(now) { link.station_tx(&with_fcs_slot(a.frame), now); }
        if let Some(f) = link.next_rx(now, false) { ap.on_station_tx(&with_fcs_slot(f), now); }
        now += 100;
    }
    let mac = link.station().unwrap().cfg.mac;
    let eth = [vec![0xff; 6], mac.to_vec(), vec![0x08, 0x06], vec![0; 28]].concat();
    link.receive_relay_frame(&eth).unwrap();
    let mut deauth = ap.beacon_like(12, &mac, 0)[..24].to_vec();
    deauth.extend_from_slice(&3u16.to_le_bytes());
    link.station_tx(&with_fcs_slot(deauth), now);
    assert_eq!(link.station().unwrap().state, sta::StaLink::Listening);
    for t in (now..now + 2_000).step_by(100) { assert!(link.next_rx(t, false).is_none(), "nothing to carry it"); }
    assert_eq!((link.relay_lost(), link.tx_dropped()), (1, 0));
    assert!(link.report().contains("1 relayed frames lost as the station had left"), "{}", link.report());
}

// ------------------------------------------------------------------ keep-alive and lease renewal
/// The guest's side for the keep-alive and lease tests: `VirtualAp` as its access point and
/// `VirtualNet`'s DHCP server behind it.
struct Guest { ap: VirtualAp, net: crate::net::VirtualNet }

impl Guest {
    fn new() -> Self { Guest { ap: VirtualAp::new(config(), false), net: crate::net::VirtualNet::new(false) } }

    /// Run the station against the guest from `from_us` to `to_us` in 1 ms steps. `ap_edit` may
    /// change each frame the access point sends; `reply` may rewrite or drop each Ethernet frame
    /// the network sends the station. Returns what the station sent, with when.
    fn run(&mut self, sta: &mut sta::VirtualSta, from_us: u64, to_us: u64, ap_edit: &dyn Fn(&mut Vec<u8>),
           reply: &mut dyn FnMut(Vec<u8>) -> Option<Vec<u8>>) -> Vec<(u64, Vec<u8>)> {
        let mut sent = Vec::new();
        for now in (from_us..to_us).step_by(1_000) {
            for a in self.ap.step(now) {
                let mut f = a.frame;
                ap_edit(&mut f);
                if let Some(eth) = sta.on_ap_tx(&with_fcs_slot(f), now) { sta.host_rx(&eth, now); }
            }
            for a in sta.step(now) {
                sent.push((now, a.frame.clone()));
                // a protected frame's CCMP header off: the payload behind it is in the clear
                let clear = |d: Vec<u8>| if d.len() >= 32 && d[1] & 0x40 != 0 { [&d[..24], &d[32..]].concat() } else { d };
                let Some(eth) = self.ap.on_station_tx(&with_fcs_slot(a.frame), now).and_then(|d| data_to_eth(&clear(d))) else { continue };
                self.net.sta_mac = sta.cfg.mac;
                for r in self.net.handle(&eth, now) {
                    let Some(r) = reply(r) else { continue };
                    if let Some(f) = self.ap.data_from_ds(&r) { self.ap.queue.push(AirFrame { at_us: now, frame: f }); }
                }
            }
        }
        sent
    }

    /// Join with an open network and take the lease, with `reply` applied as in `run`: the time
    /// the station has the lease, and what it sent.
    fn lease(&mut self, sta: &mut sta::VirtualSta, reply: &mut dyn FnMut(Vec<u8>) -> Option<Vec<u8>>) -> u64 {
        for t in (0..2_000_000).step_by(1_000) {
            self.run(sta, t, t + 1_000, &|_| {}, reply);
            if sta.lease.is_some() { return t + 1_000; }
        }
        panic!("no lease: {}", sta.report())
    }
}

fn no_edit(r: Vec<u8>) -> Option<Vec<u8>> { Some(r) }

/// A null data frame to the DS: frame control 0x0148.
fn is_null(f: &[u8]) -> bool { f.len() >= 24 && f[..2] == [0x48, 0x01] }

/// What a DHCP message from the station says: the Ethernet and IP destinations, the IP source,
/// the broadcast flag, `ciaddr`, the message type and the options it carries.
#[derive(Debug)]
struct DhcpOut { eth_dst: [u8; 6], ip_src: [u8; 4], ip_dst: [u8; 4], broadcast: bool, ciaddr: [u8; 4], kind: u8, options: Vec<u8> }

fn dhcp_out(f: &[u8]) -> Option<DhcpOut> {
    // an open network's data frame to the DS: the header, LLC/SNAP, IPv4, UDP, BOOTP
    if f.len() < 24 + 8 + 28 + 240 || f[..2] != [0x08, 0x01] || f[30..32] != [0x08, 0x00] { return None; }
    let (ip, m) = (&f[32..], &f[60..]);
    if ip[9] != 17 || ip[22..24] != [0, 67] { return None; }
    let (mut kind, mut options, mut i) = (0, Vec::new(), 240);
    while i + 1 < m.len() && m[i] != 255 {
        if m[i] == 53 { kind = m[i + 2]; }
        options.push(m[i]);
        i += 2 + m[i + 1] as usize;
    }
    let a = |s: &[u8]| -> [u8; 4] { s[..4].try_into().unwrap() };
    Some(DhcpOut { eth_dst: f[16..22].try_into().unwrap(), ip_src: a(&ip[12..]), ip_dst: a(&ip[16..]), broadcast: m[10] & 0x80 != 0,
                   ciaddr: a(&m[12..]), kind, options })
}

/// `VirtualNet`'s ACK with its 86400 s lease (option 51) given as `lease_s`, and options 58 and 59
/// added when given; other frames as they are.
fn ack_with(lease_s: u32, t1_t2: Option<(u32, u32)>) -> impl FnMut(Vec<u8>) -> Option<Vec<u8>> {
    move |mut r: Vec<u8>| {
        let Some(i) = r.windows(6).position(|w| w == [51, 4, 0, 1, 0x51, 0x80]) else { return Some(r) };
        r[i + 2..i + 6].copy_from_slice(&lease_s.to_be_bytes());
        if let Some((t1, t2)) = t1_t2 {
            let extra = [&[58, 4][..], &t1.to_be_bytes(), &[59, 4], &t2.to_be_bytes()].concat();
            r.splice(i + 6..i + 6, extra);
            // the IP total length and UDP length grow by the 12 bytes; IPv4's checksum is not checked
            for at in [16, 38] { let n = u16::from_be_bytes([r[at], r[at + 1]]) + 12; r[at..at + 2].copy_from_slice(&n.to_be_bytes()); }
        }
        Some(r)
    }
}

#[test]
fn station_sends_no_keepalive_before_it_has_joined() {
    // the wrong passphrase: beacons, authentication, association and handshakes, never joined
    let (mut sta, mut g) = (station("ssid=esp32sim,psk=not-the-pass"), Guest::new());
    g.ap = VirtualAp::new(ApConfig { psk: Some("esp32sim-pass".into()), ..config() }, false);
    let sent = g.run(&mut sta, 0, 30_000_000, &|_| {}, &mut no_edit);
    assert!(sta.attempts >= 5 && sta.stats.0 == 0, "{}", sta.report());
    assert!(!sent.iter().any(|(_, f)| is_null(f)), "a keep-alive before the join");
    assert_eq!(sta.keepalives, 0);
    // nothing on the air: the station sends nothing at all, and is idle throughout
    let mut sta = station("ssid=esp32sim");
    for t in (0..30_000_000).step_by(1_000) { assert!(sta.idle_at(t) && sta.step(t).is_empty(), "at {t}"); }
    // a station that left an access point which then went silent
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut no_edit);
    let mut deauth = g.ap.beacon_like(12, &sta.cfg.mac, 0)[..24].to_vec();
    deauth.extend_from_slice(&3u16.to_le_bytes());
    sta.on_ap_tx(&with_fcs_slot(deauth), leased);
    for t in (leased..leased + 30_000_000).step_by(1_000) { assert!(sta.step(t).is_empty(), "at {t}"); }
    assert_eq!((sta.state, sta.keepalives), (sta::StaLink::Listening, 0));
}

#[test]
fn a_joined_station_sends_a_keepalive_after_the_interval_with_nothing_else_sent() {
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut no_edit);
    let sent = g.run(&mut sta, leased, leased + 10_000_000, &|_| {}, &mut no_edit);
    // the REQUEST went out just before the lease; nothing then until the keep-alive, 8 s after it
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (due, null) = &sent[0];
    assert!(is_null(null), "{null:02x?}");
    assert_eq!((&null[4..10], &null[10..16], &null[16..22], null.len()), (&config().bssid[..], &sta.cfg.mac[..], &config().bssid[..], 24));
    assert!(*due > leased + 7_000_000 && *due <= leased + 8_000_000, "the keep-alive at {due}, the lease at {leased}");
    assert!(sta.report().contains(", 1 keep-alives"), "{}", sta.report());
    // the next one 8 s after it, as nothing else went out
    let sent = g.run(&mut sta, due + 1_000, due + 9_000_000, &|_| {}, &mut no_edit);
    assert_eq!(sent.iter().map(|(t, f)| (*t - due, is_null(f))).collect::<Vec<_>>(), [(8_000_000, true)]);
}

#[test]
fn idle_at_is_false_exactly_when_a_keepalive_is_due() {
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut no_edit);
    let at = g.run(&mut sta, leased, leased + 9_000_000, &|_| {}, &mut no_edit)[0].0;
    // the same run again, stopped 1 ms before the 1-ms step that sent it
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut no_edit);
    assert!(g.run(&mut sta, leased, at - 1_000, &|_| {}, &mut no_edit).is_empty());
    let due = (at - 1_000..=at).find(|&t| !sta.idle_at(t)).expect("idle_at true through the step that sent it");
    assert!(sta.step(due - 1).is_empty() && sta.keepalives == 0, "idle just before");
    let f = sta.step(due);
    assert!(f.len() == 1 && is_null(&f[0].frame) && sta.keepalives == 1, "{} frames", f.len());
    assert!(sta.idle_at(due), "and idle again once sent");
}

#[test]
fn a_station_whose_relay_carries_frames_sends_no_keepalive() {
    let mut link = StationLink::default();
    link.set_relay(true);
    link.attach_station(station("ssid=esp32sim")).unwrap();
    let mut ap = VirtualAp::new(config(), false);
    let mac = link.station().unwrap().cfg.mac;
    let eth = [vec![0xff; 6], mac.to_vec(), vec![0x08, 0x06], vec![0; 28]].concat();
    let mut nulls = Vec::new();
    for now in (0..60_000_000u64).step_by(1_000) {
        for a in ap.step(now) { link.station_tx(&with_fcs_slot(a.frame), now); }
        // the host program sends every 5 s for 30 s, then stops
        if now < 30_000_000 && now % 5_000_000 == 0 && link.station().unwrap().state == sta::StaLink::Joined { link.receive_relay_frame(&eth).unwrap(); }
        if let Some(f) = link.next_rx(now, false) {
            if is_null(&f) { nulls.push(now); }
            ap.on_station_tx(&with_fcs_slot(f), now);
        }
        link.take_relay_frames();
    }
    // the relay's last frame at 25 s: keep-alives at 33, 41, 49 and 57 s, none while it talked
    assert_eq!(nulls, [33_000_000, 41_000_000, 49_000_000, 57_000_000]);
    assert!(link.report().contains(", 4 keep-alives"), "{}", link.report());
}

/// A BSS Max Idle Period element (ID 90) of `period` units of 1000 TU, appended to the access
/// point's association response.
fn max_idle(period: u16, options: u8) -> impl Fn(&mut Vec<u8>) {
    move |f: &mut Vec<u8>| if f[0] == 0x10 { f.extend_from_slice(&[90, 3]); f.extend_from_slice(&period.to_le_bytes()); f.push(options); }
}

#[test]
fn a_bss_max_idle_period_shortens_the_keepalive_interval() {
    // 4 units: 4.096 s, so a keep-alive every 2.048 s
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let sent = g.run(&mut sta, 0, 12_000_000, &max_idle(4, 0), &mut no_edit);
    let nulls: Vec<u64> = sent.iter().filter(|(_, f)| is_null(f)).map(|(t, _)| *t).collect();
    assert!(nulls.len() >= 4, "{nulls:?}");
    assert!(nulls.windows(2).all(|w| w[1] - w[0] == 2_048_000), "{nulls:?}");
    // a long period leaves the station's own interval: the access point may also have a shorter
    // inactive time of its own
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let sent = g.run(&mut sta, 0, 20_000_000, &max_idle(100, 0), &mut no_edit);
    let nulls: Vec<u64> = sent.iter().filter(|(_, f)| is_null(f)).map(|(t, _)| *t).collect();
    assert!(nulls.windows(2).all(|w| w[1] - w[0] == 8_000_000) && !nulls.is_empty(), "{nulls:?}");
}

#[test]
fn the_keepalive_timer_starts_again_after_leaving_and_joining() {
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut no_edit);
    // 5 s in, the access point deauthenticates the station, which listens and joins again
    g.run(&mut sta, leased, leased + 5_000_000, &|_| {}, &mut no_edit);
    let mut deauth = g.ap.beacon_like(12, &sta.cfg.mac, 0)[..24].to_vec();
    deauth.extend_from_slice(&3u16.to_le_bytes());
    sta.on_ap_tx(&with_fcs_slot(deauth), leased + 5_000_000);
    assert_eq!(sta.state, sta::StaLink::Listening);
    let sent = g.run(&mut sta, leased + 5_000_000, leased + 20_000_000, &|_| {}, &mut no_edit);
    assert_eq!((sta.stats.0, sta.state), (2, sta::StaLink::Joined), "joined again: {}", sta.report());
    let last = sent.iter().filter(|(_, f)| !is_null(f)).map(|(t, _)| *t).max().unwrap();
    let nulls: Vec<u64> = sent.iter().filter(|(_, f)| is_null(f)).map(|(t, _)| *t).collect();
    // none at the first join's 8 s, none while listening: the first one 8 s after the second
    // join's last frame (which went out up to 1 ms before the step that sent it)
    assert!(!nulls.is_empty() && nulls[0] > last + 7_999_000 && nulls[0] <= last + 8_000_000, "{nulls:?}, the last frame at {last}");
    assert!(nulls.windows(2).all(|w| w[1] - w[0] == 8_000_000), "{nulls:?}");
}

/// The DHCP messages among what the station sent, with when, relative to `since`.
fn dhcp_sent(sent: &[(u64, Vec<u8>)], since: u64) -> Vec<(u64, DhcpOut)> {
    sent.iter().filter_map(|(t, f)| dhcp_out(f).map(|d| (t - since, d))).collect()
}

/// Every ACK dropped, as a server that does not answer.
fn no_ack(r: Vec<u8>) -> Option<Vec<u8>> { if r.windows(3).any(|w| w == [53, 1, 5]) { None } else { Some(r) } }

/// An ARP request for `ip` from the gateway, as the network would send it.
fn arp_for(ip: [u8; 4]) -> Vec<u8> {
    let gw = crate::net::VirtualNet::new(false).gw_mac;
    [&[0xff; 6][..], &gw, &[0x08, 0x06], &[0, 1, 8, 0, 6, 4, 0, 1], &gw, &[10, 0, 2, 2], &[0; 6], &ip].concat()
}

#[test]
fn the_lease_renews_at_t1_unicast_and_rebinds_at_t2_broadcast() {
    // options 58 and 59: T1 20 s, T2 40 s of a 100 s lease; then the server stops answering
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut ack_with(100, Some((20, 40))));
    let sent = g.run(&mut sta, leased, leased + 60_000_000, &|_| {}, &mut no_ack);
    let d = dhcp_sent(&sent, leased);
    assert_eq!(d.len(), 2, "{d:?}");
    let (gw, me) = (crate::net::VirtualNet::new(false).gw_mac, [10, 0, 2, 15]);
    // RENEWING: a REQUEST to the server, unicast, from the address held, `ciaddr` set, no 50 or 54
    let (t, r) = &d[0];
    assert!((19_998_000..=20_000_000).contains(t), "renewing at {t}");
    assert_eq!((r.kind, r.eth_dst, r.ip_src, r.ip_dst, r.ciaddr, r.broadcast), (3, gw, me, [10, 0, 2, 2], me, false));
    assert!(!r.options.contains(&50) && !r.options.contains(&54), "{:?}", r.options);
    // REBINDING: the same REQUEST to every server; no retransmission between, as the 60 s minimum is past T2
    let (t, r) = &d[1];
    assert!((39_998_000..=40_000_000).contains(t), "rebinding at {t}");
    assert_eq!((r.kind, r.eth_dst, r.ip_src, r.ip_dst, r.ciaddr, r.broadcast), (3, [0xff; 6], me, [255; 4], me, false));
    assert!(!r.options.contains(&50) && !r.options.contains(&54), "{:?}", r.options);
    assert_eq!(sta.lease, Some(me), "still held until its end");
}

#[test]
fn without_options_58_and_59_t1_and_t2_are_half_and_seven_eighths_of_the_lease() {
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut ack_with(80, None));
    let sent = g.run(&mut sta, leased, leased + 79_000_000, &|_| {}, &mut no_ack);
    let at: Vec<(u64, bool)> = dhcp_sent(&sent, leased).iter().map(|(t, r)| (t.div_ceil(1_000_000), r.eth_dst == [0xff; 6])).collect();
    assert_eq!(at, [(40, false), (70, true)], "renewing at T1 = 40 s, rebinding at T2 = 70 s");
}

#[test]
fn an_ack_while_renewing_binds_again_with_its_new_times() {
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut ack_with(100, Some((20, 40))));
    let sent = g.run(&mut sta, leased, leased + 50_000_000, &|_| {}, &mut ack_with(100, Some((20, 40))));
    let at: Vec<u64> = dhcp_sent(&sent, leased).iter().map(|(t, r)| { assert_eq!(r.eth_dst, crate::net::VirtualNet::new(false).gw_mac); *t }).collect();
    // each ACK starts T1 again: the second renewal 20 s after the first one's ACK, a few ms after it
    assert!(at.len() == 2 && at[0] <= 20_000_000 && (20_000_000..20_010_000).contains(&(at[1] - at[0])), "{at:?}");
    assert_eq!((sta.lease, sta.renewals), (Some([10, 0, 2, 15]), 2));
    assert!(sta.report().contains(", 2 lease renewals"), "{}", sta.report());
}

#[test]
fn a_nak_while_renewing_gives_the_address_up_and_starts_again_with_discover() {
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut ack_with(100, Some((20, 40))));
    let mut nak_once = true;
    let mut nak = |mut r: Vec<u8>| {
        if let Some(i) = r.windows(3).position(|w| w == [53, 1, 5]).filter(|_| nak_once) { r[i + 2] = 6; nak_once = false; }
        Some(r)
    };
    let sent = g.run(&mut sta, leased, leased + 21_000_000, &|_| {}, &mut nak);
    let kinds: Vec<(u8, bool)> = dhcp_sent(&sent, leased).iter().map(|(_, r)| (r.kind, r.ciaddr == [0; 4])).collect();
    // the renewal, then DISCOVER and REQUEST from nothing, and the address leased again
    assert_eq!(kinds, [(3, false), (1, true), (3, true)]);
    assert_eq!((sta.lease, sta.renewals), (Some([10, 0, 2, 15]), 0));
}

#[test]
fn at_the_leases_end_arp_stops_answering_and_the_station_discovers_again() {
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut ack_with(100, Some((20, 40))));
    g.run(&mut sta, leased, leased + 99_000_000, &|_| {}, &mut no_ack);
    // still held: ARP answers
    sta.take_sent();
    sta.host_rx(&arp_for([10, 0, 2, 15]), leased + 99_000_000);
    assert_eq!(sta.take_sent().len(), 1, "an ARP reply before the end");
    sta.step(leased + 99_001_000);
    let sent = g.run(&mut sta, leased + 99_002_000, leased + 101_000_000, &|_| {}, &mut |_| None);
    let d = dhcp_sent(&sent, leased);
    assert_eq!(d.iter().map(|(t, r)| (t.div_ceil(1_000_000), r.kind, r.ciaddr)).collect::<Vec<_>>(), [(100, 1, [0; 4])], "DISCOVER at the end");
    assert_eq!(sta.lease, None);
    sta.take_sent();
    sta.host_rx(&arp_for([10, 0, 2, 15]), leased + 101_000_000);
    assert!(!sta.take_sent().iter().any(|(_, e)| e[12..14] == [0x08, 0x06]), "no ARP reply for the address given up");
    assert!(sta.report().contains("lease none"), "{}", sta.report());
}

#[test]
fn a_lease_without_a_lease_time_is_never_renewed() {
    // option 51 taken out (its six bytes as padding), and 0xffffffff, the infinite lease
    for lease in [None, Some(u32::MAX)] {
        let edit = move |mut r: Vec<u8>| {
            if let Some(i) = r.windows(6).position(|w| w == [51, 4, 0, 1, 0x51, 0x80]) {
                match lease { Some(l) => r[i + 2..i + 6].copy_from_slice(&l.to_be_bytes()), None => r[i..i + 6].fill(0) }
            }
            Some(r)
        };
        let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
        let mut edit = edit;
        let leased = g.lease(&mut sta, &mut edit);
        let sent = g.run(&mut sta, leased, leased + 200_000_000, &|_| {}, &mut edit);
        assert!(dhcp_sent(&sent, leased).is_empty(), "{lease:?}");
        assert_eq!(sta.lease, Some([10, 0, 2, 15]));
    }
}

#[test]
fn where_the_access_point_requires_protected_keepalives_the_station_sends_protected_data() {
    // WPA2, period 4 with "protected keep-alive required"; no host on the station, so nothing but
    // keep-alives once joined
    for (options, protected) in [(1, true), (0, false)] {
        let (mut sta, mut g) = (station("ssid=esp32sim,psk=esp32sim-pass"), Guest::new());
        g.ap = VirtualAp::new(ApConfig { psk: Some("esp32sim-pass".into()), ..config() }, false);
        sta.host = false;
        let sent = g.run(&mut sta, 0, 10_000_000, &max_idle(4, options), &mut no_edit);
        assert_eq!(sta.state, sta::StaLink::Joined, "{}", sta.report());
        let (eapol_out, rest): (Vec<_>, Vec<_>) = sent.iter().filter(|(_, f)| f[0] & 0x0c == 0x08).partition(|(_, f)| f.windows(2).any(|w| w == [0x88, 0x8e]));
        assert_eq!(eapol_out.len(), 2, "messages 2 and 4");
        assert!(rest.len() >= 3 && rest.windows(2).all(|w| w[1].0 - w[0].0 == 2_048_000), "{:?}", rest.iter().map(|r| r.0).collect::<Vec<_>>());
        for (_, f) in &rest {
            if protected {
                // data, to-DS, protected; the CCMP header, LLC/SNAP for IPv4, no payload, the MIC
                assert_eq!((&f[..2], f.len(), &f[32..38], &f[38..40]), (&[0x08, 0x41][..], 24 + 8 + 8 + 8, &[0xaa, 0xaa, 3, 0, 0, 0][..], &[0x08, 0x00][..]));
            } else { assert!(is_null(f), "{f:02x?}"); }
        }
        assert_eq!(sta.keepalives as usize, rest.len());
    }
}

#[test]
fn a_protected_keepalive_is_not_captured_and_an_open_network_gets_null_frames() {
    // WPA2 with protected keep-alives, the station the host: a lease, then keep-alives that are
    // protected data frames and stay out of the capture, which holds what the guest's network sees
    let (mut sta, mut g) = (station("ssid=esp32sim,psk=esp32sim-pass"), Guest::new());
    g.ap = VirtualAp::new(ApConfig { psk: Some("esp32sim-pass".into()), ..config() }, false);
    let mut t = 0;
    while sta.lease.is_none() { g.run(&mut sta, t, t + 1_000, &max_idle(4, 1), &mut no_edit); t += 1_000; assert!(t < 3_000_000, "{}", sta.report()); }
    sta.take_sent();
    let sent = g.run(&mut sta, t, t + 10_000_000, &max_idle(4, 1), &mut no_edit);
    assert!(sent.len() >= 4 && sent.iter().all(|(_, f)| f[..2] == [0x08, 0x41]), "protected data frames only");
    assert!(sta.take_sent().is_empty(), "keep-alives in the capture");
    // an open network has no key to protect one with: null frames
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    sta.host = false;
    let sent = g.run(&mut sta, 0, 10_000_000, &max_idle(4, 1), &mut no_edit);
    let after_join: Vec<_> = sent.iter().filter(|(_, f)| f[0] & 0x0c == 0x08).collect();
    assert!(after_join.len() >= 4 && after_join.iter().all(|(_, f)| is_null(f)), "{:?}", after_join.iter().map(|(_, f)| &f[..2]).collect::<Vec<_>>());
}

#[test]
fn with_the_relay_switched_on_the_station_gives_up_its_own_dhcp() {
    // a lease with T1 at 20 s, then the station stops being the host
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut ack_with(100, Some((20, 40))));
    sta.set_host(false);
    assert_eq!(sta.lease, None);
    let sent = g.run(&mut sta, leased, leased + 200_000_000, &|_| {}, &mut ack_with(100, Some((20, 40))));
    assert!(dhcp_sent(&sent, leased).is_empty(), "DHCP under the host program's MAC");
    assert_eq!(sta.state, sta::StaLink::Joined, "still carrying the relay");
    // the relay off again: the station is the host once more and takes a lease
    sta.set_host(true);
    g.run(&mut sta, leased + 200_000_000, leased + 201_000_000, &|_| {}, &mut no_edit);
    assert_eq!(sta.lease, Some([10, 0, 2, 15]));
    // and through `StationLink`, as the relay is switched
    let mut link = StationLink::default();
    link.attach_station(station("ssid=esp32sim")).unwrap();
    let (mut ap, mut net) = (VirtualAp::new(config(), false), crate::net::VirtualNet::new(false));
    for now in (0..1_000_000).step_by(100) {
        for a in ap.step(now) { link.station_tx(&with_fcs_slot(a.frame), now); }
        let Some(f) = link.next_rx(now, false) else { continue };
        let Some(eth) = ap.on_station_tx(&with_fcs_slot(f), now).and_then(|d| data_to_eth(&d)) else { continue };
        net.sta_mac = link.station().unwrap().cfg.mac;
        for reply in net.handle(&eth, now) { if let Some(f) = ap.data_from_ds(&reply) { ap.queue.push(AirFrame { at_us: now, frame: f }); } }
    }
    assert!(link.station().unwrap().lease.is_some());
    link.set_relay(true);
    assert_eq!(link.station().unwrap().lease, None);
}

#[test]
fn a_lease_of_0_s_is_taken_as_1_s() {
    // a server that leases for 0 s: the station renews twice a second, not in every round
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut ack_with(0, None));
    let sent = g.run(&mut sta, leased, leased + 10_000_000, &|_| {}, &mut ack_with(0, None));
    let n = dhcp_sent(&sent, leased).len();
    assert!((18..=22).contains(&n), "{n} DHCP messages in 10 s");
    assert_eq!(sta.lease, Some([10, 0, 2, 15]));
}

#[test]
fn a_t1_or_t2_of_0_s_is_taken_as_1_s() {
    // options 58 and 59 of 0 s with a 100 s lease: one renewal a second, not one in every round
    for (t1, t2) in [(0, 50), (0, 0), (100, 0)] {
        let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
        let leased = g.lease(&mut sta, &mut ack_with(100, Some((t1, t2))));
        let sent = g.run(&mut sta, leased, leased + 10_000_000, &|_| {}, &mut ack_with(100, Some((t1, t2))));
        let n = dhcp_sent(&sent, leased).len();
        assert!((9..=11).contains(&n), "T1 {t1}, T2 {t2}: {n} DHCP messages in 10 s");
        assert_eq!(sta.lease, Some([10, 0, 2, 15]));
    }
}

#[test]
fn a_renewal_without_an_answer_is_sent_again_after_half_the_time_left_at_least_60_s() {
    // ESP-IDF's default lease, 7200 s: T1 3600 s, T2 6300 s. The server stops answering. Time goes
    // on in 1 s steps with a beacon each, which `idle_at` makes exact for the station
    let (mut sta, mut g) = (station("ssid=esp32sim"), Guest::new());
    let leased = g.lease(&mut sta, &mut ack_with(7200, None));
    let mut at = Vec::new();
    for s in 1..=7300u64 {
        let now = leased + s * 1_000_000;
        sta.on_ap_tx(&with_fcs_slot(g.ap.beacon_like(8, &[0xff; 6], now)), now);
        for a in sta.step(now) { if let Some(d) = dhcp_out(&a.frame) { at.push((s, d.kind, d.eth_dst == [0xff; 6])); } }
    }
    // renewing: 3600, then half of what is left to T2, at least 60 s (4950, 5625, 5962.5, 6131.25,
    // 6215.6, 6275.6); rebinding at T2, 6300, then half of what is left to the end (6750, 6975,
    // 7087.5, 7147.5); DISCOVER at the end, 7200. Each goes out 200 us after its time, so in the
    // 1 s step after it
    let renewing: Vec<u64> = at.iter().filter(|a| a.1 == 3 && !a.2).map(|a| a.0).collect();
    let rebinding: Vec<u64> = at.iter().filter(|a| a.1 == 3 && a.2).map(|a| a.0).collect();
    assert_eq!(renewing, [3601, 4951, 5626, 5964, 6133, 6217, 6277]);
    assert_eq!(rebinding, [6301, 6751, 6976, 7089, 7149]);
    assert_eq!(at.iter().find(|a| a.1 == 1).map(|a| a.0), Some(7201));
    assert_eq!(sta.lease, None);
}
