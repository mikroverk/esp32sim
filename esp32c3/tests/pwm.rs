use esp_soc::SocBus;

#[test]
fn ledc_matrix_clock_reset_and_block_wiring() {
    let mut m = esp32c3::machine([0; 6], 4 << 20);
    let b = &mut m.bus;
    assert!(b.pwm_output(4).is_none());
    b.periph.write32(0x600c0018, 0);
    b.periph.write32(0x600c0010, 1 << 11);
    b.periph.write32(0x60004024, 1 << 4);
    b.periph.write32(0x600190d0, 3);
    b.periph.write32(0x600190a0, 8 | (256 << 4) | (1 << 25));
    b.periph.write32(0x60019008, 64 << 4);
    b.periph.write32(0x6001900c, 1 << 31);
    b.periph.write32(0x60019000, 4 | (1 << 4));
    b.periph.tick(10000);
    for (flags, duty) in [(0, 16384), (1 << 9, 16384), (1 << 8, 49151)] {
        b.periph.write32(0x60004564, 45 | flags);
        assert_eq!(b.pwm_output(4), Some((156250.0, duty)));
    }
    b.periph.write32(0x600c0010, 0);
    assert!(b.pwm_output(4).is_none());
    b.periph.write32(0x600c0010, 1 << 11);
    assert_eq!(b.pwm_output(4), Some((156250.0, 49151)));
    b.periph.write32(0x600c0018, 1 << 11);
    assert!(b.pwm_output(4).is_none());
    b.periph.write32(0x600c0018, 0);
    assert!(b.pwm_output(4).is_none());
}
