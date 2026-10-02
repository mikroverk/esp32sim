//! Ethernet relay through guest Wi-Fi descriptors on all three chips.
use esp_soc::net::VirtualNet;
use esp_soc::wifi::{ApConfig, StaState, VirtualAp};
use esp_soc::SocBus;

const STA: [u8; 6] = [2, 0, 0, 0, 0, 3];
const AP: [u8; 6] = [2, 0x53, 0x49, 0x4d, 0, 1];

fn discover() -> Vec<u8> {
    let mut e = vec![0; 14 + 20 + 8 + 244];
    e[..6].fill(255);
    e[6..12].copy_from_slice(&STA);
    e[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
    let ip = &mut e[14..34];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&272u16.to_be_bytes());
    ip[8] = 64;
    ip[9] = 17;
    ip[16..20].fill(255);
    let mut sum: u32 = ip
        .as_chunks::<2>().0.iter()
        .map(|w| u16::from_be_bytes([w[0], w[1]]) as u32)
        .sum();
    while sum > 65535 {
        sum = (sum & 65535) + (sum >> 16);
    }
    ip[10..12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
    e[34..42].copy_from_slice(&[0, 68, 0, 67, 0, 252, 0, 0]);
    let d = &mut e[42..];
    d[..3].copy_from_slice(&[1, 1, 6]);
    d[4..8].copy_from_slice(&0x12345678u32.to_be_bytes());
    d[28..34].copy_from_slice(&STA);
    d[236..].copy_from_slice(&[99, 130, 83, 99, 53, 1, 1, 255]);
    e
}

fn ap() -> VirtualAp {
    let mut ap = VirtualAp::new(ApConfig::parse("").unwrap(), false);
    // These tests start after association; firmware receipts cover the connection exchange.
    ap.state = StaState::Associated;
    ap.sta = STA;
    ap
}

type BusCase = (Box<dyn SocBus>, u32, u32, u32, bool);

fn buses() -> Vec<BusCase> {
    let mut s3 = esp32s3::bus::SocBus::new(1 << 20, 0, STA);
    s3.periph.wifi.ap = Some(ap());
    s3.periph.wifi.net = Some(VirtualNet::new(false));
    s3.periph.wifi.net.as_mut().unwrap().nat = Some(esp_soc::nat::Nat::new(false));
    s3.refresh_tick_budget();
    let mut c3 = esp32c3::bus::SocBus::new(1 << 20, STA);
    c3.periph.wifi.ap = Some(ap());
    c3.periph.wifi.net = Some(VirtualNet::new(false));
    let mut c6 = esp32c6::bus::SocBus::new(1 << 20, STA);
    c6.periph.wifi_mac.ap = Some(ap());
    c6.periph.wifi_mac.net = Some(VirtualNet::new(false));
    c6.periph.wifi_mac.net.as_mut().unwrap().nat = Some(esp_soc::nat::Nat::new(false));
    vec![
        (Box::new(s3), 0x3fc90000, 0x60033000, 240_000, false),
        (Box::new(c3), 0x3fc90000, 0x60033000, 160_000, false),
        (Box::new(c6), 0x40810000, 0x600a4000, 160_000, true),
    ]
}

fn transmit(bus: &mut dyn SocBus, ram: u32, mac: u32, c6: bool, eth: &[u8]) {
    let mut f = vec![0x08, 0x01, 0, 0];
    f.extend_from_slice(&AP);
    f.extend_from_slice(&STA);
    f.extend_from_slice(&eth[..6]);
    f.extend_from_slice(&[0, 0, 0xaa, 0xaa, 3, 0, 0, 0]);
    f.extend_from_slice(&eth[12..]);
    let len = f.len();
    if c6 {
        let mut packet = (len as u32).to_le_bytes().to_vec();
        packet.extend_from_slice(&[0; 4]);
        packet.extend_from_slice(&f);
        f = packet;
    }
    bus.load_bytes(ram + 0x100, &f).unwrap();
    bus.write32(
        ram,
        3 << 30 | (f.len() as u32) << if c6 { 14 } else { 12 } | f.len() as u32,
    )
    .unwrap();
    bus.write32(ram + 4, ram + 0x100).unwrap();
    bus.write32(
        mac + if c6 { 0xd6c } else { 0xd08 },
        (ram & 0xfffff) | 3 << 30,
    )
    .unwrap();
}

fn arm_rx(bus: &mut dyn SocBus, ram: u32, mac: u32, c6: bool) {
    bus.write32(ram + 0x1000, 1 << 31 | 1700).unwrap();
    bus.write32(ram + 0x1004, ram + 0x2000).unwrap();
    bus.write32(ram + 0x1008, 0).unwrap();
    bus.write32(mac + if c6 { 0x84 } else { 0x88 }, ram + 0x1000)
        .unwrap();
}

fn received(bus: &mut dyn SocBus, ram: u32, c6: bool) -> Vec<u8> {
    let dw = bus.read32(ram + 0x1000).unwrap();
    assert_eq!(dw >> 30, 3, "RX descriptor filled");
    let header = if c6 { 92 } else { 48 };
    let len = (dw >> if c6 { 14 } else { 12 }) & if c6 { 0x3fff } else { 0xfff };
    let frame: Vec<_> = (0..len - header - 4)
        .map(|i| bus.read8(ram + 0x2000 + header + i).unwrap())
        .collect();
    let mut eth = frame[4..10].to_vec();
    eth.extend_from_slice(&frame[16..22]);
    eth.extend_from_slice(&frame[30..]);
    eth
}

#[test]
fn dhcp_discover_relay_and_default_network_use_the_same_guest_descriptors() {
    let discover = discover();
    let reply = VirtualNet::new(false).handle(&discover, 1000).remove(0);
    for (mut bus, ram, mac, cycles, c6) in buses() {
        bus.set_ethernet_relay(true).unwrap();
        transmit(&mut *bus, ram, mac, c6, &discover);
        bus.tick(cycles);
        assert_eq!(bus.take_ethernet_frames(), vec![discover.clone()]);
        assert!(bus.take_ethernet_frames().is_empty());
        arm_rx(&mut *bus, ram, mac, c6);
        bus.tick(cycles);
        assert_eq!(
            bus.read32(ram + 0x1000).unwrap() >> 30,
            2,
            "built-in DHCP must not answer in relay mode"
        );
        bus.receive_ethernet_frame(&reply).unwrap();
        bus.tick(cycles);
        bus.flush_ticks();
        assert_eq!(received(&mut *bus, ram, c6), reply);
        // Switching back resumes the original network, including its NAT object.
        bus.set_ethernet_relay(false).unwrap();
        arm_rx(&mut *bus, ram, mac, c6);
        transmit(&mut *bus, ram, mac, c6, &discover);
        bus.tick(cycles);
        bus.tick(cycles);
        bus.flush_ticks();
        assert!(bus.take_ethernet_frames().is_empty());
        assert_eq!(received(&mut *bus, ram, c6), reply);
    }
}

#[test]
fn relay_bounds_mode_changes_and_reset() {
    for (mut bus, ram, mac, cycles, c6) in buses() {
        assert!(bus.receive_ethernet_frame(&[0; 14]).is_err());
        bus.set_ethernet_relay(true).unwrap();
        for len in [0, 13, 1519] {
            assert!(bus.receive_ethernet_frame(&vec![0; len]).is_err());
        }
        for len in [14, 1518].into_iter().cycle().take(64) {
            bus.receive_ethernet_frame(&vec![0; len]).unwrap();
        }
        assert!(bus.receive_ethernet_frame(&[0; 14]).is_err());
        bus.set_ethernet_relay(true).unwrap();
        assert!(
            bus.receive_ethernet_frame(&[0; 14]).is_err(),
            "same mode preserves queue"
        );
        bus.set_ethernet_relay(false).unwrap();
        bus.set_ethernet_relay(true).unwrap();
        bus.receive_ethernet_frame(&[0; 14]).unwrap();
        bus.reboot(STA);
        bus.receive_ethernet_frame(&[0; 14]).unwrap();
        for _ in 0..65 {
            transmit(&mut *bus, ram, mac, c6, &discover());
            bus.tick(cycles);
        }
        assert_eq!(bus.take_ethernet_frames().len(), 64);
    }
}
