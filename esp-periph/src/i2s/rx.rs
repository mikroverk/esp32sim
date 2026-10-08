// Register fields: ESP-IDF v5.5.5 components/soc/esp32s3/register/soc/i2s_reg.h
// lines 119-246 (RX_CONF), 375-416 (RX_CONF1), 677-692 (TDM), 1058-1065 (EOF).
// C3 has the same standard-RX fields, no PDM converter (esp32c3 i2s_reg.h:117-230).
// C6 keeps these fields (esp32c6 i2s_reg.h:149-290, 424-478, 930-945), but uses
// components/soc/esp32c6/register/soc/pcr_reg.h:719-784 for RX clocks.
// PCM packing/filter behavior is a model contract, not a hardware measurement.
use super::I2s;
use std::collections::VecDeque;

/// Host stereo PCM16 frames, consumed at the receiver's programmed sample rate.
/// Duplicate a mono sample into both lanes. Empty input produces silence.
/// The source survives chip resets; queued frames pause while RX/DMA is stopped.
#[derive(Default)]
pub struct PcmInput {
    frames: VecDeque<[i16; 2]>,
    tone: Option<(f64, f64, f64)>,
}
impl PcmInput {
    /// Append up to 65536 queued frames. Returns the number accepted; retry the rest later.
    /// Disables the tone generator without discarding queued frames.
    pub fn push(&mut self, frames: &[[i16; 2]]) -> usize {
        self.tone = None;
        let n = frames.len().min(65536 - self.frames.len());
        self.frames.extend(&frames[..n]);
        n
    }
    /// Replace queued audio with a continuous sine, amplitude in 0..=1 of full scale.
    /// Zero frequency or amplitude produces silence. Frequencies above Nyquist alias.
    pub fn tone(&mut self, hz: f64, amplitude: f64) -> Result<(), &'static str> {
        if !hz.is_finite()
            || !(0.0..=192000.0).contains(&hz)
            || !amplitude.is_finite()
            || !(0.0..=1.0).contains(&amplitude)
        {
            return Err("I2S tone needs frequency 0..192000 Hz and amplitude 0..1");
        }
        self.frames.clear();
        self.tone = Some((hz, amplitude, 0.0));
        Ok(())
    }
    pub fn clear(&mut self) {
        self.frames.clear();
        self.tone = None;
    }
    fn next(&mut self, rate: u32) -> [i16; 2] {
        if let Some((hz, amplitude, phase)) = &mut self.tone {
            let sample = (phase.sin() * *amplitude * 32767.0).round() as i16;
            *phase = (*phase + std::f64::consts::TAU * *hz / rate as f64) % std::f64::consts::TAU;
            [sample; 2]
        } else {
            self.frames.pop_front().unwrap_or([0; 2])
        }
    }
}

