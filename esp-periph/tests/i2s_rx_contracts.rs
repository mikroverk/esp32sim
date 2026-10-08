use esp_periph::I2s;

fn receiver() -> I2s {
    let mut i = I2s::new(160_000_000);
    i.write(0x30, (1 << 26) | (2 << 27) | 25);
    i.write(0x28, (24 << 7) | (15 << 13) | (15 << 18));
    i.write(0x50, (1 << 16) | 3);
    i.write(0x20, 4);
    i
}

#[test]
fn mono_consumes_one_sample_with_both_slots_enabled() {
    let mut i = receiver();
    i.rx_input.push(&[[12, 34], [56, 78]]);
    i.write(0x20, 4 | (1 << 5));
    assert_eq!(i.rx_data(20000, false), [12, 0]);
    i.write(0x20, 4);
    assert_eq!(i.rx_data(20000, false), [56, 0, 78, 0]);
}

#[test]
fn pdm_double_decimation_halves_pcm_rate() {
    let mut i = receiver();
    i.write(0x20, 4 | (1 << 20) | (1 << 21) | (1 << 22));
    assert_eq!(i.rx_rate(), Some(2000));
    i.rx_input.push(&[[12, 34]]);
    assert!(i.rx_data(79999, true).is_empty());
    assert_eq!(i.rx_data(1, true), [12, 0, 34, 0]);
}

#[test]
fn tone_replaces_queue_and_push_replaces_tone() {
    let mut i = receiver();
    i.rx_input.push(&[[1, 2]]);
    i.rx_input.tone(1000.0, 0.5).unwrap();
    i.rx_input.push(&[[12, 34]]);
    assert_eq!(i.rx_data(20000, false), [12, 0, 34, 0]);
    assert_eq!(i.rx_data(20000, false), [0; 4]);
}
