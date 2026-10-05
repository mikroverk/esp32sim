use super::*;

fn config() -> ApConfig {
    ApConfig { ssid: "esp32sim".into(), bssid: [2, 0x53, 0x49, 0x4d, 0, 1], channel: 6, psk: None }
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
    let ap = l.ap.take();
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