impl I2s {
    #[inline(always)]
    pub fn rx_running(&self) -> bool {
        self.rx_conf & 4 != 0
    }
    /// Standard I2S frame rate, or S3 PDM-to-PCM output rate. No external slave clock.
    pub fn rx_rate(&self) -> Option<u32> {
        let conf1 = self.ram.read(0x28);
        let frame_bits = if self.rx_conf & (1 << 20) != 0 {
            if self.rx_conf & (1 << 22) != 0 {
                128
            } else {
                64
            }
        } else {
            2 * (((conf1 >> 18) & 0x3f) as u64 + 1)
        };
        Self::clock_rate(self.ram.read(0x30), self.ram.read(0x38), conf1, frame_bits)
    }
    /// C6's RX clock lives in PCR instead of the I2S block.
    pub fn rx_pcr_clock(&mut self, conf: u32, div: u32) {
        self.ram.write(
            0x30,
            ((conf >> 12) & 0xff) | (((conf >> 20) & 3) << 27) | (((conf >> 22) & 1) << 26),
        );
        self.ram.write(0x38, div);
    }
    /// PCM at the DMA boundary, not a pin-level serializer or a PDM filter simulation.
    /// Supports standard 16/24/32-bit mono/stereo and S3 I2S0 converted PDM16.
    /// Raw PDM, TDM >2 slots, slave clocks, endian/bit-order/companding modes do not advance.
    pub fn rx_data(&mut self, cycles: u64, pdm2pcm: bool) -> Vec<u8> {
        let bits = ((self.ram.read(0x28) >> 13) & 31) + 1;
        let tdm = self.ram.read(0x50);
        let mask = tdm & 0xffff;
        let pdm = self.rx_conf & (1 << 20) != 0;
        let supported = self.rx_running()
            && [16, 24, 32].contains(&bits)
            && mask != 0
            && mask & !3 == 0
            && self.rx_conf & ((1 << 3) | (1 << 7) | (3 << 10) | (1 << 18)) == 0
            && if pdm {
                pdm2pcm && bits == 16 && self.rx_conf & (1 << 21) != 0
            } else {
                (tdm >> 16) & 15 == 1
            };
        let Some(rate) = self.rx_rate().filter(|_| supported) else {
            self.rx_acc = 0;
            return Vec::new();
        };
        self.rx_acc += cycles * u64::from(rate);
        let frames = self.rx_acc / self.cpu_hz;
        self.rx_acc %= self.cpu_hz;
        let mut bytes = std::mem::take(&mut self.rx_buffer);
        bytes.clear();
        for _ in 0..frames {
            let frame = self.rx_input.next(rate);
            for (lane, sample) in frame.into_iter().enumerate() {
                if mask & (1 << lane) == 0 {
                    continue;
                }
                if bits == 16 {
                    bytes.extend_from_slice(&sample.to_le_bytes());
                } else {
                    bytes.extend_from_slice(&(i32::from(sample) << 16).to_le_bytes());
                }
                if self.rx_conf & (1 << 5) != 0 {
                    break;
                }
            }
        }
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn receiver() -> I2s {
        let mut i = I2s::new(160_000_000);
        i.write(0x30, (1 << 26) | (2 << 27) | 25);
        i.write(0x28, (24 << 7) | (15 << 13) | (15 << 18) | (15 << 24));
        i.write(0x50, (1 << 16) | 3);
        i.write(0x20, 4);
        i
    }
    #[test]
    fn pcm_clock_slots_width_silence_and_reset() {
        let mut i = receiver();
        assert_eq!(i.rx_rate(), Some(8000));
        assert_eq!(i.rx_input.push(&[[1234, -2345], [111, 222]]), 2);
        assert!(i.rx_data(19999, true).is_empty());
        assert_eq!(i.rx_data(1, true), [0xd2, 4, 0xd7, 0xf6]);
        i.write(0x20, 1); // peripheral reset keeps host input
        assert!(i.rx_data(20000, true).is_empty());
        i.write(0x20, 4 | (1 << 5));
        i.write(0x50, (1 << 16) | 2);
        assert_eq!(i.rx_data(20000, true), 222i16.to_le_bytes());
        assert_eq!(i.rx_data(20000, true), [0; 2]);
        for bits in [24, 32] {
            i.write(
                0x28,
                (24 << 7) | ((bits - 1) << 13) | (15 << 18) | (31 << 24),
            );
            i.rx_input.push(&[[123, -123]]);
            assert_eq!(i.rx_data(20000, true), (-123i32 << 16).to_le_bytes());
        }
        i.write(0x20, 4 | (1 << 20) | (1 << 21));
        i.write(0x28, (24 << 7) | (15 << 13));
        assert_eq!(i.rx_rate(), Some(4000));
        assert!(i.rx_data(40000, false).is_empty());
        i.rx_input.push(&[[1, 2]]);
        assert_eq!(i.rx_data(40000, true), [2, 0]);
    }
    #[test]
    fn unsupported_modes_preserve_queued_input() {
        for flag in [1 << 3, 1 << 7, 1 << 10, 1 << 11, 1 << 18, 1 << 20] {
            let mut i = receiver();
            i.rx_input.push(&[[17, -19]]);
            i.write(0x20, 4 | flag);
            assert!(i.rx_data(40000, true).is_empty(), "flag {flag}");
            i.write(0x20, 4);
            assert_eq!(i.rx_data(20000, true), [17, 0, 237, 255]);
        }
        for (off, value) in [(0x28, (24 << 7) | (7 << 13) | (15 << 18)), (0x50, 0x10000), (0x50, 0x10004), (0x50, 0x20003), (0x30, 0)] {
            let mut i = receiver();
            i.write(off, value);
            assert!(i.rx_data(20000, true).is_empty());
        }
    }

    #[test]
    fn reset_discards_fractional_receiver_time() {
        let mut i = receiver();
        i.rx_input.push(&[[1, 2]]);
        assert!(i.rx_data(19999, true).is_empty());
        i.write(0x20, 1);
        i.write(0x20, 4);
        assert!(i.rx_data(1, true).is_empty());
        assert_eq!(i.rx_data(19999, true), [1, 0, 2, 0]);
    }

    #[test]
    fn bounded_input_and_tone() {
        let mut input = PcmInput::default();
        assert_eq!(input.push(&vec![[1; 2]; 65537]), 65536);
        assert_eq!(input.push(&[[2; 2]]), 0);
        assert!(input.tone(f64::NAN, 0.5).is_err());
        input.tone(1000.0, 0.5).unwrap();
        let samples: Vec<_> = (0..8).map(|_| input.next(8000)[0]).collect();
        assert_eq!(samples, [0, 11585, 16384, 11585, 0, -11585, -16384, -11585]);
        input.clear();
        assert_eq!(input.next(8000), [0; 2]);
        assert_eq!(input.next(8000), [0; 2]);
    }
}
