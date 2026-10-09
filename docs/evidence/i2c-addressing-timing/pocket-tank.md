# Pocket Tank correctness: finite I2C time changes the simulation seed

Verdict: expected workload divergence from finite I2C transfers, not an
addressing failure, new NACK or stalled transfer. The shared controller needs
no correction for this workload. Exact hardware timing remains unvalidated.

Base is main `017af5241f48855c412d912ff91f0a801dd3c1bd`; the controller source used for these runs is identified by
`candidate_controller_sha256` in `pocket-tank.json`; firmware and other sources
are identified by `inputs.json`. `pocket-tank.json` records console
hashes, instruction counts, stop totals and per-source interrupt counts.
No CPU benchmark was run. The CLI's incidental wall-time/speed fields are
excluded from evidence and are not used for this conclusion.

## Console comparison

Both runs use the committed Pocket Tank app, bootloader and partition table,
the fetched model, default AMOLED board, default IMU mode 0, both console
channels, no inputs, and 30 emulated seconds.

| Observation | Main | Candidate |
|---|---|---|
| Instructions | 10,073,833,775 | 10,656,135,787 |
| Console SHA-256 | `c3776029dd98a1cd1ad148aa3cfdbd6639a49e465157214c2f1ca34ac5e2bbd5` | `880ce5621ddef2109e54978f10e698ee2efd954dd5b7504a8f144c9c25fe8100` |
| First changed line, board revision V2 | 293 ms | 294 ms |
| Display initialization warning | 373 ms | 374 ms |
| Panel/touch/battery ready | 473 ms | 474 ms |
| IMU reset/configuration and director ready | 498 ms | 499 ms |
| RTC initialized and population selected | 498 ms; pip + nori | 500 ms; pip + kelp |
| Stats card and first IMU poll | 543 ms | 545 ms |
| Return from app_main | 554 ms, after first IMU log | 545 ms, before first IMU log |
| Next two IMU polls | 800/1056 ms | 801/1057 ms |
| First advisor decision | seek_food, urgency 5, p=0.41 | follow_friend, urgency 4, p=0.98 |
| 10-second tank report | 10553 ms; seek_food/seek_food; asks 6, decisions 5 | 10571 ms; follow_friend/seek_food; asks 7, decisions 4 |
| 20-second tank report | 20556 ms; follow_friend/explore; asks 12, decisions 10 | 20574 ms; follow_friend/explore; asks 13, decisions 10 |
| Display render/flush at 10 seconds | 3.2/6.1 ms | 3.4/6.3 ms |
| Display render/flush at 20 seconds | 4.0/6.1 ms | 4.3/6.3 ms |

The first 170 console lines are identical. The remaining differences are
initialization/poll timestamps, USB/UART duplication interleaving, changed fish
names/personalities/positions and advisor prompts/goals/probabilities, and the
resulting guest-reported rendering/inference durations. Framebuffer sample
`0x0125`, vegetation values, heap sizes, zero ignored starvation events,
62.6/62.5 fps reports and 24.3 tok/s remain the same. The common MSPI warning
and RTC-unset warning remain; neither run adds an I2C error. Touch ID remains
183; IMU reset ACK and control readback remain `40/08/01`; acceleration stays
zero in the existing mode-0 stub.

The console diff was inspected directly, including its interleaved fragments.
The exact two consoles and unified diff are retained with the local PR handoff;
`pocket-tank.json` identifies the bytes. They are not added as raw logs to Git.

## Which transfers change

I2C0, GPIO15 SDA / GPIO14 SCL on `waveshare-amoled18-v2`. None of these board
devices overrides address matching or uses aliases/general call. All differences
here come from elapsed transfer time.

The first three transfers write TCA9554 address `0x20`: register/value
`03/78`, `01/80`, `01/87`. Each used to complete synchronously; each now takes
72.5 microseconds of modeled bus time. Touch address `0x15`, PMIC `0x34`, IMU
`0x6b` and RTC `0x51` follow. The 17 initialization command lists total
141,200 APB ticks, **1.765 ms** of newly modeled bus time before population
selection. Scheduling and log rounding explain why printed deltas are 1–2 ms.

A separate first-second diagnostic comparison has **825 identical address/data
events** after normalizing only `W/R` versus `read=false/true` spelling. It
contains 112 command lists and 336 checked address/write ACKs, zero NACKs.
It includes all initialization, 94 touch reads, and six IMU reads. In particular:

- Touch identification: `0x15`, register `0xa7` returns `0xb7`.
- PMIC and IMU probes ACK at `0x34` and `0x6b`.
- IMU WHO_AM_I returns `0x05`; reset `60/b0` ACKs; configuration `02/40`,
  `03/08`, `08/01` reads back unchanged.
- RTC reads seven zeros from `0x04`, then accepts the same build-time bytes
  `37 47 17 11 05 09 26` in both runs.
- Recurring touch reads of five bytes at `0x02` take 187.5 microseconds;
  IMU six-byte reads at `0x35` take 210 microseconds. Replies remain zero.

