#[test]
fn released_pad_resolves_output_pulls_and_interrupts() {
    let mut gpio = esp_periph::Gpio::new();
    for pin in [0, 7, 32, 48] {
        let bit = 1u64 << pin;
        gpio.write(0x74 + 4 * pin as u32, (3 << 7) | (1 << 13));
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

#[test]
fn output_edges_only_queue_for_selected_matrix_inputs() {
    for select in [0x40, 0x80] {
        let mut gpio = esp_periph::Gpio::new();
        gpio.input_select = select;
        for pin in [4, 40] {
            if select == 0x40 && pin == 40 { continue; }
            let (out, enable) = if pin < 32 { (4, 0x24) } else { (0x10, 0x30) };
            let bit = 1 << (pin % 32);
            gpio.write(enable, bit);
            gpio.write(out, bit);
            assert!(gpio.input_changes.is_empty());
            gpio.write(0x154, select | pin);
            gpio.write(0x158, select | pin); // A second route must survive removal of the first.
            gpio.write(0x154, 0x3c);
            gpio.write(out, 0);
            assert_eq!(gpio.input_changes, [(pin as u8, false)]);
            gpio.input_changes.clear();
            gpio.write(0x158, pin | (select / 2)); // Inversion without matrix selection.
            gpio.write(out, bit);
            assert!(gpio.input_changes.is_empty());
            gpio.write(0x158, select | pin | (select / 2));
            gpio.write(out, 0);
            assert_eq!(gpio.input_changes, [(pin as u8, false)]);
            gpio.input_changes.clear();
            gpio.write(0x158, select | (select / 2 - 1)); // Constant/invalid pin.
            gpio.write(out, bit);
            assert!(gpio.input_changes.is_empty());
        }
    }
}

#[test]
fn input_change_result_does_not_depend_on_irq_configuration() {
    let mut gpio = esp_periph::Gpio::new();
    assert!(gpio.set_input(4, false));
    assert!(!gpio.set_input(4, false));
    assert!(gpio.release_input(4));
    assert!(!gpio.release_input(4));
    assert_eq!(gpio.status, 0);
}
