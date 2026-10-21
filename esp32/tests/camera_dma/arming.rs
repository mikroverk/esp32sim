use super::*;
use esp32::board::aithinker::ARM_WINDOW;
fn falling(b: &mut SocBus) -> u64 {
    let now = esp_soc::SocBus::cycles(b);
    let next = b.board.next_deadline().unwrap();
    tick_bus(b, next - now);
    tick_bus(b, VSYNC_CYCLES);
    assert!(!b.periph.gpio.gpio.level(25));
    esp_soc::SocBus::cycles(b)
}
fn stop_reset(b: &mut SocBus) {
    let conf = b.read32(I + 8).unwrap();
    put(b, I + 8, conf & !(1 << 5));
    let link = b.read32(I + 0x34).unwrap();
    put(b, I + 0x34, link | (1 << 28));
    put(b, I + 0x60, (1 << 12) | 0xd);
    put(b, I + 0x60, 1 << 12);
    put(b, I + 8, conf | (1 << 5));
}
#[test]
fn c71_falling_edge_full_frame_and_tick_splits() {
    for split in [1, 7, 257] {
        for mode in [1, 3] {
            let mut b = setup(mode);
            let at = falling(&mut b);
            tick_bus(&mut b, ARM_WINDOW / 4);
            let size = if mode == 1 {64} else {128};
            descriptor(&mut b, 0, size, 0); arm(&mut b, size / 4);
            let span = at + FRAME_CYCLES - VSYNC_CYCLES - esp_soc::SocBus::cycles(&b);
            for _ in 0..split { tick_bus(&mut b, span / split); }
            tick_bus(&mut b, span % split);
            assert_eq!(raw(&mut b), DONE | EOF);
            assert_eq!(b.read32(D).unwrap(), size | (size << 12) | (1 << 30));
            let pattern = if mode == 1 {vec![0,0,248,0]} else {vec![0,0,248,0,0,0,0,0]};
            assert_eq!(bytes(&mut b, B, size), pattern.repeat(16));
        }
    }
}
#[test]
fn c71_inclusive_window_boundary_and_exact_catchup() {
    for delay in [0, ARM_WINDOW, ARM_WINDOW + 1] {
        let mut b = setup(1);
        // Large frame gives multiple words due inside the back porch.
        write(&mut b, 0x5a, 40); write(&mut b, 0x5b, 30);
        let at = falling(&mut b);
        tick_bus(&mut b, delay);
        for j in 0..20 {
            put(&mut b, D+j*12, (1 << 31) | 3840);
            put(&mut b, D+j*12+4, B+j*3840);
            put(&mut b, D+j*12+8, if j==19 {0} else {D+(j+1)*12});
        }
        arm(&mut b, 19200);
        tick_bus(&mut b, 1);
        let due = (((delay + 1) as u128 * 19200) / u128::from(FRAME_CYCLES - VSYNC_CYCLES)) as u32 * 4;
        if delay <= ARM_WINDOW {
            assert_eq!(bytes(&mut b, B, due), [0,0,248,0].repeat(due as usize / 4), "edge-relative catchup");
            assert_eq!(bytes(&mut b, B+due, 4), [0;4]);
        } else { assert_eq!(bytes(&mut b, B, 76800), vec![0;76800]); }
        let remaining = at + FRAME_CYCLES - VSYNC_CYCLES - esp_soc::SocBus::cycles(&b);
        tick_bus(&mut b, remaining);
        assert_eq!(raw(&mut b) & EOF, if delay <= ARM_WINDOW {EOF} else {0});
        if delay > ARM_WINDOW {
            begin(&mut b); tick_bus(&mut b, FRAME_CYCLES - VSYNC_CYCLES);
            assert_eq!(raw(&mut b) & EOF, EOF);
        }
    }
}
#[test]
fn c71_driver_stop_reset_start_inside_window() {
    let mut b = setup(1);
    // Old RX was started but has not observed the falling edge on a DMA tick:
    // the driver stops it at the event, resets, then starts the fresh inlink.
    let at = falling(&mut b);
    tick_bus(&mut b, 100);
    stop_reset(&mut b);
    descriptor(&mut b, 0, 64, 0); arm(&mut b, 16);
    let remaining = at + FRAME_CYCLES - VSYNC_CYCLES - esp_soc::SocBus::cycles(&b);
    tick_bus(&mut b, remaining);
    assert_eq!(raw(&mut b), DONE | EOF);
    assert_eq!(bytes(&mut b, B, 64), [0,0,248,0].repeat(16));
}
#[test]
fn c71_second_start_cannot_replay_claimed_frame() {
    let mut b = setup(1);
    write(&mut b, 0x5a, 40); write(&mut b, 0x5b, 30);
    let at = falling(&mut b);
    descriptor(&mut b, 0, 3840, 0); arm(&mut b, 960);
    tick_bus(&mut b, ARM_WINDOW / 2);
    assert_ne!(bytes(&mut b, B, 32), [0;32], "first start delivered words");
    stop_reset(&mut b);
    descriptor(&mut b, 0, 3840, 0);
    for n in 0..3840 { b.write8(B+n, 0).unwrap(); }
    arm(&mut b, 960);
    tick_bus(&mut b, ARM_WINDOW / 4);
    assert_eq!(bytes(&mut b, B, 3840), vec![0;3840], "no duplicate catchup");
    let remaining = at + FRAME_CYCLES - VSYNC_CYCLES - esp_soc::SocBus::cycles(&b);
    tick_bus(&mut b, remaining);
    assert_eq!(raw(&mut b), 0);
    begin(&mut b); tick_bus(&mut b, (FRAME_CYCLES - VSYNC_CYCLES) / 20);
    assert_eq!(raw(&mut b), DONE | EOF);
}
#[test]
fn c71_forced_low_power_clock_stream_stop_never_starts_frame() {
    for cause in [0, 1, 2] {
        let mut b = setup(1);
        let next = b.board.next_deadline().unwrap();
        let now = esp_soc::SocBus::cycles(&b);
        tick_bus(&mut b, next - now + VSYNC_CYCLES / 2);
        assert!(b.periph.gpio.gpio.level(25));
        match cause {
            0 => put(&mut b, 0x3ff4_4010, 1),
            1 => put(&mut b, 0x3ff4_4530, 256),
            _ => write(&mut b, 0xe0, 4),
        }
        assert!(!b.periph.gpio.gpio.level(25));
        assert_eq!(b.board.camera_frame_start(false), None, "forced low has no scheduled edge");
        match cause {
            0 => put(&mut b, 0x3ff4_4010, 0),
            1 => { clock(&mut b); receiver(&mut b); }
            _ => write(&mut b, 0xe0, 0),
        }
        descriptor(&mut b, 0, 64, 0); arm(&mut b, 16);
        tick_bus(&mut b, ARM_WINDOW);
        assert_eq!(bytes(&mut b, B, 64), vec![0;64]);
        assert_eq!(raw(&mut b), 0);
        begin(&mut b); tick_bus(&mut b, FRAME_CYCLES - VSYNC_CYCLES);
        assert_eq!(raw(&mut b), DONE | EOF);
    }
}
#[test]
fn c71_route_loss_and_restore_inside_window_requires_new_pulse() {
    // Matrix source, IO_MUX input enable, RTC ownership, sensor power, XCLK.
    // RTC_IO SENSOR_PADS GPIO36 mux bit 27: IDF v4.4.7 rtc_io_reg.h.
    for (addr, bad) in [(0x3ff4_4130 + 4*140, 0x80 | 18),
        (0x3ff4_9004, 2 << 12), (0x3ff4_847c, 1 << 27),
        (0x3ff4_4010, 1), (0x3ff4_4530, 256)] {
        let mut b = setup(1);
        falling(&mut b);
        let old = b.read32(addr).unwrap();
        put(&mut b, addr, bad);
        put(&mut b, addr, old);
        descriptor(&mut b, 0, 64, 0); arm(&mut b, 16);
        tick_bus(&mut b, FRAME_CYCLES - VSYNC_CYCLES);
        assert_eq!(raw(&mut b), 0, "no eligible edge after loss at {addr:x}");
        assert_eq!(bytes(&mut b, B, 64), vec![0;64]);
        begin(&mut b); tick_bus(&mut b, FRAME_CYCLES - VSYNC_CYCLES);
        assert_eq!(raw(&mut b), DONE | EOF);
    }
}

