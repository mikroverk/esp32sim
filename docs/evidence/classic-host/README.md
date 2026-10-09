# EX229: classic host input and I2S receive

Base: `esp32-classic-ble`
([#211](https://github.com/mikroverk/esp32sim/pull/211), `50178766`), with public
prerequisites [#201](https://github.com/mikroverk/esp32sim/pull/201) (`06c04176`) and
[#202](https://github.com/mikroverk/esp32sim/pull/202) (`725b527e`) merged by
`22d8db41`; both sides are on upstream main `2f9443a9`. The classic branch
retains its local predecessor as first parent.
The host PR must wait for those shared prerequisites; this is local ancestry,
not native GitHub stack registration.

Classic UART APB/AHB routing, matrix input, GPIO feedback/release, physical SPI,
raw ADC observations and Ethernet relay are already supplied by EX223-EX228.
This part adds I2S0/1 RX using the shared PCM packer, source bank, clocked queue
and receive scatter from EX214/EX215. The routing descriptor now gives BCLK/WS
indices explicitly because classic's data and clock signals are not adjacent.
All existing chip callers retain their previous indices.

The merge keeps one definition of the descriptor word/fault types
(`esp_periph::dma`) and one bounded descriptor walk, the `dma_descriptor_reader!`
macro: classic's bus expands it for its memory-only reader, and the bus-generic
expansion in `esp_periph::gdma` serves GDMA receive and S3's GDMA engines as in #202. Classic maps native register/interrupt values onto the
shared receive channel. Ownership, memory-only buffers and failed writeback
cannot publish successful EOF. A null next descriptor stops a completed
transfer; surplus samples report the shared descriptor-error condition.

## Idle structure and inputs

Classic fields are appended. Receiver state and the PCM bank use #202's boxed
per-controller state (the bank on controller 0), allocated on host access;
the shared queue advances lazily on host/RX access, without a per-tick source
scan. There is no separate receive Cargo feature. An inline clock/channel
gate guards receive work; the data/completion/fault helpers are inline too.
No per-tick work is added to S3/C3/C6. No CPU measurements were run.

Static check: fat-LTO release builds (`cargo +1.99.0 build --release -p esp32sim
--bins`) of this branch, #202 (`725b527e`) and main (`2f9443a9`) were disassembled
and compared per function after normalising addresses, padding and page offsets.
Against #202, every S3/C3/C6 hot function (`Machine::run`/`run_modeled`/
`run_unmodeled`/`step_core`, bus `tick`/`tick_impl`/`next_deadline`/
`refresh_tick_budget`/`periph_read`/`periph_write`, GDMA engines, `refresh_irq`)
is instruction-identical except: the RX-only helpers (C3/C6 `i2s_receive`, S3
`dma_i2s_port`), which store the two BCLK/WS indices and run only while RX is
active; and `RtcCntl::write`, `I2c::write`, `Gpio::write` and
`apply_due_script_events`, which differ from main exactly as on #211. Against
main, the remaining differences are those #202 already has. Classic's adapter
writes the shared I2S model through `Device::write`, like the other chips'
dispatch, so the shared inherent writer keeps one caller.

The inherited classic fixture/ROM hashes are in [EX223](../classic-core/README.md).
I2S tests generate their PCM samples and DMA descriptors in memory; there is no
additional firmware or external-input requirement. ADC stream tests use EX215's
shared typed raw stream and prove clocking, attenuation bypass and reset state.

## Sources and limits

Classic register definitions: ESP-IDF v5.5.4
`components/soc/esp32/register/soc/i2s_reg.h:93-98,738-756,1373-1390`.
`include/soc/gpio_sig_map.h:69-72,299,309-312,343` supplies I2S0/1 input data
and BCLK/WS indices. `include/soc/interrupts.h:50-51` supplies sources 32/33.
`register/soc/dport_reg.h:947,965` supplies I2S clock gates;
`i2s_reg.h:149-178` supplies descriptor-error and successful-EOF bits.
EX214/EX215 cite IDF v5.5.5 for their S3/C3/C6 layouts; classic uses the
v5.5.4 headers matching its inherited fixture. No IDF 4.4 validation is claimed.

The classic adapter supports standard 16/24/32-bit RX with the PLL clock.
APLL, external slave clocking, ADC-to-I2S capture, serial edge timing and classic
I2S TX are not implemented here. One shared DMA call uses RXEOF_NUM * 4,
including EOF across descriptors. IDF v5.5.4 `i2s_reg.h:676-682` defines
RXEOF_NUM at +0x24 with reset value 64; `components/hal/esp32/include/hal/i2s_ll.h:633-642`
converts bytes to register words. Word counts above u32::MAX/4 cannot fit the
shared DMA byte counter and report a descriptor error instead of wrapping.
Physical routing follows the shared matrix contract; it does not validate
IO_MUX/pad enable. No hardware microphone or audio-fidelity claim is made.
An attached empty source bank produces silence; per-controller input is used
only when no bank has been attached. Host source and queued PCM state survive
reset. No private application or firmware capture is part of these checks.

## Verification

Run the full [EX223 Rust 1.99.0 command set](../classic-core/README.md) on this
branch, including both Clippy targets, empty-HOME CI-mode and plain release
workspace tests, production WASM and all eight scenarios, and evidence privacy.

[Mutation table](mutations.json) records 19 replacements killed by assertion
failures. Run `cargo +1.99.0 test -p esp32 --test TARGET TEST` for integration
rows or `--lib TEST` otherwise, restoring source after each mutation. EX214/EX215
retain their own shared-model contracts and mutation receipts.

Results on Rust 1.99.0: both required Clippy targets pass; 816 CI-mode
workspace tests and 794 plain tests (36 ignored) pass with empty HOME.
All eight WASM scenarios pass.
Evidence privacy passes and existing goldens remain byte-identical. No JIT
implementation files changed.

The EOF regression covers a threshold spanning two descriptors, word-to-byte
scaling, reset value, zero/overflow counts and exhausted destinations. The PCM
packing test now runs exactly two frames for its two-frame buffer; overflow is
asserted separately. No firmware golden changes are needed. Shared RX tests
reuse one signal constant instead of repeated routing literals.

## CPU comparison

PENDING
