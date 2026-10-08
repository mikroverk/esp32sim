# EX224: classic ESP32 peripheral adapters

Base: `esp32-classic-2-core`, `d80dd522`. Adds I2C0/1, sixteen LEDC
channels, SPI2/3 and eight RMT channels. Board callbacks are gated while no board is attached;
LED output uses the existing BoardModel/Ws2812Chain path. I2S RX belongs
to EX229. No MCPWM implementation is added for classic in this part.

The shared I2C engine is specialized by command count. Existing chips retain
exactly eight slots and no runtime layout field; classic has sixteen slots
and the old opcode mapping. Main's pin-address device matching is preserved.
Both slot counts expand one macro body into separate non-generic impls, so
S3/C3/C6 still compile one copy of each I2C method; a generic impl would be
instantiated in every calling crate and inlined differently there.
S3's checked descriptor reader and bounded descriptor visits move into
esp-periph as `dma_descriptor_reader!`, which expands them on each chip bus as
monomorphic code; the S3 expansion is the code S3 ran before. Classic expands
it over memory-only descriptor access and adds a finite chain walk with cycle
rejection. The S3 SPI, camera and crypto walks remain in place. DMA errors do
not publish classic TX completion or send malformed transfers to the board.

No existing chip gains fields or per-tick work. Classic peripheral clocks and
transaction timings are inferred. Hardware fade, RMT receive and bit-level
SPI timing are not modeled. RMT emits bits through the shared decoder and
BoardModel path; streams beyond 4096 bits are bounded and not published as
complete frames. There is no second pulse observer, FIFO mode or speculative
RX ownership error model. Continuous output retains only the latest lap.

Classic LEDC maps HS/LS banks onto two `Ledc<true>` groups: S3's eight-channel
map with C6-width timer fields, and a configuration write that cancels a
pending duty update. As with I2C, both variants expand one macro body. The
inline source hook selects APB, REF_TICK or RC_FAST per classic timer;
existing chips pass their single source clock. SPI receive words use the
shared `fill_spi_w!` macro and RMT bits the shared `ws2812_bit!` decoder;
S3/C3/C6 expand both to their previous inline code. I2C uses I2c<16> directly,
with blocked-line timeout handled at START. Pin routes use the shared router
over a linear copy of the classic pad registers in which the GPIO-matrix
function carries the S3/C3/C6 number; existing chips' pin tables are unchanged.
The inactive-board gate has an assertion test that rejects tick/MMIO callbacks.

The shared RMT engine generalization is not included: classic tests require
half-symbol deadlines, an idle tick between continuous laps, and wrapping a
borrowed allocation across word 511. The existing shared engine consumes whole
symbols and rejects allocations past the end of RAM. Unifying these contracts
would expand this part into a timing change for S3/C3/C6; retaining the classic
engine preserves both contracts without adding branches to their tick paths.

## Sources

ESP-IDF v5.5.4 `components/soc/esp32/register/soc/`:
`spi_reg.h` lines 361-372, 1285-1334 and 1414-1468 define transfer, link and
interrupt bits. `rmt_reg.h` lines 79-138 define idle, clock and continuous
controls. `ledc_reg.h` lines 1464-1472 and 1666-1674 define timer resolution.
`i2c_reg.h` defines sixteen command slots; `components/hal/esp32/include/hal/i2c_ll.h`
defines their old opcode mapping. `clk_tree_defs.h:40` supplies the nominal
8.5 MHz RC_FAST clock; the port corrects the prototype's 8 MHz value. Code comments cite these sources. The
firmware boot regression uses the IDF 5.5.4 fixture and hashes from EX223.
No hardware or IDF 4.4 validation is claimed.

## Checks

Use the full EX223 command set on this branch, including both Clippy targets,
empty-HOME workspace runs with ROM-only inputs and with no firmware variables,
all eight production WASM scenarios, and evidence privacy. No goldens are
regenerated. No JIT implementation changes.

[Mutation table](mutations.json) records 30 mutations killed by assertion
failures. Apply each single replacement and run
`cargo +1.99.0 test -p CRATE --lib TEST`, or `--test TARGET TEST` for
rows naming an integration target, then restore it. The LEDC shadow test
isolates channel updates from timer updates; neither missing latch gate is
hidden by the other one.

Results on Rust 1.99.0: both Clippy checks pass with warnings denied;
743 CI-mode workspace tests pass; 721 plain workspace tests pass with 36
ignored. Both workspace runs use an empty HOME. All eight production WASM
scenarios and evidence privacy pass. Goldens remain byte-identical.

## CPU comparison

PENDING

Static check (no timing): release `esp32sim` binaries from main `2f9443a9` and
this branch were disassembled with `llvm-objdump -d` and compared function by
function after normalising addresses, alignment `nop`s, adrp page offsets and
linker-chosen symbol aliases. The S3/C3/C6 execution path is instruction-identical
to main: `Machine::run`, `step_core`, each `SocBus` `tick` and `next_deadline`, S3 `tick_impl`,
`refresh_tick_budget` and GDMA engines, the MMIO dispatchers, `TimerGroup::tick`, and the UART,
TIMG, SHA, LEDC and RMT tick and register paths, and RTC_CNTL. Remaining differences: shared `Gpio::write` (`ubfx` for `lsr`, inherited from EX223); `I2c::write`, whose bounds-check panic calls share one source location in the macro
expansion, so three merge and the END arm moves; `Ledc::output`, the host PWM query, and the `Peripherals::new` constructors.

## Privacy and limits

Only source expressions, test names and outcomes are retained. No private
firmware, captures, machine identifiers or timing samples are included.
