use esp_periph::{gpio::Gpio, i2s::I2s, rmt::{Rmt, RMT_MEM_WORDS}};

#[test]
fn gpio_rejects_every_nonexistent_input_without_mutating_state() {
    let mut gpio = Gpio::new();
    let initial = gpio.input;
    for pin in 49..=u8::MAX {
        assert!(!gpio.set_input(pin, false));
        assert!(!gpio.set_input(pin, true));
        assert!(!gpio.level(pin));
    }
    assert_eq!(gpio.input, initial);
    assert_eq!(gpio.status, 0);
    assert!(gpio.input_changes.is_empty());
    assert!(gpio.set_input(48, false));
    assert!(!gpio.level(48));
    assert_eq!(gpio.input_changes, [(48, false)]);
}

const PULSE: u32 = (10 << 16) | 0x8000 | 20;

#[test]
fn rmt_nonwrapping_memory_exhaustion_stops_and_reports_empty_error() {
    let mut rmt = Rmt::new(240_000_000);
    rmt.mem[..RMT_MEM_WORDS].fill(PULSE);
    rmt.write(0x20, 1 | (1 << 8) | (1 << 16));
    rmt.tick(100_000);
    assert!(!rmt.ch[0].running);
    assert_eq!(rmt.ch[0].rd, RMT_MEM_WORDS);
    assert_eq!(rmt.ch[0].bits.len(), RMT_MEM_WORDS);
    assert_ne!(rmt.read(0x50) & (1 << 25), 0);
    assert_eq!(rmt.int_raw, 1 << 4);
    assert_eq!(rmt.tx_count, 0, "memory exhaustion is not a zero-period end marker");
    rmt.write(0x20, 1 << 1);
    assert_eq!(rmt.read(0x50) & (1 << 25), 0);
}

#[test]
fn rmt_wrap_repeats_until_an_end_marker_in_either_half() {
    let mut rmt = Rmt::new(240_000_000);
    rmt.mem[..RMT_MEM_WORDS].fill(PULSE);
    rmt.write(0x20, 1 | (1 << 4) | (1 << 8) | (1 << 16));
    rmt.tick(2 * RMT_MEM_WORDS as u64 * 90);
    assert!(rmt.ch[0].running);
    assert_eq!(rmt.ch[0].rd, 2 * RMT_MEM_WORDS);
    assert_eq!(rmt.int_raw, 0);
    rmt.mem[0] = 0x8000;
    rmt.tick(1);
    assert!(!rmt.ch[0].running);
    assert_eq!(rmt.done[0].1.len(), 2 * RMT_MEM_WORDS);
    assert_eq!(rmt.int_raw, 1);

    rmt.write(0x7c, u32::MAX);
    rmt.mem[0] = 0x8000_0000 | 0x8000 | 20; // second half: zero period, high level
    rmt.write(0x20, 1 | (1 << 8) | (1 << 16));
    rmt.tick(100_000);
    assert!(!rmt.ch[0].running);
    assert_eq!(rmt.ch[0].rd, 1);
    assert_eq!(rmt.tx_count, 2);
    assert_eq!(rmt.int_raw, 1);
}

#[test]
fn rmt_invalid_memory_block_counts_cannot_read_past_the_ram() {
    for channel in 0..4 {
        for blocks in (9 - channel)..=15 {
            let mut rmt = Rmt::new(240_000_000);
            rmt.mem.fill(PULSE);
            rmt.write(0x20 + channel as u32 * 4, 1 | (1 << 8) | ((blocks as u32) << 16));
            rmt.tick(100_000);
            assert!(!rmt.ch[channel].running);
            assert_eq!(rmt.int_raw, 1 << (4 + channel));
        }
    }
}

#[test]
fn rmt_continuous_mode_repeats_full_memory_without_wrap_mode() {
    let mut rmt = Rmt::new(240_000_000);
    rmt.mem[..RMT_MEM_WORDS].fill(PULSE);
    rmt.write(0x20, 1 | (1 << 3) | (1 << 8) | (1 << 16));
    rmt.tick(2 * RMT_MEM_WORDS as u64 * 90);
    assert!(rmt.ch[0].running);
    assert_eq!(rmt.ch[0].rd, 2 * RMT_MEM_WORDS);
    assert_eq!(rmt.int_raw, 0);
}

#[test]
fn rmt_second_half_end_marker_waits_for_the_first_half_duration() {
    let mut rmt = Rmt::new(240_000_000);
    rmt.mem[0] = 0x8000 | 32767;
    rmt.write(0x20, 1 | (1 << 8) | (1 << 16));
    rmt.tick(1);
    assert!(rmt.ch[0].running);
    assert_eq!(rmt.int_raw, 0);
    rmt.tick(3 * 32767 - 2);
    assert!(rmt.ch[0].running);
    assert_eq!(rmt.int_raw, 0);
    rmt.tick(1);
    assert!(!rmt.ch[0].running);
    assert_eq!(rmt.int_raw, 1);
}

