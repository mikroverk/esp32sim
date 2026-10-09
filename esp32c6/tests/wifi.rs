//! The C6 WiFi model as the closed library drives it: the handshakes it waits on, then frames
//! through the guest's descriptors on the bus — received into the RX ring, sent from a TX queue —
//! and the AES-through-GDMA path the WPA2 supplicant needs.
use esp32c6::wifi::{ModemBb, WifiMac, EVENT_RX, EVENT_TX_DONE};
use esp_periph::Device;
use esp_soc::wifi::{ApConfig, VirtualAp};
use riscv_rv32::bus::Bus;

const MAC: u32 = 0x600a_4000;
const STATION: [u8; 6] = [0xdc, 0x1e, 0xd5, 0x6e, 0x8c, 0xdc];
/// 1 ms of CPU time per tick, as many as it takes
fn run_ms(m: &mut esp32c6::Machine, ms: u32) { for _ in 0..ms { m.bus.tick(160_000); } }
fn machine_with_ap() -> esp32c6::Machine {
    let mut m = esp32c6::machine(STATION, 4 << 20);
    m.bus.periph.wifi_mac.link.attach(Some(VirtualAp::new(ApConfig::parse("").unwrap(), false)), None);
    m
}

/// The channel switch as the ROM's `freq_chan_en_sw` and the PHY library's
/// `ram_set_chan_freq_sw_start` perform it: index into +0xC0, pulse bit 14, poll +0xCC bit 8.
#[test]
fn channel_switch_is_done_at_the_start_pulse() {
    let mut bb = ModemBb::new();
    assert_eq!(Device::read(&mut bb, 0xcc) & (1 << 8), 0, "no switch was asked for yet");

    let index = 6u32;
    let v = Device::read(&mut bb, 0xc0) & 0xffff_c00f | (index << 7) & 0x3ff0;
    Device::write(&mut bb, 0xc0, v);
    assert_eq!(Device::read(&mut bb, 0xcc) & (1 << 8), 0, "the index alone starts nothing");
    Device::write(&mut bb, 0xc0, v | 1 << 14);
    assert_ne!(Device::read(&mut bb, 0xcc) & (1 << 8), 0, "done");
    Device::write(&mut bb, 0xc0, v);
    assert_ne!(Device::read(&mut bb, 0xcc) & (1 << 8), 0, "still done after the pulse ends");
    assert_eq!((bb.chan_index, bb.chan_switches), (6, 1));

    Device::write(&mut bb, 0xcc, 0xffff_feff);          // the done bit is the hardware's, not a latch of writes
    Device::write(&mut bb, 0xc0, v | 1 << 14);
    Device::write(&mut bb, 0xc0, v | 1 << 14);          // held high: one pulse, one switch
    assert_eq!(bb.chan_switches, 2);
    assert_eq!(Device::read(&mut bb, 0xc0), v | 1 << 14, "the rest of the block is plain register RAM");
}

/// `hal_init`: set +0xDDC bit 1, wait for bit 0.
#[test]
fn mac_core_reports_ready_to_hal_init() {
    let mut mac = WifiMac::new();
    let v = Device::read(&mut mac, 0xddc) | 2;
    Device::write(&mut mac, 0xddc, v);
    assert_eq!(Device::read(&mut mac, 0xddc) & 3, 3, "the request bit is kept, ready reads set");
}

/// `ram_iq_est_enable`: +0x474 bit 0 then bit 1, wait for +0x4A0 bit 16.
#[test]
fn iq_estimate_is_done_while_started() {
    let mut bb = ModemBb::new();
    Device::write(&mut bb, 0x474, 1);
    assert_eq!(Device::read(&mut bb, 0x4a0) & (1 << 16), 0, "enabled, not started");
    Device::write(&mut bb, 0x474, 3);
    assert_ne!(Device::read(&mut bb, 0x4a0) & (1 << 16), 0);
    Device::write(&mut bb, 0x474, 0);
    assert_eq!(Device::read(&mut bb, 0x4a0) & (1 << 16), 0, "the next estimate waits for its own start");
}

