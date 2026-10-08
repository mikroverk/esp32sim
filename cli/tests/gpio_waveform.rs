#[path = "../../tests/gpio_waveform.rs"]
mod waveform;

#[test]
fn gpio_waveforms_have_exact_pulse_widths() {
    waveform::run();
}
