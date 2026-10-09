# EX219: LED routing and DMA output

Base: upstream main `2f9443a9` (includes #197, #200 and #204). Related EX205 provides CPU-fed
compact RMT and board pin transport. This change adds mirrored peripheral routes, S3 TX3 DMA
symbols, and C6 parallel output. It reuses the WS2812 timing constants that EX218 (#204) added
to `esp-soc/src/devices/ws2812.rs`; the GPIO waveform decoder itself is untouched.

## Behavior

RMT delivery iterates every valid GPIO matrix route, using main's `PinRoutes`
for IO_MUX, chip-specific masks, output inversion and output-enable selection.
Two enabled pins on the same signal each receive a frame. A disabled
software-enable route, inverted route, or non-GPIO mux function does not.

While TX3 DMA_ACCESS_EN is set, S3 stages finite DMA chains when RMT TX3 CONF0
is written, or when a GDMA register of a running trigger-9 OUT channel is written. It reuses the existing
`gather_dma_out` Result-based helper and bounded descriptor walker.
Automatic owner writeback is gated once in this shared helper. Channel 3 consumes
the staged symbols through the existing timed RMT transmitter. DMA mode bypasses
CPU-fed RMT memory exhaustion and threshold interrupts. FIFO reset and disabling
DMA discard staged symbols; starvation waits for more data.

C6 stages finite PARLIO chains on PARLIO and PCR TX-clock writes, and on GDMA
register writes of a running trigger-9 OUT channel, including prefill while the
TX clock is disabled. A single C6 gather helper, also used by
SPI2 and AES, owns descriptor validation, bounded traversal and writeback.
Malformed OUT chains report a GDMA error without delivering partial SPI/AES output.
A configured start, complete payload and enabled clock produce lane samples and
TX EOF. Packing widths 1, 2, 4, 8 and 16, both bit orders, mirrored lanes and
interrupt enable/clear are modeled. A free function next to `Ws2812Chain` converts
lane samples to the existing board `rmt_frame` API, iterating routes directly.
It uses the WS2812 timing constants EX218 added.
Completed PARLIO data stores samples only; width and clock remain on the peripheral.

Both staging paths honor descriptor ownership checks and automatic owner
writeback, preserve descriptor EOF addresses, and set DONE/EOF/TOTAL_EOF or
DSCR_ERR. Invalid descriptors, lengths, alignment, buffer addresses and cyclic
chains cannot produce unbounded output. S3 queues at most 4096 pixels' symbols
plus one end marker; C6 queues at most 65535 bytes. These are emulator limits.

## Idle path

Checked by disassembly, not timing: release binaries of main `2f9443a9` and this
branch (`cargo +1.99.0 build --release -p esp32sim --bins`, fat LTO, one codegen
unit), `objdump -d --demangle`, per-function comparison with instruction and
branch-target addresses and adrp page offsets normalised. 1856 functions are
identical; the S3 differences are listed here.

`Rmt` and every structure embedding it keep main's size and field offsets. The
TX3 queue is a nullable boxed `VecDeque` inside `Rmt`; it takes the 8 bytes of the
former `i64` CPU/APB ratio, which is now a `u8` in the tail beside `running`
(`running` stays at the same offset). S3 `Peripherals`, `SocBus`, the CPU run
loop, JIT helpers and bus read/fetch paths therefore compile to main's
instructions. The queue is allocated when staged symbols are inserted; FIFO
reset, DMA disable and a staging failure release it. S3 uses `RmtDma`, a
`repr(transparent)` wrapper, to select the DMA transmitter variant and to clear
the queue on CH3 CONF0 writes.

The transmitter loop is one inlined, const-specialized function. Plain
`Rmt::tick` (C3/C6) instantiates only the non-DMA variant. `RmtDma::tick` is out
of line like main's `Rmt::tick`; its idle path (prologue, `running` load, branch)
is instruction-identical, and `tick_impl` calls it where main calls `Rmt::tick`.
The only other `tick_impl` change on the idle path is the completed-frame test:
the route fanout is a cold out-of-line function, so the test branches over a call
instead of over main's inlined loop.

`periph_write_inner` is identical to main except the RMT dispatch target
(`RmtDma`'s write). Staging sits in the `periph_write` wrapper behind one bit
test of CH3 CONF0 DMA_ACCESS_EN: three instructions (offset, byte load, branch)
per peripheral write while RMT DMA is off. With the bit set, a cold function
checks the address: CH3 CONF0, or a GDMA write to a running trigger-9 channel.
Writes to other GDMA channels (SPI2, LCD, I2S, camera, crypto) do not stage.
No DMA pump or staging call runs from a device tick.

Remaining S3 differences: `gather_dma_out` carries the descriptor alignment,
length and trigger-9 checks plus the writeback gate (AES, SHA and RMT
transactions only; SPI2, LCD, I2S and camera use other walkers); `Rmt::tick`
widens the `u8` ratio on running channels; construction and drop code.
C3's `tick_with_pins` tests the completed-frame queue before calling a cold
fanout function.

C6 adds a pointer-sized PARLIO handle to `Peripherals`, which shifts later fields
by 8 bytes; the C6 bus and run-loop functions differ by those offsets. The PARLIO
device adds read/write dispatch arms. The C6 write path classifies PARLIO, GDMA
and the PCR PARLIO clock address inline and stages in a cold function; C6
`Peripherals::write32` keeps one address comparison for the PARLIO PCR clock.

PARLIO keeps a nullable boxed TX state at the end of `Peripherals`. Before its
first MMIO access or selected DMA transfer, no TX state or register RAM is
allocated. The pointer-sized handle avoids placing the 128-byte TX state among
hot fields through Rust's field reordering. Layout tests require the handle to
follow the system timer, clock tree and cached interrupt status.

PARLIO `clock()` remains `None`, including during a configured transfer, so
PARLIO never enters the optional-device active list. There is exactly one
PARLIO PCR `set_clock` call.

Source 63 uses the existing optional interrupt cache in status word 1, which
already contains LEDC and MCPWM. The `OPTIONAL_SOURCES` mask admits PARLIO's
cached bit without adding a device callback or another status-word load. Shared
interrupt routing is identical to upstream. Tests cover PARLIO's routed assertion
and acknowledgement, and prove the unused device has no storage, clock or active
entry.

## Register sources

Public ESP-IDF v5.5.4 sources, with paths relative to `components/`:

| Meaning | Header and lines |
| --- | --- |
| S3 DMA enable bit 25, TX3 only | `soc/esp32s3/register/soc/rmt_struct.h:117-120`; `hal/esp32s3/include/hal/rmt_ll.h:237-240` |
| S3 AFIFO reset bit 23, DMA failure bit 28 | `soc/esp32s3/register/soc/rmt_reg.h:575-581`, `1650-1656` |
| S3 GDMA trigger 9 | `soc/esp32s3/include/soc/gdma_channel.h:21` |
| S3 GDMA auto-writeback, owner check, output interrupts | `soc/esp32s3/register/soc/gdma_reg.h:585-618`, `649-678` |
| C6 PARLIO TX config, width, readiness and interrupts | `soc/esp32c6/register/soc/parl_io_reg.h:161-355` |
| C6 PARLIO PCR clock source, divider, gate, reset | `soc/esp32c6/register/soc/pcr_reg.h:1061-1098`; `hal/esp32c6/include/hal/parlio_ll.h:413-430` |
| C6 bit packing order | `hal/include/hal/parlio_types.h:28-50` |
| C6 approximate RC_FAST 17.5 MHz | `soc/esp32c6/include/soc/clk_tree_defs.h:47` |
| C6 GDMA trigger 9 | `soc/esp32c6/include/soc/gdma_channel.h:17` |
| C6 GDMA auto-writeback, owner check, output interrupts | `soc/esp32c6/register/soc/gdma_reg.h:1665-1716`, `699-732` |
| C6 PARLIO/PCR base addresses | `soc/esp32c6/register/soc/reg_base.h:34,53` |
| C6 PARLIO DATA0 signal 47 | `soc/esp32c6/include/soc/gpio_sig_map.h:84` |
| RMT signal bases S3=81, C3=51, C6=71 | `soc/esp32s3/include/soc/gpio_sig_map.h:158`; C3:84; C6:132 |
| C6 interrupt source 63 | `soc/esp32c6/include/soc/interrupts.h:83-85` |
| DMA descriptor size, length, EOF, owner and next fields | `hal/include/hal/dma_types.h:23-32` |

IDF v4.4.8 S3 `include/soc/rmt_struct.h:126` calls bits 25–31 reserved;
v5.5.4 explicitly names DMA_ACCESS_EN at bit 25. This port follows the IDF 5.x
DMA path, not an Arduino-ESP32 2.x DMA claim. GDMA writeback, owner-check and
interrupt bits match v4.4.8 `include/soc/gdma_reg.h:594-687`. C6 is IDF 5.x.
The PCR header's reset prose and unpack-order prose contain ambiguous values;
the LL reset sequence and `parlio_types.h` enum resolve the modeled meanings.
No hardware verification is claimed.

## Verification

`checks.json` records the final required command results on Rust 1.99.0. Both
workspace invocations use an empty HOME. The ignored-inclusive invocation sets
only ESP32SIM_ROM_DIR among emulator variables, to the checkout's `web/wasm/fw`;
the plain invocation has no emulator variables. CARGO_HOME and RUSTUP_HOME retain
access to the toolchain. The default toolchain is unchanged.

Inputs and source-test hashes are in `inputs.json`. Demo assets were obtained
with `tools/fetch-demo-assets.sh --no-linux` inside this checkout. No firmware
fixtures are copied from another checkout or required from a developer machine.
Existing goldens are unchanged. This branch does not modify JIT code.

The focused tests assert two independent RMT frame deliveries on each chip;
S3 multi-descriptor symbol output and RMT interrupt state; C6 clock-off prefill,
three routed outputs, both lane colours, descriptor handback and source-63 IRQ
assertion/clear. Additional tests cover widths, bit order, reset, starvation,
malformed descriptors and queue bounds. The shared C6 gather regression pins SPI
payloads, AES-128 zero-key/zero-input ciphertext, owner handback and completion
interrupt state with automatic writeback enabled and disabled. These are register-level workloads, not
a claim of running a particular FastLED or NeoPixel firmware release.

`mutations.json` records 38 exact mutations and the tests that kill them, including
the MMIO staging hooks and channel gates, the S3 DMA_ACCESS_EN write gate and CH3
CONF0 address gate, multi-route fanout, DMA mode, reset, ownership, lengths, the
trigger-9 alignment rule, writeback selection, EOF interrupts, queue bounds, the
RMT clock ratio and PARLIO packing/clock/IRQ. Storage/idle mutations force eager
PARLIO and RMT DMA allocation, retained RMT queue storage after disable, inline
PARLIO TX storage and an active PARLIO clock.
Each changed source is restored before the next mutation. Every mutation fails
an assertion in a previously passing focused test.

## Limits

Only finite DMA chains are supported. DMA staging and descriptor completion occur
at MMIO boundaries; FIFO backpressure, concurrent guest buffer modification and
continuous DMA rings are not modeled. PARLIO completes transactionally rather
than after elapsed wire time. It models TX data output, not RX, external input
clocks, valid-signal gating or every PCR reset interaction. Reserved widths and
odd byte counts for 16-bit samples do not complete.

WS2812 high widths of 150–1100 ns, a 550 ns bit split and 50 us reset detection are
inferred decoder policy. A transaction boundary also terminates a parallel frame.
These tolerances and completion rules are not silicon timing claims.

## CPU comparison

PENDING

## Evidence privacy

Retained evidence contains public header paths, hashes, commands and outcomes.
Local build logs and filesystem identities are omitted. No raw private captures
or hardware identifiers are retained.
