use esp_soc::SocBus;

#[test]
fn ledc_matrix_clock_reset_and_block_wiring() {
    let mut m = esp32c6::machine([0; 6], 4 << 20);
    let b = &mut m.bus;
    assert!(b.pwm_output(4).is_none());
    b.periph.write32(0x60096034, 1);
    b.periph.write32(0x60096038, (1 << 22) | (3 << 20));
    b.periph.write32(0x60091024, 1 << 4);
    b.periph.write32(0x600070d0, 3);
    b.periph.write32(0x600070a0, 8 | (256 << 5) | (1 << 26));
    b.periph.write32(0x60007008, 64 << 4);
    b.periph.write32(0x6000700c, 1 << 31);
    b.periph.write32(0x60007000, 4 | (1 << 4));
    b.periph.tick(10000);
    for (flags, duty) in [(0, 16384), (1 << 9, 16384), (1 << 8, 49151)] {
        b.periph.write32(0x60091564, flags);
        assert_eq!(b.pwm_output(4), Some((156250.0, duty)));
    }
    b.periph.write32(0x60096034, 0);
    assert!(b.pwm_output(4).is_none());
    b.periph.write32(0x60096034, 1);
    b.periph.write32(0x60096038, (1 << 22) | (3 << 20));
    assert_eq!(b.pwm_output(4), Some((156250.0, 49151)));
    for (sel, hz) in [(1, 80_000_000.0), (2, 17_500_000.0), (3, 40_000_000.0)] {
        b.periph.write32(0x60096038, (1 << 22) | (sel << 20));
        assert_eq!(b.pwm_output(4), Some((hz / 256.0, 49151)));
    }
    b.periph.write32(0x60096038, 3 << 20);
    assert!(b.pwm_output(4).is_none());
    b.periph.write32(0x60096038, (1 << 22) | (3 << 20));
    b.periph.write32(0x60096034, 3);
    assert!(b.pwm_output(4).is_none());
    b.periph.write32(0x60096034, 1);
    assert!(b.pwm_output(4).is_none());
}

#[test]
fn mcpwm_matrix_clock_reset_and_block_wiring() {
    for (base, signal, _bit) in [(0x60014000, 87, 0)] {
        let mut m = esp32c6::machine([0; 6], 4 << 20);
        let b = &mut m.bus;
        b.periph.write32(0x6009609c, 1);
        b.periph.write32(0x600960a0, (1 << 22) | (1 << 20));
        b.periph.write32(0x60091024, 1 << 4);
        for (off, value) in [(0, 15), (4, (19999 << 8) | 9), (8, 2 | (1 << 3)), (0x40, 1500), (0x50, 2 | (1 << 4))] {
            b.periph.write32(base + off, value);
        }
        for (flags, duty) in [(0, 4915), (1 << 9, 4915), (1 << 8, 60620)] {
            b.periph.write32(0x60091564, signal | flags);
            assert_eq!(b.pwm_output(4), Some((50.0, duty)));
        }
        for (sel, hz) in [(1, 160_000_000.0), (2, 40_000_000.0), (3, 17_500_000.0)] {
            b.periph.write32(0x600960a0, (1 << 22) | (sel << 20) | (3 << 12));
            assert_eq!(b.pwm_output(4), Some((hz / 4.0 / 3_200_000.0, 60620)));
        }
        b.periph.write32(0x600960a0, 1 << 20);
        assert!(b.pwm_output(4).is_none());
        b.periph.write32(0x600960a0, (1 << 22) | (1 << 20));
        b.periph.write32(0x6009609c, 0);
        assert!(b.pwm_output(4).is_none());
        b.periph.write32(0x6009609c, 1);
        assert_eq!(b.pwm_output(4), Some((50.0, 60620)));
        b.periph.write32(0x6009609c, 3);
        assert!(b.pwm_output(4).is_none());
        b.periph.write32(0x6009609c, 1);
        assert!(b.pwm_output(4).is_none());
    }
}
