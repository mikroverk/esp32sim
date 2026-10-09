// ESP-IDF v5.5.5 components/soc/esp32s3/include/soc/gpio_sig_map.h:56-71;
// esp32c3:38-43 and esp32c6:34-39. Select/invert fields in register/soc/gpio_reg.h:
// S3:2672-2678,7804-7810; C3:1325-1331,3897-3903; C6:2466-2473,4998-5005.
// Selection compares the data input, BCLK output and WS output including inversion bits.
// Routing is a matrix-level model; pad enable, IO_MUX and serial edges are not simulated here.
use crate::gpio::Gpio;
use crate::clocked_queue::ClockedQueue;

/// Physical microphone wiring. PDM supplies already converted PCM, not raw bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmPins {
    I2s { bclk: u8, ws: u8, data: u8 },
    Pdm { clk: u8, data: u8 },
}

/// A host-clocked stereo PCM16 source. Duplicate mono samples into both lanes.
/// Time advances even when no receiver or DMA channel is running, evaluated on access.
pub struct PcmSource {
    pins: Option<PcmPins>,
    queue: ClockedQueue<[i16; 2]>,
    tone: Option<(f64, f64, f64)>,
}
impl Default for PcmSource {
    fn default() -> Self { Self { pins: None, queue: ClockedQueue::new(32768, [0; 2], false).unwrap(), tone: None } }
}
impl PcmSource {
    pub fn new(rate: u32, pins: PcmPins) -> Result<Self, &'static str> {
        let valid = match pins {
            PcmPins::I2s { bclk, ws, data } => bclk < 49 && ws < 49 && data < 49 && bclk != ws && bclk != data && ws != data,
            PcmPins::Pdm { clk, data } => clk < 49 && data < 49 && clk != data,
        };
        if !valid { return Err("PCM source needs distinct GPIOs in 0..49"); }
        Ok(Self { pins: Some(pins), queue: ClockedQueue::new(rate, [0; 2], false)?, tone: None })
    }
    pub fn queued_frames(&self) -> usize { self.queue.frames.len() }
    /// Routed sources retain the newest two seconds; per-port input accepts up to 65536 frames.
    pub fn push(&mut self, frames: &[[i16; 2]]) -> usize {
        self.tone = None;
        let n = if self.pins.is_none() { frames.len().min(65536 - self.queue.frames.len()) } else { frames.len() };
        self.queue.push(frames[..n].iter().copied());
        n
    }
    pub fn tone(&mut self, hz: f64, amplitude: f64) -> Result<(), &'static str> {
        if !hz.is_finite() || !(0.0..=192000.0).contains(&hz) || !amplitude.is_finite() || !(0.0..=1.0).contains(&amplitude) {
            return Err("I2S tone needs frequency 0..192000 Hz and amplitude 0..1");
        }
        self.queue.frames.clear();
        self.tone = Some((hz, amplitude, 0.0));
        Ok(())
    }
    pub fn clear(&mut self) { self.queue.frames.clear(); self.tone = None; }
    pub(super) fn next(&mut self, rate: u32) -> [i16; 2] {
        if let Some((hz, amplitude, phase)) = &mut self.tone {
            let sample = (phase.sin() * *amplitude * 32767.0).round() as i16;
            *phase = (*phase + std::f64::consts::TAU * *hz / rate as f64) % std::f64::consts::TAU;
            [sample; 2]
        } else {
            self.queue.advance(1, u64::from(self.queue.rate));
            self.queue.current
        }
    }
    fn sample(&self, cycles: u64, cpu_hz: u64) -> [i16; 2] { self.queue.sample(cycles, cpu_hz) }
    fn advance(&mut self, cycles: u64, cpu_hz: u64) { self.queue.advance(cycles, cpu_hz); }
}

/// Shared by all RX ports. Attaching the bank selects physical routing, even when empty.
/// Slots, audio and clock phase survive chip reset. Lowest matching slot wins.
#[derive(Default)]
pub struct PcmSources {
    pub inputs: Vec<Option<PcmSource>>,
    now: u64,
}
impl PcmSources {
    /// Synchronize once before reading/pushing or sampling an RX interval.
    pub fn advance_to(&mut self, now: u64, cpu_hz: u64) {
        self.advance(now.saturating_sub(self.now), cpu_hz);
        self.now = self.now.max(now);
    }

