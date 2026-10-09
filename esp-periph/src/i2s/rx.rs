// Register fields: ESP-IDF v5.5.5 components/soc/esp32s3/register/soc/i2s_reg.h
// lines 119-246 (RX_CONF), 375-416 (RX_CONF1), 677-692 (TDM), 1058-1065 (EOF).
// C3 has the same standard-RX fields, no PDM converter (esp32c3 i2s_reg.h:117-230).
// C6 keeps these fields (esp32c6 i2s_reg.h:149-290, 424-478, 930-945), but uses
// components/soc/esp32c6/register/soc/pcr_reg.h:719-784 for RX clocks.
// PCM packing/filter behavior is a model contract, not a hardware measurement.
use super::{I2s, PcmInput, PcmSources};

/// Cold receiver state behind one pointer, so an idle controller embeds no queue.
/// Controller 0 also holds the chip's pin-routed source bank, shared by every receiver.
#[derive(Default)]
pub(super) struct RxState {
    input: PcmInput,
    acc: u64,
    buffer: Vec<u8>,
    sources: Option<Box<PcmSources>>,
}

impl I2s {
    #[inline(always)]
    pub fn rx_running(&self) -> bool {
        self.rx_conf & 4 != 0
    }
    /// Host input for this receiver, allocating the receiver state on first use.
    pub fn rx_input(&mut self) -> &mut PcmInput {
        &mut self.rx.get_or_insert_with(Box::default).input
    }
    /// The chip's pin-routed source bank slot (kept on controller 0), allocating receiver state.
    pub fn pcm_bank(&mut self) -> &mut Option<Box<PcmSources>> {
        &mut self.rx.get_or_insert_with(Box::default).sources
    }
    /// Lend the source bank to a receive interval; return it with `restore_pcm_bank`.
    pub fn take_pcm_bank(&mut self) -> Option<Box<PcmSources>> {
        self.rx.as_mut().and_then(|rx| rx.sources.take())
    }
    pub fn restore_pcm_bank(&mut self, bank: Option<Box<PcmSources>>) {
        if let Some(rx) = &mut self.rx {
            rx.sources = bank;
        }
    }
    /// Carry host input and sources across a chip reset; the rebuilt receiver restarts its frame phase.
    pub fn keep_rx_input(&mut self, old: &mut I2s) {
        self.rx = old.rx.take().map(|mut rx| {
            rx.acc = 0;
            rx
        });
    }
    /// Return a buffer from `rx_data` for reuse by the next interval.
    pub fn recycle_rx_buffer(&mut self, bytes: Vec<u8>) {
        if let Some(rx) = &mut self.rx {
            rx.buffer = bytes;
        }
    }
    pub(super) fn reset_rx_phase(&mut self) {
        if let Some(rx) = &mut self.rx {
            rx.acc = 0;
        }
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
    pub fn receive(&mut self, cycles: u64, now: u64, pdm2pcm: bool,
        gpio: &crate::Gpio, signals: super::RxSignals, sources: Option<&mut super::PcmSources>) -> Vec<u8> {
        let cpu_hz = self.cpu_hz;
        if let Some(sources) = sources {
            sources.advance_to(now.saturating_sub(cycles), cpu_hz);
            let selected = if sources.active() { sources.select(gpio, signals, self.rx_conf & (1 << 20) != 0) } else { None };
            self.rx_data(cycles, pdm2pcm, |offset| selected.map_or([0; 2], |id| sources.sample(id, offset, cpu_hz)))
        } else {
            let mut input = self.rx.as_mut().map(|rx| std::mem::take(&mut rx.input)).unwrap_or_default();
            let rate = self.rx_rate().unwrap_or(1);
            let bytes = self.rx_data(cycles, pdm2pcm, |_| input.next(rate));
            if let Some(rx) = &mut self.rx {
                rx.input = input;
            }
            bytes
        }
    }

    pub fn rx_data(&mut self, cycles: u64, pdm2pcm: bool, mut next_frame: impl FnMut(u64) -> [i16; 2]) -> Vec<u8> {
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
            self.reset_rx_phase();
            return Vec::new();
        };
        let cpu_hz = self.cpu_hz;
        let rx = self.rx.get_or_insert_with(Box::default);
        let previous = rx.acc;
        rx.acc += cycles * u64::from(rate);
        let frames = rx.acc / cpu_hz;
        rx.acc %= cpu_hz;
        let mut bytes = std::mem::take(&mut rx.buffer);
        bytes.clear();
        for n in 1..=frames {
            let offset = (n * cpu_hz - previous).div_ceil(u64::from(rate));
            let frame = next_frame(offset);
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
    use crate::i2s::PcmInput;
    fn receiver() -> I2s {
        let mut i = I2s::new(160_000_000);
        i.write(0x30, (1 << 26) | (2 << 27) | 25);
        i.write(0x28, (24 << 7) | (15 << 13) | (15 << 18) | (15 << 24));
        i.write(0x50, (1 << 16) | 3);
        i.write(0x20, 4);
        i
    }
    const SIG: super::super::RxSignals = super::super::RxSignals { data: 15, clock: [16, 17], input_select_bit: 6, output_mask: 0x1ff };

    #[test]
    fn pcm_clock_slots_width_silence_and_reset() {
        let mut i = receiver();
        assert_eq!(i.rx_rate(), Some(8000));
        assert_eq!(i.rx_input().push(&[[1234, -2345], [111, 222]]), 2);
        assert!(i.receive(19999, 19999, true, &crate::Gpio::new(), SIG, None).is_empty());
        assert_eq!(i.receive(1, 1, true, &crate::Gpio::new(), SIG, None), [0xd2, 4, 0xd7, 0xf6]);
        i.write(0x20, 1); // peripheral reset keeps host input
        assert!(i.receive(20000, 20000, true, &crate::Gpio::new(), SIG, None).is_empty());
        i.write(0x20, 4 | (1 << 5));
        i.write(0x50, (1 << 16) | 2);
        assert_eq!(i.receive(20000, 20000, true, &crate::Gpio::new(), SIG, None), 222i16.to_le_bytes());
        assert_eq!(i.receive(20000, 20000, true, &crate::Gpio::new(), SIG, None), [0; 2]);
        for bits in [24, 32] {
            i.write(
                0x28,
                (24 << 7) | ((bits - 1) << 13) | (15 << 18) | (31 << 24),
            );
            i.rx_input().push(&[[123, -123]]);
            assert_eq!(i.receive(20000, 20000, true, &crate::Gpio::new(), SIG, None), (-123i32 << 16).to_le_bytes());
        }
        i.write(0x20, 4 | (1 << 20) | (1 << 21));
        i.write(0x28, (24 << 7) | (15 << 13));
        assert_eq!(i.rx_rate(), Some(4000));
        assert!(i.receive(40000, 40000, false, &crate::Gpio::new(), SIG, None).is_empty());
        i.rx_input().push(&[[1, 2]]);
        assert_eq!(i.receive(40000, 40000, true, &crate::Gpio::new(), SIG, None), [2, 0]);
    }
    #[test]
    fn unsupported_modes_preserve_queued_input() {
        for flag in [1 << 3, 1 << 7, 1 << 10, 1 << 11, 1 << 18, 1 << 20] {
            let mut i = receiver();
            i.rx_input().push(&[[17, -19]]);
            i.write(0x20, 4 | flag);
            assert!(i.receive(40000, 40000, true, &crate::Gpio::new(), SIG, None).is_empty(), "flag {flag}");
            i.write(0x20, 4);
            assert_eq!(i.receive(20000, 20000, true, &crate::Gpio::new(), SIG, None), [17, 0, 237, 255]);
        }
        for (off, value) in [(0x28, (24 << 7) | (7 << 13) | (15 << 18)), (0x50, 0x10000), (0x50, 0x10004), (0x50, 0x20003), (0x30, 0)] {
            let mut i = receiver();
            i.write(off, value);
            assert!(i.receive(20000, 20000, true, &crate::Gpio::new(), SIG, None).is_empty());
        }
    }

    #[test]
    fn reset_discards_fractional_receiver_time() {
        let mut i = receiver();
        i.rx_input().push(&[[1, 2]]);
        assert!(i.receive(19999, 19999, true, &crate::Gpio::new(), SIG, None).is_empty());
        i.write(0x20, 1);
        i.write(0x20, 4);
        assert!(i.receive(1, 1, true, &crate::Gpio::new(), SIG, None).is_empty());
        assert_eq!(i.receive(19999, 19999, true, &crate::Gpio::new(), SIG, None), [1, 0, 2, 0]);
    }

    #[test]
    fn idle_and_unsupported_receivers_allocate_nothing() {
        let mut i = I2s::new(160_000_000);
        i.write(0x24, 4);
        i.write(0x20, 4 | (1 << 3));
        assert!(i.receive(20000, 20000, true, &crate::Gpio::new(), SIG, None).is_empty());
        i.write(0x20, 1);
        i.recycle_rx_buffer(Vec::with_capacity(8));
        assert!(i.rx.is_none());
        let mut r = receiver();
        assert!(r.rx.is_none());
        assert_eq!(r.receive(20000, 20000, true, &crate::Gpio::new(), SIG, None), [0; 4]);
        assert!(r.rx.is_some());
    }

    #[test]
    fn active_gate_covers_both_directions_and_rx_writes_report() {
        use crate::{Device, WriteEffect};
        let mut i = I2s::new(160_000_000);
        assert!(!i.active());
        assert_eq!(Device::write(&mut i, 0x20, 4), WriteEffect::RX_CONF);
        assert!(i.active());
        assert_eq!(Device::write(&mut i, 0x24, 4), WriteEffect::NONE);
        Device::write(&mut i, 0x20, 0);
        assert!(i.active());
        Device::write(&mut i, 0x24, 0);
        assert!(!i.active());
        // A stopped transmitter loses its fractional frame phase at the next gate poll.
        i.write(0x24, 4);
        assert_eq!(i.frames_due(3000), 0);
        i.write(0x24, 0);
        assert!(!i.poll_active());
        i.write(0x24, 4);
        assert!(i.poll_active());
        assert_eq!(i.frames_due(1000), 0);
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
