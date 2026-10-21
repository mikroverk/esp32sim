# Classic PSRAM CLI and delay-clock command qualification

Base: upstream main `77e43c20a70054ac84d1724d757fa9ce12668151`.
Implementation: `86c6d0bb9bd8e1d32cfea10e5fce268b7552577f`, on the rebased
classic PSRAM series. Documentation-only descendants do not change tested Rust.

## Command contract and independent devices

The rebase retains both the octal CS0 flash state/commands and size-dependent
CS1 octal MR2 from #232, and the independently opted-in classic quad device.
Classic quad phase decoding is reached only with `classic_quad.is_some()`;
octal flash commands only with `octal_flash`. No JIT implementation changes.

ESP-IDF v5.5.1, `components/esp_psram/esp32/esp_psram_impl_quad.c:421-519`
and `:936-940` selects delay-clock mode when PSRAM and flash clock pins differ;
it is not a rev0-only protocol. For 40 MHz PSRAM it sends:

| operation | phases | result |
| --- | --- | --- |
| exit QPI | MOSI only, 16 bits, word `0xF500` | returns to SPI mode |
| read ID | two zero command bits, 32 address bits `0x9F000000`, 64 MISO bits | words `0x00225D0D`, `0x00000000` for 4 MiB |
| enter QPI | two zero command bits, 8 address bits `0x35` | enters QPI mode |

The extra bits delay the external clock; this model decodes the resulting
transaction without modelling wire timing. The ordinary 80/80 MHz command
shapes remain supported. Tests also reject malformed command width/value,
address width and MISO length, retain the QPI gate, and return all ones without
a backing device. The existing octal/quad flash and PSRAM tests remain controls.

The classic CLI now passes explicit 0/4 MiB to the existing bus setter. Its
default remains absent. Unsupported classic sizes, including a value that
would wrap on shifting, fail with an option error before shifting. The CLI
boot test executes the committed IDF allocator fixture through the real binary,
not a test-only bus call; 4 MiB passes, explicit zero and the absent default
produce the no-device ID error.

## Official MicroPython SPIRAM

Download an unmodified image locally (not redistributed by this change):

```sh
curl -fL https://micropython.org/resources/firmware/ESP32_GENERIC-SPIRAM-20260824-v1.29.0.bin -o micropython-spiram.bin
# SHA-256 a8dc95b75cbbda9e5702a37166f94d08469f5b9046262be7187b1911ac07a358
# size 1612336 bytes
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" MICROPYTHON_SPIRAM_IMAGE="$PWD/micropython-spiram.bin" \
  nice -n 10 cargo +1.99.0 test --release -j2 -p esp32sim \
  --test classic_psram_micropython -- --ignored --nocapture
```

The harness writes the complete official image at flash offset `0x1000`, boots
from the ECO3 ROM, waits for `>>>`, and sends
`import gc; print('PSRAM_FREE', gc.mem_free())` over the actual UART.
It does not patch guest code, registers, mappings or return values, and requires
zero stubs, intercepted calls and reboots. The banner is
`MicroPython v1.29.0 on 2026-08-24; Generic ESP32 module with SPIRAM with ESP32`.

| configured PSRAM | observed `gc.mem_free()` bytes | ID read error |
| --- | ---: | --- |
| 4 MiB | 4178272 | absent |
| none | 160096 | `0xffffffff`, startup continues without external RAM |

This is an end-to-end delay-clock regression, not timing or silicon validation.
The older stock Arduino fixtures select 80/80 MHz and do not independently
exercise these startup shapes. Public upstream MicroPython is MIT licensed;
this qualification retains only URL/hash/size and the small host test, not the
downloaded image or a claim to have audited every linked binary component.

## Verification

All 21 local CI-equivalent steps pass, including both Clippy modes with warnings
denied, production WASM sections, nine demos, timing/BLE APIs, worker/browser and
retained-receipt checks, the full 111126-case JIT differential run (99679 modules
released), 24 GPIO waveform cases (738 modules released), and profiling-feature
compilation. The ROM-enabled empty-HOME workspace suite passes 1036 tests with
29 external tests filtered; the no-ROM empty-HOME suite passes 1005 with 60
ignored. The native virtual-quantum tests pass 43/43. All 55 tracked golden files
are byte-identical to the base. These are local checks, not a remote CI run.

See `checks.json` for command-level exits, test totals, fixture input hashes and
mutation assertions. The test environment uses Rust 1.99.0, two build jobs,
empty HOME for both workspace test modes, and only an explicit ROM setting for
the ROM-enabled run. Build targets and raw console captures are local outputs.
No existing golden is changed. The classic hello golden includes instructions,
console and interrupt totals/per-source counts and is exercised by the ROM suite.

Run the SPI mutations in an isolated source worktree, with an owned target:

```sh
CARGO_BUILD_JOBS=2 python3 docs/evidence/classic-psram-spi/mutations.py "$PWD" OUTPUT
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" CARGO_BUILD_JOBS=2 \
  python3 docs/evidence/classic-psram-dclk/mutations-cli.py "$PWD" CLI_OUTPUT
```

The SPI script kills 31/31 mutations (the 24 inherited command mutations plus seven
DCLK mutations). The CLI size-wiring mutation is also assertion-killed. After
source restoration the SPI tests pass 8/8 and the CLI tests pass 2/2. The four
fixture profiles (IDF 4.4.7, Arduino 2.0.17, 3.0.7 and 3.1.1) and the separate
stock low/high prerequisite replay all pass; reused Arduino firmware hashes
match the retained provenance. The fixture batch also reruns MicroPython and
the CLI boot/size checks.

A mutation kill requires its named runtime assertion to fail, not a compiler
error. The new rows remove DCLK ID, exit and entry decoding, or weaken the ID
command-width, zero-command, address-width and MISO-width requirements.

## CPU comparison

PENDING. No CPU timing was run. This command-engine/CLI change adds no per-tick
work. The pre-existing absent-device fetch/load/store range checks still cost
code and are not claimed to be idle-cost-free. Historical codegen receipts
predate this rebase and do not establish final-binary equivalence on this base.

## Limits and deferred review points

This qualification does not trim inherited evidence, repair its historical
references, add the experiments row or fold the hot-path range checks; those
are a separate follow-up after review. The model still has a shared cache gate
without caller-core identity: once APP is clocked, PRO accesses also require
APP cache enabled, masks clear and equal normal-mode tables. That is a model
restriction, not a hardware claim. Out-of-array SPI reads still return zero,
while an absent device returns all ones. Reboot still zeroes the backing as a
deterministic power-cycle policy, unlike warm hardware reset. Cache-disabled
accesses fault `Prohibited`, not the hardware cache-IA interrupt. Arduino
qualification still needs the pinned external public builds; only the IDF
allocator and CLI boot test use committed firmware in CI. DMA, PSRAM instruction
fetch, camera buffers, other capacities and detailed cache/wire timing are not
covered. No production or hardware evidence is claimed.
