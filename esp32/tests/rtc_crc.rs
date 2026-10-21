use emu_core::bus::Bus as _;

const CONF: u32 = 0x3ff4_8100;
const RESULT: u32 = CONF + 4;
const START: u32 = 1 << 8;
const FINISH: u32 = 1 << 31;

#[test]
fn rtc_crc_start_completes_and_uses_fast_memory_range() {
    let mut bus = esp32::bus::SocBus::new(4 << 20, [0; 6]);
    bus.rtc_fast[4..16].copy_from_slice(b"123456789abc");
    bus.rtc_slow.fill(0xff);
    let config = (1 << 9) | (2 << 20) | 0x5a;
    bus.write32(CONF, config).unwrap();
    assert_eq!(bus.read32(CONF).unwrap(), config);
    assert_eq!(bus.read32(RESULT).unwrap(), 0);
    bus.write32(CONF, config | START).unwrap();
    assert_eq!(bus.read32(CONF).unwrap(), config | START | FINISH);
    // CRC-32/ISO-HDLC model vector, not a measured hardware vector.
    assert_eq!(bus.read32(RESULT).unwrap(), 0xbdb0_c0e4);
    bus.write32(CONF, config | FINISH).unwrap();
    assert_eq!(bus.read32(CONF).unwrap(), config, "FINISH cannot be forged");
    bus.write32(RESULT, 0x1234_5678).unwrap();
    assert_eq!(
        bus.read32(RESULT).unwrap(),
        0xbdb0_c0e4,
        "result is read-only"
    );
    bus.write32(0x400c_0004, 0x3433_3230).unwrap();
    bus.write32(CONF, config | START).unwrap();
    assert_ne!(
        bus.read32(RESULT).unwrap(),
        0xbdb0_c0e4,
        "fresh memory each start"
    );
}

#[test]
fn rtc_crc_alias_subword_full_and_wrapping_ranges() {
    let mut bus = esp32::bus::SocBus::new(4 << 20, [0; 6]);
    // LEN=0 means one word; AHB alias and byte writes share the APB path.
    bus.write32(0x6000_8100, 0x23).unwrap();
    bus.write8(0x6000_8101, 1).unwrap();
    assert_eq!(bus.read32(CONF).unwrap(), FINISH | START | 0x23);
    assert_eq!(bus.read32(RESULT).unwrap(), 0x2144_df1c);
    bus.write32(CONF, 0x7ff0_0001).unwrap();
    bus.write32(CONF, 0x7ff0_0101).unwrap();
    assert_eq!(bus.read32(RESULT).unwrap(), 0xd8f4_9994, "all 8192 bytes");
    bus.rtc_fast[8188..].copy_from_slice(b"1234");
    bus.rtc_fast[..4].copy_from_slice(b"5678");
    bus.write32(CONF, (0x7ff << 9) | (1 << 20) | START).unwrap();
    // Out-of-end addressing wraps the 11-bit word address in this model.
    assert_eq!(bus.read32(RESULT).unwrap(), 0x9ae0_daaf);
}
