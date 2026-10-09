use emu_core::Bus;
use esp_periph::{analog::AnalogStream, AnalogSource};
use esp_soc::{Soc, SocBus};

fn conversions<S: Soc>(mut m: esp_soc::Machine<S>, pin: u8, control: u32, select: u32, start: u32) {
    let stream = AnalogStream::new(8000, 0.5, 0).unwrap();
    stream.push(&[0.75, 0.25, 0.5], 0, S::CPU_HZ).unwrap();
    m.bus.analog_set(pin, AnalogSource::Stream(stream.clone()));
    let mut readings = Vec::new();
    for sample in 0..4 {
        if sample == 2 {
            m.bus.reboot([0; 6]);
        }
        m.bus.tick((S::CPU_HZ / 8000) as u32);
        m.bus.write32(control, select).unwrap();
        m.bus.write32(control, select | start).unwrap();
        let observation = m.bus.adc_observation(pin).unwrap();
        assert_eq!(observation.generation, sample + 1);
        readings.push(observation.raw);
    }
    assert!(
        readings[0] > readings[2] && readings[2] > readings[1],
        "{readings:?}"
    );
    assert_eq!(readings[2], readings[3]);
    assert_eq!(stream.queued_samples(m.bus.cycles(), S::CPU_HZ), 0);
}

#[test]
fn stream_conversions_follow_time_across_reset() {
    conversions(esp32s3::machine([0; 6]), 1, 0x6000880c, 1 << 19, 1 << 17);
    conversions(
        esp32c3::machine([0; 6], 4 << 20),
        0,
        0x60040020,
        1 << 31,
        1 << 29,
    );
    conversions(
        esp32c6::machine([0; 6], 8 << 20),
        0,
        0x6000e020,
        1 << 31,
        1 << 29,
    );
}

fn raw_conversions<S: Soc>(
    mut m: esp_soc::Machine<S>,
    pin: u8,
    control: u32,
    select: u32,
    start: u32,
    attenuation: Option<u32>,
) {
    for atten in 0..4 {
        let now = m.bus.cycles();
        let stream = AnalogStream::new_raw(8000, 777, now).unwrap();
        stream.push_raw(&[0, 1, 1024, 2048, 4095], now, S::CPU_HZ).unwrap();
        m.bus.analog_set(pin, AnalogSource::RawStream(stream));
        let select = if let Some(register) = attenuation {
            m.bus.write32(register, atten).unwrap();
            select
        } else {
            select | (atten << 23)
        };
        for expected in [777, 0, 1, 1024, 2048, 4095, 4095] {
            m.bus.write32(control, select).unwrap();
            m.bus.write32(control, select | start).unwrap();
            assert_eq!(m.bus.adc_observation(pin).unwrap().raw, expected);
            m.bus.tick((S::CPU_HZ / 8000) as u32);
        }
    }
}

#[test]
fn raw_stream_bypasses_attenuation_on_three_chips() {
    raw_conversions(
        esp32s3::machine([0; 6]),
        1,
        0x6000880c,
        1 << 19,
        1 << 17,
        Some(0x60008814),
    );
    raw_conversions(
        esp32c3::machine([0; 6], 4 << 20),
        0,
        0x60040020,
        1 << 31,
        1 << 29,
        None,
    );
    raw_conversions(
        esp32c6::machine([0; 6], 8 << 20),
        0,
        0x6000e020,
        1 << 31,
        1 << 29,
        None,
    );
}
