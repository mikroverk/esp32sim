use esp_soc::SocBus;

#[test]
fn ledc_matrix_clock_reset_and_block_wiring() {
    let mut m = esp32s3::machine([0; 6]);
    let b = &mut m.bus;
    assert!(b.pwm_output(4).is_none());
    b.periph.write32(0x600c0018, 1 << 11);
    b.periph.write32(0x60004024, 1 << 4);
    b.periph.write32(0x600190d0, 3);
    b.periph.write32(0x600190a0, 8 | (256 << 4) | (1 << 25));
    b.periph.write32(0x60019008, 64 << 4);
    b.periph.write32(0x6001900c, 1 << 31);
    b.periph.write32(0x60019000, 4 | (1 << 4));
    b.periph.tick(10000);
    for (flags, duty) in [(0, 16384), (1 << 10, 16384), (1 << 9, 49151)] {
        b.periph.write32(0x60004564, 73 | flags);
        assert_eq!(b.pwm_output(4), Some((156250.0, duty)));
    }
    b.periph.write32(0x600c0018, 0);
    assert!(b.pwm_output(4).is_none());
    b.periph.write32(0x600c0018, 1 << 11);
    assert_eq!(b.pwm_output(4), Some((156250.0, 49151)));
    b.periph.write32(0x600c0020, 1 << 11);
    assert!(b.pwm_output(4).is_none());
    b.periph.write32(0x600c0020, 0);
    assert!(b.pwm_output(4).is_none());
}

#[test]
fn mcpwm_matrix_clock_reset_and_block_wiring() {
    for (base, signal, bit) in [(0x6001e000, 160, 17), (0x6002c000, 166, 20)] {
        let mut m = esp32s3::machine([0; 6]);
        let b = &mut m.bus;
        b.periph.write32(0x600c0018, 1 << bit);
        b.periph.write32(0x60004024, 1 << 4);
        for (off, value) in [(0, 15), (4, (19999 << 8) | 9), (8, 2 | (1 << 3)), (0x40, 1500), (0x50, 2 | (1 << 4))] {
            b.periph.write32(base + off, value);
        }
        for (flags, duty) in [(0, 4915), (1 << 10, 4915), (1 << 9, 60620)] {
            b.periph.write32(0x60004564, signal | flags);
            assert_eq!(b.pwm_output(4), Some((50.0, duty)));
        }
        b.periph.write32(0x600c0018, 0);
        assert!(b.pwm_output(4).is_none());
        b.periph.write32(0x600c0018, 1 << bit);
        assert_eq!(b.pwm_output(4), Some((50.0, 60620)));
        b.periph.write32(0x600c0020, 1 << bit);
        assert!(b.pwm_output(4).is_none());
        b.periph.write32(0x600c0020, 0);
        assert!(b.pwm_output(4).is_none());
    }
}
