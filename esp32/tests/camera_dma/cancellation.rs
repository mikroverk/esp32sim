use super::*;

// MMIO loss and restoration deliberately have no intervening DMA tick.
// The first five reproduce physical route loss; SCCB streaming and clock
// gates cover the other configuration paths. Register references are inherited
// from aithinker.rs and arming.rs (ESP-IDF v4.4.7/v5.5.4).
fn loss_restore(b: &mut SocBus, cause: usize) {
    let reg = match cause {
        0 => Some((0x3ff4_4130 + 4 * 140, 0x80 | 18)),
        1 => Some((0x3ff4_9004, 2 << 12)),
        2 => Some((0x3ff4_847c, 1 << 27)),
        3 => Some((0x3ff4_4010, 1)),
        4 => Some((0x3ff4_4530, 256)),
        6 => Some((0x3ff0_00c0, 1 << 11)), // I2S gated, LEDC retained
        7 => Some((0x3ff0_00c0, 1 << 4)), // LEDC gated, I2S retained
        _ => None,
    };
    if let Some((addr, bad)) = reg {
        let old = b.read32(addr).unwrap();
        put(b, addr, bad);
        assert_ne!(raw(b) & ERR, 0, "loss is synchronous: cause {cause}");
        put(b, addr, old);
    } else {
        write(b, 0xe0, 4);
        assert_ne!(raw(b) & ERR, 0, "streaming stop is synchronous");
        write(b, 0xe0, 0);
    }
}

fn cancellation(cause: usize, partial: bool) {
    for mode in [1, 3] {
        let mut b = setup(mode);
        let size = if mode == 1 { 64 } else { 128 };
        descriptor(&mut b, 0, size, 0); arm(&mut b, size / 4);
        if partial {
            begin(&mut b);
            tick_bus(&mut b, (FRAME_CYCLES - VSYNC_CYCLES) / 4);
            assert_ne!(bytes(&mut b, B, size), vec![0; size as usize]);
        } else {
            // Observe a valid route before a physical pulse, but no packed data.
            tick_bus(&mut b, 1);
            assert_eq!(bytes(&mut b, B, size), vec![0; size as usize]);
        }
        assert_eq!(raw(&mut b) & EOF, 0);
        let before = bytes(&mut b, B, size);
        let desc = bytes(&mut b, D, 12);
        let eof_desc = b.read32(I + 0x3c).unwrap();
        loss_restore(&mut b, cause);
        // Clearing the interrupt and benign RX configuration cannot rearm DMA.
        put(&mut b, I + 0x18, u32::MAX);
        let conf = b.read32(I + 8).unwrap();
        put(&mut b, I + 8, conf);
        for _ in 0..2 {
            begin(&mut b); tick_bus(&mut b, FRAME_CYCLES - VSYNC_CYCLES);
            assert_eq!(raw(&mut b), 0, "no continued IRQ cause {cause}");
            assert_eq!(bytes(&mut b, B, size), before, "no continued bytes cause {cause}");
            assert_eq!(bytes(&mut b, D, 12), desc, "no descriptor writeback cause {cause}");
            assert_eq!(b.read32(I + 0x3c).unwrap(), eof_desc);
        }
        descriptor(&mut b, 0, size, 0); arm(&mut b, size / 4);
        begin(&mut b); tick_bus(&mut b, FRAME_CYCLES - VSYNC_CYCLES);
        assert_eq!(raw(&mut b), DONE | EOF, "fresh rearm recovers cause {cause}");
        assert_eq!(b.read32(D).unwrap(), size | (size << 12) | (1 << 30));
        let pattern = if mode == 1 { vec![0, 0, 248, 0] } else { vec![0, 0, 248, 0, 0, 0, 0, 0] };
        assert_eq!(bytes(&mut b, B, size), pattern.repeat(16));
    }
}

#[test]
fn cancellation_matrix() { cancellation(0, true); }
#[test]
fn cancellation_iomux() { cancellation(1, true); }
#[test]
fn cancellation_rtc() { cancellation(2, true); }
#[test]
fn cancellation_power() { cancellation(3, true); }
#[test]
fn cancellation_xclk() { cancellation(4, true); }
#[test]
fn cancellation_streaming() { cancellation(5, true); }
#[test]
fn cancellation_i2s_gate() { cancellation(6, true); }
#[test]
fn cancellation_ledc_gate() { cancellation(7, true); }
#[test]
fn cancellation_qualified_waiting() {
    for cause in 0..8 { cancellation(cause, false); }
}
#[test]
fn cancellation_valid_changes_preserve_active_capture() {
    let mut b = setup(1);
    descriptor(&mut b, 0, 64, 0); arm(&mut b, 16); begin(&mut b);
    tick_bus(&mut b, (FRAME_CYCLES - VSYNC_CYCLES) / 4);
    // Unrelated pad, unrelated RTC bit, pullup, and valid inverted PWM.
    put(&mut b, 0x3ff4_9060, 2 << 12);
    let pad = b.read32(0x3ff4_9004).unwrap();
    put(&mut b, 0x3ff4_9004, pad ^ (1 << 8));
    let rtc = b.read32(0x3ff4_847c).unwrap();
    put(&mut b, 0x3ff4_847c, rtc ^ (1 << 26));
    put(&mut b, 0x3ff4_4530, 71 | (1 << 9));
    assert_eq!(raw(&mut b), 0);
    tick_bus(&mut b, (FRAME_CYCLES - VSYNC_CYCLES) * 3 / 4);
    assert_eq!(raw(&mut b), DONE | EOF);
    assert_eq!(bytes(&mut b, B, 64), [0, 0, 248, 0].repeat(16));
}