#[test]
fn rmt_continuous_mode_restarts_on_either_half_end_marker() {
    for second_half_marker in [false, true] {
        let mut rmt = Rmt::new(240_000_000);
        rmt.mem[0] = if second_half_marker { 0x8000 | 20 } else { PULSE };
        rmt.write(0x20, 1 | (1 << 3) | (1 << 8) | (1 << 16));
        rmt.tick(600);
        assert!(rmt.ch[0].running);
        assert!(rmt.ch[0].bits.len() <= 1);
        assert_eq!(rmt.int_raw, 0);
        assert!(rmt.done.is_empty());
    }
}

fn configure_i2s(i2s: &mut I2s, data_bits: u32, slot_bits: u32, slots: u32, active: u32) {
    i2s.write(0x2c, (3 << 7) | ((data_bits - 1) << 13)
        | ((slot_bits * slots / 2 - 1) << 18) | ((slot_bits - 1) << 24));
    i2s.write(0x54, ((slots - 1) << 16) | active);
    i2s.write(0x34, (1 << 26) | (2 << 27) | 8);
}

#[test]
fn i2s_short_ws_pulse_has_no_effect_on_frame_duration() {
    let mut i2s = I2s::new(240_000_000);
    configure_i2s(&mut i2s, 16, 32, 2, 3);
    assert_eq!(i2s.sample_rate, 78_125);
    assert_eq!(i2s.bytes_per_frame, 4, "wire padding is not DMA data");
    let conf = i2s.tx_conf1;
    for ws_width in [0, 15, 31, 63, 127] {
        i2s.write(0x2c, conf | ws_width);
        assert_eq!(i2s.sample_rate, 78_125);
    }
    i2s.write(0x24, 1 << 2);
    assert_eq!(i2s.frames_due(240_000_000), 78_125);
}

#[test]
fn i2s_clock_selector_distinguishes_the_two_pll_sources() {
    let mut i2s = I2s::new(240_000_000);
    configure_i2s(&mut i2s, 16, 32, 2, 3);
    assert_eq!(i2s.sample_rate, 78_125);
    i2s.write(0x34, (1 << 26) | (1 << 27) | 8);
    assert_eq!(i2s.sample_rate, 117_188);
}

#[test]
fn i2s_dma_sample_width_and_active_slots_are_separate_from_wire_slots() {
    let mut i2s = I2s::new(240_000_000);
    for bits in [8, 16, 24, 32] {
        configure_i2s(&mut i2s, bits, 32, 4, 0b0101);
        assert_eq!(i2s.sample_bytes(), (bits / 8) as usize);
        assert_eq!(i2s.bytes_per_frame, 2 * (bits / 8));
        assert_eq!(i2s.sample_rate, 39_063);
        i2s.write(0x54, (3 << 16) | 0b0101 | (1 << 20));
        assert_eq!(i2s.bytes_per_frame, 4 * (bits / 8));
        i2s.write(0x24, 1 << 5);
        assert_eq!(i2s.bytes_per_frame, bits / 8);
        i2s.write(0x24, 0);
    }
    i2s.write(0x54, 3 << 16);
    assert_eq!(i2s.bytes_per_frame, 0, "disabled slots do not consume DMA data");
}

#[test]
fn i2s_supports_eight_and_sixteen_slots_within_the_128_bit_frame_limit() {
    let mut i2s = I2s::new(240_000_000);
    // ESP-IDF I2S_LL_SLOT_FRAME_BIT_MAX is 128 on S3; 16 x 32 is not a valid frame.
    for (slots, bits) in [(8, 16), (16, 8)] {
        configure_i2s(&mut i2s, bits, bits, slots, (1 << slots) - 1);
        assert_eq!(i2s.sample_rate, 39_063);
        assert_eq!(i2s.bytes_per_frame, 16);
    }
}

#[test]
fn rmt_counted_loops_raise_loop_interrupt_and_stop_when_enabled() {
    let mut rmt = Rmt::new(240_000_000);
    rmt.mem[0] = PULSE;
    rmt.write(0xa0, (3 << 9) | (1 << 19) | (1 << 21));
    rmt.write(0x20, 1 | (1 << 3));
    rmt.tick(10000);
    assert!(!rmt.ch[0].running);
    assert_eq!(rmt.int_raw, 1 << 12);
    assert!(rmt.ch[0].bits.is_empty());
}

#[test]
fn i2s_reset_slots_are_enabled() {
    let mut i2s = I2s::new(240_000_000);
    assert_eq!(i2s.read(0x54), 0xffff);
    assert_eq!(i2s.bytes_per_frame, 1);
    i2s.write(0x24, 1 << 2);
    assert_eq!(i2s.bytes_per_frame, 1);
}

#[test]
fn rmt_empty_continuous_frame_yields_without_accumulating_cycles() {
    let mut rmt = Rmt::new(240_000_000);
    rmt.write(0x20, 1 | (1 << 3));
    for _ in 0..3 {
        rmt.tick(100);
        assert!(rmt.ch[0].running);
        assert_eq!(rmt.ch[0].acc_cycles, 0);
        assert!(rmt.ch[0].bits.is_empty());
    }
}