/// A beacon from the access point lands in the ring the way `wDev_ProcessRxSucData` reads it:
/// 14-bit size and length fields, the 92-byte control header with the frame's length at byte 84,
/// the frame and its FCS, the receive event, and the last-descriptor address split over +0x08C
/// and +0xC70.
#[test]
fn a_beacon_fills_the_next_rx_descriptor() {
    let mut m = machine_with_ap();
    let (d0, d1, b0, b1) = (0x4081_0000u32, 0x4081_000c, 0x4081_1000, 0x4081_2000);
    let armed = 1 << 31 | 1700 << 14 | 1700;                       // 0x81A906A4, as the library arms it
    for (d, b, next) in [(d0, b0, d1), (d1, b1, d0)] {
        m.bus.write32(d, armed).unwrap(); m.bus.write32(d + 4, b).unwrap(); m.bus.write32(d + 8, next).unwrap();
    }
    m.bus.write32(MAC + 0x084, d0).unwrap();                        // hal_mac_rx_set_base
    run_ms(&mut m, 99);
    assert_eq!(m.bus.read32(MAC + 0xc48).unwrap(), 0, "the first beacon is due at 100 ms");
    run_ms(&mut m, 2);

    let dw0 = m.bus.read32(d0).unwrap();
    let total = (dw0 >> 14) & 0x3fff;
    assert_eq!(dw0 & 0x3fff, 1700, "the size field is untouched");
    assert_eq!(dw0 >> 30, 3, "has_data, and the owner bit stays");
    let sig_len = m.bus.read32(b0 + 84).unwrap() & 0x3fff;
    assert_eq!(total, 92 + sig_len, "header + frame + FCS");
    assert_eq!(m.bus.read8(b0 + 88).unwrap(), 0, "receive state: good");
    assert_eq!(m.bus.read32(b0).unwrap() >> 28 & 3, 1, "a group frame matches filter 0 only");
    assert_eq!(m.bus.read16(b0 + 92).unwrap(), 0x0080, "frame control: a beacon");
    let frame: Vec<u8> = (0..sig_len - 4).map(|i| m.bus.read8(b0 + 92 + i).unwrap()).collect();
    assert!(frame.windows(8).any(|w| w == b"esp32sim"), "with the SSID in it");
    assert_eq!(m.bus.read32(b0 + 92 + sig_len - 4).unwrap(), esp_soc::wifi::fcs(&frame), "the FCS follows");

    assert_eq!(m.bus.read32(MAC + 0xc48).unwrap(), EVENT_RX);
    assert_eq!(Device::irq_sources(&m.bus.periph.wifi_mac), 1, "the MAC's interrupt source is up");
    let last = m.bus.read32(MAC + 0xc70).unwrap() & 0xfff0_0000 | m.bus.read32(MAC + 0x08c).unwrap() & 0xf_ffff;
    assert_eq!(last, d0, "hal_mac_rx_get_last_dscr");
    assert_eq!(m.bus.read32(MAC + 0x088).unwrap(), d1 & 0xf_ffff, "the hardware moved on to the next descriptor");
    m.bus.write32(MAC + 0xc4c, EVENT_RX).unwrap();
    assert_eq!(Device::irq_sources(&m.bus.periph.wifi_mac), 0, "cleared by hal_mac_interrupt_clr_event");

    // the library has not recycled d0: the ring is not overrun, the next beacon goes to d1
    run_ms(&mut m, 110);
    assert_eq!(m.bus.read32(d1).unwrap() >> 30, 3);
    run_ms(&mut m, 110);
    assert_eq!(m.bus.periph.wifi_mac.link.rx_frames(), 2, "both descriptors are full; nothing is overwritten");
    assert!(m.bus.periph.wifi_mac.link.rx_dropped() > 0);
}

/// A probe request leaves queue 0: the packet is an 8-byte header (the frame's length in its
/// first word) and the frame; the queue completes, and the access point answers.
#[test]
fn a_probe_request_leaves_its_tx_queue_and_is_answered() {
    let mut m = machine_with_ap();
    let mut frame = vec![0x40, 0x00, 0x00, 0x00];                  // probe request
    frame.extend_from_slice(&[0xff; 6]); frame.extend_from_slice(&STATION); frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&[0x10, 0x00, 0x00, 0x00]);            // sequence; wildcard SSID
    let (desc, pkt) = (0x4081_0000u32, 0x4081_0100u32);
    m.bus.write32(pkt, frame.len() as u32).unwrap(); m.bus.write32(pkt + 4, 0).unwrap();
    for (i, b) in frame.iter().enumerate() { m.bus.write8(pkt + 8 + i as u32, *b).unwrap(); }
    m.bus.write32(desc, 3 << 30 | (frame.len() as u32 + 8) << 14 | 0x58).unwrap();
    m.bus.write32(desc + 4, pkt).unwrap();

    m.bus.write32(MAC + 0xd6c, desc & 0xf_ffff | 3 << 30).unwrap(); // hal_mac_txq_enable(0)
    run_ms(&mut m, 1);
    assert_eq!(m.bus.read32(MAC + 0xc48).unwrap(), EVENT_TX_DONE);
    assert_eq!(m.bus.read32(MAC + 0xcb8).unwrap(), 1, "queue 0 completed");
    assert_eq!(m.bus.read32(MAC + 0xd6c).unwrap() >> 30, 0, "the start bits are the hardware's to clear");
    m.bus.write32(MAC + 0xcb4, 1).unwrap();
    assert_eq!(m.bus.read32(MAC + 0xcb8).unwrap(), 0);
    assert_eq!(m.bus.periph.wifi_mac.link.ap().unwrap().stats.1, 1, "one probe response");

    // +0xCB8 and +0xC8C are 16-byte steps below queue 0 too, but past queue 10: not queues
    m.bus.write32(MAC + 0xc8c, 0xc000_0000).unwrap();
    run_ms(&mut m, 1);
    assert_eq!(m.bus.periph.wifi_mac.link.tx_frames(), 1);
}

