use emu_core::Bus;

#[test]
fn ecc_mmio_routes_cached_interrupts_without_clock_work() {
    let mut bus = esp32c6::bus::SocBus::new(4 << 20, [0; 6]);
    const ECC: u32 = 0x6008_b000;
    let asserted = |bus: &esp32c6::bus::SocBus| bus.periph.source_status()[2] & (1 << (76 - 64));
    assert_eq!(bus.read32(ECC + 0x1c).unwrap(), 1 << 31);
    bus.write32(ECC + 0x1c, 0x45).unwrap(); // Invalid zero point, but verification completes.
    assert_eq!(bus.read32(ECC + 0xc).unwrap(), 1);
    assert_eq!(bus.read32(ECC + 0x10).unwrap(), 0);
    assert_eq!(asserted(&bus), 0);
    bus.write32(ECC + 0x14, 1).unwrap();
    assert_ne!(asserted(&bus), 0);
    assert_eq!(bus.read32(ECC + 0x10).unwrap(), 1);
    assert!(bus.periph.misc.active_optional.is_empty());
    bus.write32(ECC + 0x18, 0).unwrap();
    assert_ne!(asserted(&bus), 0);
    bus.write32(ECC + 0xc, 1).unwrap();
    assert_eq!(asserted(&bus), 0);
    bus.write32(ECC + 0x1c, 0x45).unwrap();
    bus.write32(ECC + 0x14, 0).unwrap();
    assert_eq!(asserted(&bus), 0);
    bus.write32(ECC + 0x14, 1).unwrap();
    assert_ne!(asserted(&bus), 0);
    bus.write32(ECC + 0x1c, 2).unwrap();
    assert_eq!(asserted(&bus), 0);
    assert_eq!(bus.read32(ECC + 0xc).unwrap(), 0);
}