#[test]
fn c71_unrelated_pad_and_noroute_bit_writes_keep_window() {
    let mut b = setup(1);
    falling(&mut b);
    // GPIO4 is not a DVP input; GPIO36 pull-up is not route qualification.
    put(&mut b, 0x3ff4_9060, 2 << 12);
    put(&mut b, 0x3ff4_8494, 1 << 19);
    let pad = b.read32(0x3ff4_9004).unwrap();
    put(&mut b, 0x3ff4_9004, pad ^ (1 << 8));
    let rtc = b.read32(0x3ff4_847c).unwrap();
    put(&mut b, 0x3ff4_847c, rtc ^ (1 << 26)); // GPIO37, not wired
    descriptor(&mut b, 0, 64, 0); arm(&mut b, 16);
    tick_bus(&mut b, FRAME_CYCLES - VSYNC_CYCLES);
    assert_eq!(raw(&mut b), DONE | EOF);
    assert_eq!(bytes(&mut b, B, 64), [0,0,248,0].repeat(16));
}

#[test]
fn c71_initially_invalid_route_waits_instead_of_aborting() {
    let mut b = setup(1);
    falling(&mut b);
    put(&mut b, 0x3ff4_4130 + 4*140, 0x80 | 18);
    descriptor(&mut b, 0, 64, 0); arm(&mut b, 16);
    tick_bus(&mut b, 1);
    assert_eq!(raw(&mut b), 0, "no valid source has been observed yet");
    put(&mut b, 0x3ff4_4130 + 4*140, 0x80 | 5);
    tick_bus(&mut b, FRAME_CYCLES - VSYNC_CYCLES - 1);
    assert_eq!(raw(&mut b), 0);
    assert_eq!(bytes(&mut b, B, 64), vec![0;64]);
    begin(&mut b); tick_bus(&mut b, FRAME_CYCLES - VSYNC_CYCLES);
    assert_eq!(raw(&mut b), DONE | EOF);
    assert_eq!(bytes(&mut b, B, 64), [0,0,248,0].repeat(16));
}
