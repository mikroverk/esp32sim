#!/usr/bin/env python3
"""Run source/ADC mutations with the EX214 restore-and-check runner."""
import importlib.util
import json
from pathlib import Path

spec = importlib.util.spec_from_file_location('rx_mutations', Path(__file__).resolve().parents[1] / 'i2s-rx-input/mutations.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
UNIT = runner.UNIT
PCM = 'esp-periph/src/i2s/sources.rs'
ADC = 'esp-periph/src/analog/stream.rs'
RX = runner.RX
BUS_PCM = ['cargo', '+1.99.0', 'test', '-p', 'esp32sim', '--test', 'pcm_sources']
BUS_ADC = ['cargo', '+1.99.0', 'test', '-p', 'esp32sim', '--test', 'adc_stream']
QUEUE = 'esp-periph/src/clocked_queue.rs'
runner.CASES = [
    ('PCM rate validation', QUEUE, '!(8000..=96000).contains(&rate)', 'false', UNIT, 'source_clock_bound_and_underrun'),
    ('PCM pin validation', PCM, 'if !valid', 'if false', UNIT, 'source_clock_bound_and_underrun'),
    ('PCM capacity', QUEUE, 'self.rate as usize * 2', 'self.rate as usize * 3', UNIT, 'source_clock_bound_and_underrun'),
    ('PCM oldest discard', QUEUE, 'self.frames.drain(..discard);', 'let _ = discard;', UNIT, 'source_clock_bound_and_underrun'),
    ('PCM current sample', QUEUE, 'self.current = self.sample(cycles, cpu_hz);', '', UNIT, 'source_clock_bound_and_underrun'),
    ('PCM phase retention', QUEUE, 'self.phase = (phase % u128::from(cpu_hz)) as u64;', 'self.phase = 0;', UNIT, 'source_clock_bound_and_underrun'),
    ('PCM lazy time', PCM, 'now.saturating_sub(self.now)', '0', UNIT, 'absolute_time_is_lazy_monotonic_and_handles_long_stalls'),
    ('PCM monotonic time', PCM, 'self.now.max(now)', 'now', UNIT, 'absolute_time_is_lazy_monotonic_and_handles_long_stalls'),
    ('PCM long clock', QUEUE, 'u128::from(cycles) * u128::from(self.rate)', 'u128::from(cycles * u64::from(self.rate))', UNIT, 'absolute_time_is_lazy_monotonic_and_handles_long_stalls'),
    ('PCM source priority', PCM, '.position(|source|', '.rposition(|source|', UNIT, 'lowest_matching_source_wins_and_empty_bank_is_silent'),
    ('PCM BCLK route', PCM, 'gpio.func_out_sel[bclk as usize] & output_mask', '(data as u32 + if pdm { 2 } else { 1 })', UNIT, 'routing_checks_every_wire_and_inversion'),
    ('PCM WS route', PCM, 'ws.is_none_or(|ws| {\n                    gpio.func_out_sel[ws as usize] & output_mask == data as u32 + 2\n                })', 'true', UNIT, 'routing_checks_every_wire_and_inversion'),
    ('PCM resample offset', RX, '(n * cpu_hz - previous).div_ceil(u64::from(rate))', 'cycles', UNIT, 'resampling_is_tick_independent_and_shared_by_receivers'),
    ('PCM empty bank silence', RX, 'if let Some(sources) = sources {', 'if let Some(sources) = sources.filter(|s| s.active()) {', UNIT, 'lowest_matching_source_wins_and_empty_bank_is_silent'),
    ('ADC rate validation', QUEUE, '!(8000..=96000).contains(&rate)', 'false', UNIT, 'stream_clock_bound_hold_and_shared_handle'),
    ('ADC clock validation', ADC, 'cpu_hz == 0 ||', 'false ||', UNIT, 'stream_clock_bound_hold_and_shared_handle'),
    ('ADC finite bias', ADC, '!bias.is_finite()', 'false', UNIT, 'stream_clock_bound_hold_and_shared_handle'),
    ('ADC finite voltage samples', ADC, 'samples.iter().any(|v| !v.is_finite())', 'false', UNIT, 'stream_clock_bound_hold_and_shared_handle'),
    ('ADC raw bias validation', ADC, 'if bias > 4095', 'if false', UNIT, 'raw_stream_counts_and_validation'),
    ('ADC raw sample validation', ADC, 'samples.iter().any(|&v| v > 4095)', 'false', UNIT, 'raw_stream_counts_and_validation'),
    ('ADC raw clock validation', ADC, 'cpu_hz == 0 ||', 'false ||', UNIT, 'raw_stream_counts_and_validation'),
    ('ADC raw calibration bypass', 'esp-periph/src/analog.rs', 'Some(AnalogSource::RawStream(stream)) => stream.sample(now, self.cpu_hz),', 'Some(AnalogSource::RawStream(stream)) => code(f32::from(stream.sample(now, self.cpu_hz))) as u16,', UNIT, 'raw_stream_counts_and_validation'),
    ('ADC capacity', QUEUE, 'self.rate as usize * 2', 'self.rate as usize * 3', UNIT, 'stream_clock_bound_hold_and_shared_handle'),
    ('ADC oldest discard', QUEUE, 'self.frames.drain(..discard);', 'let _ = discard;', UNIT, 'stream_clock_bound_hold_and_shared_handle'),
    ('ADC current sample', QUEUE, 'self.current = self.sample(cycles, cpu_hz);', '', UNIT, 'stream_clock_bound_hold_and_shared_handle'),
    ('ADC fractional phase', QUEUE, 'self.phase = (phase % u128::from(cpu_hz)) as u64;', 'self.phase = 0;', UNIT, 'stream_fractional_clock_is_read_independent'),
    ('ADC monotonic time', ADC, 'self.now.max(now)', 'now', UNIT, 'old_timestamps_do_not_replay_and_sources_replace_each_other'),
    ('ADC hold last sample', QUEUE, 'if self.hold', 'if false', UNIT, 'stream_clock_bound_hold_and_shared_handle'),
    ('PCM silence on underrun', QUEUE, 'if self.hold', 'if true', UNIT, 'source_clock_bound_and_underrun'),
]
# Conversions keep main's peripheral-clock timestamp; attached streams carry its bus-cycle offset.
ACROSS_RESET = 'stream_conversions_follow_time_across_reset'
runner.CASES.append(('stream clock offset applied', ADC, 'now.saturating_add(self.clock_offset)', 'now', BUS_ADC, ACROSS_RESET))
for variant in ['Stream', 'RawStream']:
    runner.CASES.append((variant + ' offset recorded', 'esp-periph/src/analog.rs', 'AnalogSource::' + variant + '(stream) => stream.clock_offset = offset,', 'AnalogSource::' + variant + '(_) => {}', BUS_ADC, ACROSS_RESET))
for chip,device in [('s3','rtc'),('c3','adc'),('c6','adc')]:
    soc = 'esp32' + chip + '/src/soc.rs'
    runner.CASES.append((chip + ' offset at reset', soc, 'p.' + device + '.analog.set_stream_clock_offset(self.cycles);', '', BUS_ADC, ACROSS_RESET))
    runner.CASES.append((chip + ' offset at attach', soc, 'self.periph.' + device + '.analog.set_stream_clock_offset(offset);', '', BUS_ADC, ACROSS_RESET))
for chip in ['c3', 'c6']:
    runner.CASES.append((chip + ' offset excludes peripheral time', 'esp32' + chip + '/src/bus.rs', 'self.cycles - self.periph.clock_cycles()', 'self.cycles', BUS_ADC, ACROSS_RESET))
runner.CASES.append(('s3 offset excludes deferred ticks', 'esp32s3/src/bus.rs', ' - u64::from(self.tick_pending)', '', BUS_ADC, ACROSS_RESET))
runner.CASES.append(('s3 offset excludes peripheral time', 'esp32s3/src/bus.rs', ' - self.periph.clock_cycles()', '', BUS_ADC, ACROSS_RESET))
runner.CASES.append(('bank returned after RX interval', RX, 'rx.sources = bank;', '', BUS_PCM, 'routing_timing_dma_stall_and_reset_on_all_chips'))
runner.CASES.append(('shared lazy host synchronization', 'esp-soc/src/soc.rs', 'sources.advance_to(cycles, cpu_hz);', '', BUS_PCM, 'routing_timing_dma_stall_and_reset_on_all_chips'))

if __name__ == '__main__':
    rows = runner.run()
    Path(__file__).with_name('mutations.json').write_text(json.dumps(rows, indent=2) + '\n')
    raise SystemExit(0 if all(row['killed'] for row in rows) else 1)
