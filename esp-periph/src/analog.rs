//! Analog pad voltages the SAR ADC samples, set from the host: a constant (`adc <gpio> <volts>`)
//! or a waveform played against emulated time (`adcwave <gpio> <file> <rate_hz>`), so a recorded
//! or synthetic signal (EMG, a sensor trace) reaches the firmware's analogRead() sample by sample.
//! The ADC model that reads these is the chip's business (the S3's SENS one-shot path in
//! `rtc_cntl.rs`). Raw inputs bypass the voltage transfer curve; observations track completed samples.
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

/// Last completed conversion. Generation starts at zero and wraps at u64::MAX.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AdcObservation { pub generation: u64, pub raw: u16 }

#[derive(Default)]
pub struct AnalogInputs { pins: HashMap<u8, AnalogSource>, raw: HashMap<u8, u16>, observed: HashMap<u8, AdcObservation>, pub cpu_hz: u64 }
impl AnalogInputs {
    pub fn new(cpu_hz: u64) -> Self { AnalogInputs { cpu_hz, ..Default::default() } }
    pub fn set(&mut self, pin: u8, src: AnalogSource) { self.raw.remove(&pin); self.pins.insert(pin, src); }
    /// Replace the voltage source with a post-attenuation 12-bit count. Invalid counts leave it intact.
    pub fn set_raw(&mut self, pin: u8, raw: u16) -> bool {
        if raw > 4095 { return false; }
        self.pins.remove(&pin); self.raw.insert(pin, raw); true
    }
    pub fn observation(&self, pin: u8) -> AdcObservation { self.observed.get(&pin).copied().unwrap_or_default() }
    /// Complete one sample. Reading or replacing a source never advances its generation.
    pub fn convert(&mut self, pin: u8, now: u64, code: impl FnOnce(f32) -> u32) -> u32 {
        let raw = self.raw.get(&pin).copied().unwrap_or_else(|| code(self.volts(pin, now)) as u16);
        let sample = self.observed.entry(pin).or_default();
        sample.generation = sample.generation.wrapping_add(1); sample.raw = raw;
        u32::from(raw)
    }
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
