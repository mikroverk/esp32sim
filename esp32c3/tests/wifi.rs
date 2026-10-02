use emu_core::Bus;
const STATION: [u8; 6] = [2, 0, 0, 0, 0, 3];
/// AES-128 ECB of the FIPS-197 vector through GDMA peripheral 6, which is how ESP-IDF's driver
/// (and with it the WPA2 key unwrap) uses the block: the OUT chain in, the IN chain out, DONE.
#[test]
fn aes_runs_through_gdma() {
    let mut m = esp32c3::machine(STATION, 4 << 20);
    const AES: u32 = 0x6003_a000;
    let key = [0x0001_0203u32, 0x0405_0607, 0x0809_0a0b, 0x0c0d_0e0f];
    let plain: [u8; 16] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];
    let cipher: [u8; 16] = [
        0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5,
        0x5a,
    ];
    let (out_desc, in_desc, src, dst) = (0x3fc9_0000u32, 0x3fc9_0010, 0x3fc9_0100, 0x3fc9_0200);
    for (i, b) in plain.iter().enumerate() {
        m.bus.write8(src + i as u32, *b).unwrap();
    }
    m.bus
        .write32(out_desc, 1 << 31 | 1 << 30 | 16 << 12 | 16)
        .unwrap();
    m.bus.write32(out_desc + 4, src).unwrap();
    m.bus.write32(in_desc, 1 << 31 | 16).unwrap();
    m.bus.write32(in_desc + 4, dst).unwrap();
    const GDMA: u32 = 0x6003_f000;
    m.bus.write32(GDMA + 0xa0, 6).unwrap(); // channel 0 IN peripheral
    m.bus.write32(GDMA + 0x100, 6).unwrap(); // channel 0 OUT peripheral
    m.bus
        .write32(GDMA + 0x80, (in_desc & 0xfffff) | 1 << 22)
        .unwrap();
    m.bus
        .write32(GDMA + 0xe0, (out_desc & 0xfffff) | 1 << 21)
        .unwrap();
    for (i, k) in key.iter().enumerate() {
        m.bus.write32(AES + 4 * i as u32, k.swap_bytes()).unwrap();
    }
    m.bus.write32(AES + 0x40, 0).unwrap(); // AES-128 encrypt
    m.bus.write32(AES + 0x90, 1).unwrap();
    m.bus.write32(AES + 0x94, 0).unwrap();
    m.bus.write32(AES + 0x98, 1).unwrap();
    m.bus.write32(AES + 0x48, 1).unwrap(); // trigger
    m.bus.tick(160_000);

    assert_eq!(
        m.bus.read32(AES + 0x4c).unwrap(),
        2,
        "aes_hal_wait_done sees DONE"
    );
    let got: Vec<u8> = (0..16).map(|i| m.bus.read8(dst + i).unwrap()).collect();
    assert_eq!(got, cipher);
    let dw0 = m.bus.read32(in_desc).unwrap();
    assert_eq!(
        (dw0 >> 12 & 0xfff, dw0 >> 30),
        (16, 1),
        "16 bytes, SUC_EOF, owner back with the CPU"
    );
    assert!(!m.bus.periph.gdma.inp[0].running && !m.bus.periph.gdma.out[0].running);
}

#[test]
fn calibration_and_mac_events_follow_c3_registers() {
    let mut p = esp32c3::periph::Peripherals::new([0; 6]);
    assert_eq!(p.read32(0x60006174) & (1 << 16), 0);
    p.write32(0x60006144, 3);
    p.tick(158);
    assert_eq!(p.read32(0x60006174) & (1 << 16), 0);
    p.tick(2);
    assert_ne!(p.read32(0x60006174) & (1 << 16), 0);
    p.write32(0x60006144, 0);
    p.write32(0x60006144, 3);
    assert_eq!(p.read32(0x60006174) & (1 << 16), 0);
    p.write32(0x6000e000, 1 << 24 | 0xa5 << 16 | 0x0362);
    p.write32(0x6000e004, 0x0362);
    assert_eq!(p.read32(0x6000e004), 0xa50362);
    p.write32(0x60033d14, 2);
    assert_eq!(p.read32(0x60033d14) & 3, 3);
    p.wifi.tx_done(0);
    assert_ne!(p.read32(0x60033c3c) & (1 << 7), 0);
    p.write32(0x60033c40, 1 << 7);
    assert_eq!(p.read32(0x60033c3c), 0);
    p.write32(0x60033cac, 1);
    assert_eq!(p.read32(0x60033cb0), 0);
}