/// AES-128 ECB of the FIPS-197 vector through GDMA peripheral 6, which is how ESP-IDF's driver
/// (and with it the WPA2 key unwrap) uses the block: the OUT chain in, the IN chain out, DONE.
#[test]
fn aes_runs_through_gdma() {
    let mut m = esp32c6::machine(STATION, 4 << 20);
    const AES: u32 = 0x6008_8000;
    let key = [0x0001_0203u32, 0x0405_0607, 0x0809_0a0b, 0x0c0d_0e0f];
    let plain: [u8; 16] = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff];
    let cipher: [u8; 16] = [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a];
    let (out_desc, in_desc, src, dst) = (0x4081_0000u32, 0x4081_0010, 0x4081_0100, 0x4081_0200);
    for (i, b) in plain.iter().enumerate() { m.bus.write8(src + i as u32, *b).unwrap(); }
    m.bus.write32(out_desc, 1 << 31 | 1 << 30 | 16 << 12 | 16).unwrap(); m.bus.write32(out_desc + 4, src).unwrap();
    m.bus.write32(in_desc, 1 << 31 | 16).unwrap(); m.bus.write32(in_desc + 4, dst).unwrap();
    { let g = &mut m.bus.periph.gdma.gdma;
      g.out[0].peri_sel = 6; g.out[0].desc = out_desc; g.out[0].running = true;
      g.inp[0].peri_sel = 6; g.inp[0].desc = in_desc; g.inp[0].running = true; }
    for (i, k) in key.iter().enumerate() { m.bus.write32(AES + 4 * i as u32, k.swap_bytes()).unwrap(); }
    m.bus.write32(AES + 0x40, 0).unwrap();                          // AES-128 encrypt
    m.bus.write32(AES + 0x90, 1).unwrap(); m.bus.write32(AES + 0x94, 0).unwrap(); m.bus.write32(AES + 0x98, 1).unwrap();
    m.bus.write32(AES + 0x48, 1).unwrap();                          // trigger
    run_ms(&mut m, 1);

    assert_eq!(m.bus.read32(AES + 0x4c).unwrap(), 2, "aes_hal_wait_done sees DONE");
    let got: Vec<u8> = (0..16).map(|i| m.bus.read8(dst + i).unwrap()).collect();
    assert_eq!(got, cipher);
    let dw0 = m.bus.read32(in_desc).unwrap();
    assert_eq!((dw0 >> 12 & 0xfff, dw0 >> 30), (16, 1), "16 bytes, SUC_EOF, owner back with the CPU");
    assert!(!m.bus.periph.gdma.gdma.inp[0].running && !m.bus.periph.gdma.gdma.out[0].running);
}

#[test]
fn txdc_and_calibration_status_follow_start_without_a_clock() {
    let mut bb = ModemBb::new();
    assert_eq!(bb.clock(), None);
    for (start, status, done) in [(0x418, 0x418, 1 << 22), (0x810, 0x814, 7 << 14)] {
        for _ in 0..2 {
            bb.write(start, 0);
            assert_eq!(bb.read(status) & done, 0);
            bb.write(status, done);
            assert_eq!(bb.read(status) & done, 0, "software cannot set DONE");
            bb.write(start, 1);
            assert_eq!(bb.read(status) & done, done);
            if status != start { assert_eq!(bb.read(status) & !done, 0, "ideal zero comparator output"); }
        }
    }
}

#[test]
fn txdc_comparator_outputs_ignore_software_writes() {
    let mut bb = ModemBb::new();
    bb.write(0x418, u32::MAX);
    assert_eq!(bb.read(0x418), 0x007f_ffff);
    bb.write(0x814, u32::MAX);
    assert_eq!(bb.read(0x814), 0);
}
