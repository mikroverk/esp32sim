#[test]
fn released_pad_resolves_output_pulls_and_interrupts() {
    let mut gpio = esp_periph::Gpio::new();
    for pin in [0, 7, 32, 48] {
        let bit = 1u64 << pin;
        gpio.pin[pin] = (3 << 7) | (1 << 13);
        gpio.set_input(pin as u8, false);
        gpio.set_pulls(pin as u8, true, false);
        assert_eq!(gpio.input & bit, 0);
        assert!(gpio.release_input(pin as u8));
        assert_ne!(gpio.input & bit, 0);
        assert_ne!(gpio.status & bit, 0);
        let (out, ena, shift) = if pin < 32 {
            (4, 0x24, pin)
        } else {
            (0x10, 0x30, pin - 32)
        };
        gpio.write(ena, 1 << shift);
        assert_eq!(gpio.input & bit, 0);
        gpio.write(out, 1 << shift);
        assert_ne!(gpio.input & bit, 0);
        gpio.set_input(pin as u8, false);
        assert!(!gpio.level(pin as u8));
        gpio.release_input(pin as u8);
        assert!(gpio.level(pin as u8));
        gpio.set_pulls(pin as u8, false, true);
        gpio.write(ena + 4, 1 << shift);
        assert!(!gpio.level(pin as u8));
        gpio.set_pulls(pin as u8, false, false);
        assert!(gpio.level(pin as u8));
    }
    let before = gpio.input;
    assert!(!gpio.release_input(255));
    assert!(!gpio.set_input(255, false));
    gpio.set_pulls(255, false, true);
    assert_eq!(gpio.input, before);
}