    pub(crate) fn active(&self) -> bool {
        self.inputs.iter().any(Option::is_some)
    }
    fn advance(&mut self, cycles: u64, cpu_hz: u64) {
        for source in self.inputs.iter_mut().flatten() {
            source.advance(cycles, cpu_hz);
        }
    }
    pub(crate) fn select(&self, gpio: &Gpio, signals: RxSignals, pdm: bool) -> Option<usize> {
        let RxSignals {
            data,
            input_select_bit,
            output_mask,
        } = signals;
        self.inputs.iter().position(|source| {
            let Some(source) = source else {
                return false;
            };
            let (din, bclk, ws) = match (source.pins, pdm) {
                (Some(PcmPins::I2s { bclk, ws, data }), false) => (data, bclk, Some(ws)),
                (Some(PcmPins::Pdm { clk, data }), true) => (data, clk, None),
                _ => return false,
            };
            gpio.func_in_sel[data] & ((1 << (input_select_bit + 1)) - 1)
                == u32::from(din) | (1 << input_select_bit)
                && gpio.func_out_sel[bclk as usize] & output_mask
                    == data as u32 + if pdm { 2 } else { 1 }
                && ws.is_none_or(|ws| {
                    gpio.func_out_sel[ws as usize] & output_mask == data as u32 + 2
                })
        })
    }
    pub(crate) fn sample(&mut self, id: usize, cycles: u64, cpu_hz: u64) -> [i16; 2] {
        let source = self.inputs[id].as_mut().unwrap();
        // Zero-order resampling; no anti-alias filter.
        source.sample(cycles, cpu_hz)
    }
}

/// Chip-specific GPIO matrix encoding for an RX controller.
#[derive(Clone, Copy)]
pub struct RxSignals {
    pub data: usize,
    pub input_select_bit: u32,
    pub output_mask: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absolute_time_is_lazy_monotonic_and_handles_long_stalls() {
        let mut sources = PcmSources::default();
        sources.inputs.resize_with(16, || None);
        sources.advance_to(80000, 8000);
        let mut source = PcmSource::new(8000, PcmPins::Pdm { clk: 1, data: 2 }).unwrap();
        source.push(&[[1; 2], [2; 2], [3; 2]]);
        sources.inputs[0] = Some(source);
        sources.advance_to(80001, 8000);
        assert_eq!(sources.inputs[0].as_ref().unwrap().queue.current, [1; 2]);
        sources.advance_to(0, 8000);
        sources.advance_to(80002, 8000);
        assert_eq!(sources.inputs[0].as_ref().unwrap().queue.current, [2; 2]);
        sources.advance_to(u64::MAX, 8000);
        let source = sources.inputs[0].as_ref().unwrap();
        assert_eq!(source.queued_frames(), 0);
        assert_eq!(source.queue.current, [0; 2]);
    }

    #[test]
    fn lowest_matching_source_wins_and_empty_bank_is_silent() {
        use crate::i2s::I2s;
        let mut sources = PcmSources::default();
        sources.inputs.resize_with(16, || None);
        let mut gpio = Gpio::new();
        gpio.func_in_sel[25] = 6 | (1 << 7);
        gpio.func_out_sel[4] = 26;
        gpio.func_out_sel[5] = 27;
        let signals = RxSignals { data: 25, input_select_bit: 7, output_mask: 0x3ff };
        for id in [0, 15] {
            sources.inputs[id] = Some(PcmSource::new(8000, PcmPins::I2s { bclk: 4, ws: 5, data: 6 }).unwrap());
        }
        assert_eq!(sources.select(&gpio, signals, false), Some(0));
        sources.inputs[0] = None;
        assert_eq!(sources.select(&gpio, signals, false), Some(15));
        let mut rx = I2s::new(160_000_000);
        rx.write(0x30, (1 << 26) | (2 << 27) | 25);
        rx.write(0x28, (24 << 7) | (15 << 13) | (15 << 18));
        rx.write(0x50, (1 << 16) | 3);
        rx.write(0x20, 4);
        rx.rx_input().push(&[[12, 34]]);
        assert_eq!(rx.receive(20000, 20000, false, &gpio, signals, Some(&mut sources)), [0; 4]);
        sources.inputs[15] = None;
        assert_eq!(rx.receive(20000, 20000, false, &gpio, signals, Some(&mut sources)), [0; 4]);

    }

    #[test]
    fn source_clock_bound_and_underrun() {
        let pins = PcmPins::I2s {
            bclk: 4,
            ws: 5,
            data: 6,
        };
        assert!(PcmSource::new(0, pins).is_err());
        assert!(PcmSource::new(8000, PcmPins::Pdm { clk: 49, data: 1 }).is_err());
        assert!(PcmSource::new(8000, PcmPins::Pdm { clk: 1, data: 1 }).is_err());
        let mut source = PcmSource::new(8000, pins).unwrap();
        source.push(&[[1; 2], [2; 2], [3; 2]]);
        assert_eq!(source.sample(9999, 80_000_000), [0; 2]);
        assert_eq!(source.sample(10000, 80_000_000), [1; 2]);
        source.advance(15000, 80_000_000);
        assert_eq!(source.queued_frames(), 2);
        assert_eq!(source.sample(4999, 80_000_000), [1; 2]);
        assert_eq!(source.sample(5000, 80_000_000), [2; 2]);
        source.advance(25000, 80_000_000);
        assert_eq!(source.sample(0, 80_000_000), [0; 2]);
        source.push(&vec![[11; 2]; 16000]);
        source.push(&[[22; 2]]);
        assert_eq!(source.queued_frames(), 16000);
        assert_eq!(source.sample(16000 * 10000, 80_000_000), [22; 2]);
        source.push(&vec![[33; 2]; 16001]);
        assert_eq!(source.queued_frames(), 16000);
        assert_eq!(source.sample(10000, 80_000_000), [33; 2]);
        source.advance(80_000_000 * 3, 80_000_000);
        assert_eq!(source.queued_frames(), 0);
        assert_eq!(source.queue.current, [0; 2]);
    }