This is not a timeout explanation: these are successful, finite transfers.
The comparison directly checks the first second; the full console checks the
30-second run. It does not claim a complete 30-second byte trace.

## Why this is expected, and the limits of that conclusion

The binary identifies ESP-IDF **v5.4.1**. Its observed timing registers, in
order `00/38/40/44/48/4c/54`, are:

- Normal: `49,11803,49,49,49,49,2097152`.
- Probes: `199,50278,199,199,199,199,2097152`.

[IDF v5.4.1 S3 i2c_ll.h](https://github.com/espressif/esp-idf/blob/v5.4.1/components/hal/esp32s3/include/hal/i2c_ll.h#L92-L115)
calculates bus periods; lines 175–201 program low/setup/hold minus one and
literal high/wait-high, just like the previously checked v5.5.4 header.
[Its i2c_reg.h](https://github.com/espressif/esp-idf/blob/v5.4.1/components/soc/esp32s3/register/soc/i2c_reg.h#L1043-L1081)
defines divider, source and active fields. The observed clock has divider 1,
XTAL source and clock active. At modeled 40 MHz, normal SCL is
`(49+1)+27+23 = 100` clocks, or 400 kHz; probes are
`(199+1)+102+98 = 400` clocks, or 100 kHz. Nine SCL clocks per byte/ACK give
22.5 and 90 microseconds respectively. The model's START/STOP sums are 2.5
and 10 microseconds. There is no factor-of-two clock error or long timeout
hidden in these values. START/STOP scheduling remains an inference, not a
physical-bus measurement; stretching, arbitration and oscillator calibration
remain outside the model.

The public [Pocket Tank v0.2.0 main.c, lines 691–692](https://github.com/mediacutlet/pocket-tank/blob/v0.2.0/firmware/main/main.c#L691-L692)
seeds `tank_init` with `(uint32_t)esp_timer_get_time() ^ 0xC0FFEEu` immediately
after RTC initialization. Its [IMU driver, lines 51–56 and 84–85](https://github.com/mediacutlet/pocket-tank/blob/v0.2.0/firmware/main/imu_port_qmi8658.c#L51-L56)
uses 100 ms transaction timeouts and 400 kHz transfers. That public tag is
source corroboration, **not a claim that the older committed binary was built
from that tag**. Source hashes are retained. The seed explanation is inferred
from that source and the observed population change; the control below proves
the causal effect of bus time independently of source-version identity.

Control: use the candidate unchanged except append `self.advance(u64::MAX);`
after `self.schedule();` in `I2c::run()`, making transfers synchronous again.
The 30-second console becomes **byte-identical to main**, with exactly
10,073,833,775 instructions, 1,006,391 exceptions, 95,179 interrupts, and the
same per-source counts. This isolates elapsed I2C time from aliases, address
updates, board tick order and other branch changes. The control is not shipped.

Real I2C transfers take finite bus time, so allowing firmware time and its
seeded workload to change is appropriate. These results do not establish that
physical hardware produces these exact fish, timestamps or instruction counts.
The pre-existing touch/PMIC/RTC/IMU stubs are unchanged. A CPU comparison using
these two Pocket Tank runs would compare different guest work and cannot be
interpreted as emulator overhead.

## Reproduction and regression guard

```sh
CI=1 tools/fetch-pocket-tank.sh
cargo +1.99.0 build --release --bins
# Run this from the candidate root with each separately built binary:
"$BINARY" --boot rom --rom web/wasm/fw/esp32s3_rev0_rom.elf \
  --flash-mb 16 --psram-mb 8 --board waveshare-amoled18-v2 \
  --bootloader web/wasm/fw/public/pocket-tank-bootloader.bin \
  --ptable web/wasm/fw/public/pocket-tank-ptable.bin \
  --app web/wasm/fw/public/pocket-tank.bin \
  --flash-at 0x290000=web/wasm/fw/local/model_q4.bin \
  --max-seconds 30 --no-dump >"$CONSOLE" 2>"$REPORT"
diff -u main.console candidate.console
```

Build main from `git archive 017af524` into a worktree-local scratch directory;
use this worktree's firmware paths for both binaries. For the first-second
trace, replace 30 with 1 and add `--debug i2c`. `pocket-trace.patch` adds only
diagnostic logging of timing registers, deadlines and ACK outcomes to the
candidate. Apply it with `git apply --unidiff-zero` before the diagnostic build and
reverse it with `git apply -R --unidiff-zero` afterward;
its output does not alter the transfer state. Normalize the address log
spelling and compare only lines beginning `[i2c]`.

`pocket_tank_amoled_transfers_complete_at_programmed_bus_rate` replays the
observed expander, probe, touch-ID, IMU configuration/readback and RTC-read
transfers against the real AMOLED board device set. It asserts no completion
one APB tick early, completion on the expected tick, no NACK, exact FIFO data,
and command-done bits. It uses committed register fixtures and needs no model
file, firmware build or developer-machine input. Changing nine clocks per
byte to ten kills it; all 39 mutations in `mutations.json` are killed.
