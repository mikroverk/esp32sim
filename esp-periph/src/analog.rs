//! Analog pad voltages the SAR ADC samples, set from the host: a constant (`adc <gpio> <volts>`)
//! or a waveform played against emulated time (`adcwave <gpio> <file> <rate_hz>`), so a recorded
//! or synthetic signal (EMG, a sensor trace) reaches the firmware's analogRead() sample by sample.
//! The ADC model that reads these is the chip's business (the S3's SENS one-shot path in
//! `rtc_cntl.rs`); this only answers "what voltage is on pin N at cycle C".
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub enum AnalogSource {
    Const(f32),
    /// `samples` volts at `rate_hz`, starting at `start_cycles`; before the start it reads the first
    /// sample, after the end it holds the last one.
    Wave { samples: Arc<Vec<f32>>, rate_hz: f64, start_cycles: u64 },
}
impl std::fmt::Debug for AnalogSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnalogSource::Const(v) => write!(f, "Const({v} V)"),
            AnalogSource::Wave { samples, rate_hz, .. } => write!(f, "Wave({} samples @ {rate_hz} Hz)", samples.len()),
        }
    }
}

#[derive(Default)]
pub struct AnalogInputs { pins: HashMap<u8, AnalogSource>, pub cpu_hz: u64 }
impl AnalogInputs {
    pub fn new(cpu_hz: u64) -> Self { AnalogInputs { pins: HashMap::new(), cpu_hz } }
    pub fn set(&mut self, pin: u8, src: AnalogSource) { self.pins.insert(pin, src); }
    /// Volts on `pin` at `now_cycles` (0 V for a pin nobody drives).
    pub fn volts(&self, pin: u8, now_cycles: u64) -> f32 {
        match self.pins.get(&pin) {
            None => 0.0,
            Some(AnalogSource::Const(v)) => *v,
            Some(AnalogSource::Wave { samples, rate_hz, start_cycles }) => {
                if samples.is_empty() { return 0.0; }
                let dt = now_cycles.saturating_sub(*start_cycles) as f64 / self.cpu_hz.max(1) as f64;
                samples[((dt * rate_hz) as usize).min(samples.len() - 1)]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wave_follows_emulated_time_and_holds_the_ends() {
        let mut a = AnalogInputs::new(1_000);
        a.set(4, AnalogSource::Wave { samples: Arc::new(vec![0.1, 0.2, 0.3]), rate_hz: 10.0, start_cycles: 500 });
        assert_eq!(a.volts(4, 0), 0.1);        // before the start: first sample
        assert_eq!(a.volts(4, 600), 0.2);      // 0.1 s after the start at 10 Hz
        assert_eq!(a.volts(4, 99_000), 0.3);   // after the end: last sample
        assert_eq!(a.volts(5, 600), 0.0);      // nobody drives GPIO5
    }
}