    #[test]
    fn resampling_is_tick_independent_and_shared_by_receivers() {
        use crate::i2s::I2s;
        let run = |ticks: &[u64]| {
            let mut sources = PcmSources::default();
            sources.inputs.resize_with(16, || None);
            let mut input = PcmSource::new(
                16000,
                PcmPins::I2s {
                    bclk: 4,
                    ws: 5,
                    data: 6,
                },
            )
            .unwrap();
            input.push(&(1..=16).map(|sample| [sample; 2]).collect::<Vec<_>>());
            sources.inputs[0] = Some(input);
            let mut gpio = Gpio::new();
            gpio.func_in_sel[25] = 6 | (1 << 7);
            gpio.func_out_sel[4] = 26;
            gpio.func_out_sel[5] = 27;
            let signals = RxSignals {
                data: 25,
                input_select_bit: 7,
                output_mask: 0x3ff,
            };
            let mut receivers = [I2s::new(160_000_000), I2s::new(160_000_000)];
            for rx in &mut receivers {
                rx.write(0x30, (1 << 26) | (2 << 27) | 25);
                rx.write(0x28, (24 << 7) | (15 << 13) | (15 << 18));
                rx.write(0x50, (1 << 16) | 3);
                rx.write(0x20, 4);
            }
            let mut output = [Vec::new(), Vec::new()];
            let mut now = 0;
            for &cycles in ticks {
                now += cycles;
                for (rx, output) in receivers.iter_mut().zip(&mut output) {
                    output.extend(rx.receive(cycles, now, false, &gpio, signals, Some(&mut sources)));
                }
                sources.advance_to(now, 160_000_000);
            }
            assert_eq!(output[0], output[1]);
            let source = sources.inputs[0].as_ref().unwrap();
            assert_eq!(source.queued_frames(), 8);

            output[0].clone()
        };
        let expected: Vec<_> = [2i16, 4, 6, 8]
            .into_iter()
            .flat_map(|n| [n, n].into_iter().flat_map(i16::to_le_bytes))
            .collect();
        assert_eq!(run(&[80000]), expected);
        assert_eq!(run(&[1, 9998, 20002, 17000, 33000 - 1]), expected);
    }

    #[test]
    fn routing_checks_every_wire_and_inversion() {
        for (data, bit, mask) in [
            (25, 7, 0x3ff),
            (30, 7, 0x3ff),
            (15, 6, 0x1ff),
            (15, 7, 0x1ff),
        ] {
            let signals = RxSignals {
                data,
                input_select_bit: bit,
                output_mask: mask,
            };
            let mut sources = PcmSources::default();
            sources.inputs.resize_with(16, || None);
            sources.inputs[15] = Some(
                PcmSource::new(
                    8000,
                    PcmPins::I2s {
                        bclk: 4,
                        ws: 5,
                        data: 6,
                    },
                )
                .unwrap(),
            );
            let mut gpio = Gpio::new();
            gpio.func_in_sel[data] = 6 | (1 << bit);
            gpio.func_out_sel[4] = data as u32 + 1;
            gpio.func_out_sel[5] = data as u32 + 2;
            assert_eq!(sources.select(&gpio, signals, false), Some(15));
            assert_eq!(sources.select(&gpio, signals, true), None);
            for pin in [4, 5] {
                gpio.func_out_sel[pin] ^= 1;
                assert_eq!(sources.select(&gpio, signals, false), None);
                gpio.func_out_sel[pin] ^= 1;
            }
            gpio.func_in_sel[data] ^= 1 << (bit - 1);
            assert_eq!(sources.select(&gpio, signals, false), None);
            gpio.func_in_sel[data] ^= 1 << (bit - 1);
            gpio.func_in_sel[data] ^= 1 << bit;
            assert_eq!(sources.select(&gpio, signals, false), None);
            gpio.func_in_sel[data] ^= 1 << bit;
            sources.inputs[15] =
                Some(PcmSource::new(8000, PcmPins::Pdm { clk: 4, data: 6 }).unwrap());
            gpio.func_out_sel[4] = data as u32 + 2;
            gpio.func_out_sel[5] = 0;
            assert_eq!(sources.select(&gpio, signals, true), Some(15));
            assert_eq!(sources.select(&gpio, signals, false), None);
        }
    }
}
