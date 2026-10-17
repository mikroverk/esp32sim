# Classic ESP32 RTC FAST-memory CRC completion

Base: upstream main `142805d5399e0c3cf25995fe21ba524e75a2b3cb`.
Independent bug fix; no stack dependency. No firmware, ROM or sketch patch.

## What is fixed, and what is not

Writing RTC_MEM_CONF.START now computes a CRC over the configured RTC FAST
memory words, writes RTC_MEM_CRC_RES and sets FINISH synchronously. The existing
AHB alias and subword merging paths are used. There is no new state or per-tick,
read-side, scheduler or instruction-dispatch work. The CRC uses the existing
shared `esp_soc::wifi::fcs` implementation, not another polynomial engine.
The helper is not inlined into the MMIO/store dispatcher. Normal, non-wrapping
ranges borrow the memory slice; only a wrapping range allocates a temporary buffer.

This removes the classic ROM's CRC polling hang and lets the public Schematik
Digital Hair Clip starter reach its deep-sleep request. It does NOT make deep
sleep or GPIO0 wake functional. The next gap is separately observable below.

## Register sources and model limits

Public ESP-IDF v5.5.4 sources, matching the repro's Arduino-ESP32 3.3.8 / IDF 5.5.4:

- [rtc_cntl_reg.h:2017-2039](https://github.com/espressif/esp-idf/blob/v5.5.4/components/soc/esp32/register/soc/rtc_cntl_reg.h#L2017-L2039): CONF +0x100, FINISH bit31, LEN[30:20], ADDR[19:9], START bit8,
  PID[7:0], result +0x104.
- [rom/rtc.h:172-190](https://github.com/espressif/esp-idf/blob/v5.5.4/components/esp_rom/esp32/include/esp32/rom/rtc.h#L172-L190): FAST memory, ADDR 0..0x7ff, LEN=0 is 4 bytes and LEN=0x7ff is 8192 bytes.
  ADDR is interpreted as a word index, LEN as words minus one.
- [rtc_sleep.c:301-353](https://github.com/espressif/esp-idf/blob/v5.5.4/components/esp_hw_support/port/esp32/rtc_sleep.c#L301-L353): start, poll FINISH, clear START, save result, then set SLEEP_EN.
- [ESP32 Technical Reference Manual](https://www.espressif.com/sites/default/files/documentation/esp32_technical_reference_manual_en.pdf), RTC wakeup entry discussion: RTC FAST integrity is checked again before its entry point is used.

The hardware CRC polynomial, initial state, reflection and final XOR could NOT
be established from those IDF declarations, the hardware-driving ROM routine or
the TRM. CRC-32/ISO-HDLC is an explicit deterministic approximation: reflected
polynomial 0xedb88320, initial 0xffffffff, final complement, increasing byte
addresses. It is NOT claimed to match silicon. Reusing the existing CRC provides
memory-sensitive results and internally consistent repeated checks, rather than
returning a fixed success value. That does not independently qualify wake-stub
integrity on hardware.

Overflow of ADDR+LEN is undocumented here: the model wraps the 11-bit word address.
FINISH/result are hardware-owned; a configuration write clears FINISH, START=1
computes immediately, START remains set until software clears it, result survives
clearing START, and guest result writes are ignored. These synchronous completion
and lifetime choices are model policies, not measured hardware timing. Preserved
configuration includes PID; PID does not select a different CRC buffer.

## Regression and mutations

`esp32/tests/rtc_crc.rs`: two bus/register regressions fail on the unmodified base
(runtime FINISH assertions, not compilation errors) and pass with this fix.
They cover nonzero word address, LEN+1, FAST rather than SLOW memory, full 8192-byte
range, changed memory through the instruction alias on a repeated START,
AHB alias, byte writes, PID/config preservation, clear START, hardware-owned
FINISH/result, one-word minimum and the explicit wrapping policy. Fixed model
vectors were cross-checked with Python's zlib CRC32, not with hardware.

All ten compile-valid mutations fail runtime assertions; see `mutations.json`:

| Mutation | Killing test (short name) |
| --- | --- |
| Remove START action | both |
| Remove FINISH | both |
| Ignore START bit | start_completes |
| Make FINISH writable | start_completes |
| Read SLOW instead of FAST | both |
| Ignore address | both |
| Omit LEN+1 | both |
| Corrupt result | both |
| Make result writable | start_completes |
| Clamp instead of wrap | alias_subword |

Reproduce in a disposable worktree with an isolated CARGO_TARGET_DIR:

```sh
nice -n 10 cargo +1.99.0 test --release -p esp32 --test rtc_crc
nice -n 10 python3 docs/evidence/classic-rtc-mem-crc/mutate.py "$OUT/mutations"
```

No new deep-sleep firmware fixture is added: CRC is covered by portable bus tests,
and starter bring-up stops at the next sleep-controller gap instead of pinning
that missing behavior as a successful sleep fixture.

## Starter reproduction and follow-up gap

Input provenance: public starter `esp32-digital-hair-clip`, fetched from
<https://www.schematik.io/api/starter-projects/esp32-digital-hair-clip> on October 16,
2026. Unchanged sketch; PlatformIO 6.1.19, pioarduino 55.03.38-1,
Arduino-ESP32 3.3.8, IDF 5.5.4, GFX Library for Arduino 1.6.9. No user project.
Optional build artifacts are not distributed by this fix; `repro.json` pins their
hashes. The automatic regressions need none of these artifacts or a mask ROM.
Button script:

```text
1 press 0 150
3 press 0 150
```

```sh
nice -n 10 cargo +1.99.0 build --release --bin esp32sim
# BASE and FIX are release binaries built at the base and this change respectively.
# FIXTURE contains firmware.bin, firmware.elf, bootloader.bin and partitions.bin.
nice -n 10 python3 docs/evidence/classic-rtc-mem-crc/repro.py \
  "$BASE" "$FIX" web/wasm/fw/esp32_rev300_rom.elf "$FIXTURE" "$BUTTON_SCRIPT" "$OUT/repro"
```

Observed, pinned by assertions in `repro.py` and summarized in `repro.json`:

| Run | Core1 PC | CONF | CRC result | STATE0 | RTC INT_RAW |
| --- | --- | --- | --- | --- | --- |
| main, 5 s | 0x400081ac (ROM CRC poll) | 0x7ff00100 | 0 | 0 | 0 |
| fix, 2 s | 0x40086b97 (rtc_sleep_start+0x83) | 0x7ff00000 | 0x064fb977 | 0x80000000 | 0 |
| fix, 5 s | 0x40086b97 | 0x7ff00000 | 0x064fb977 | 0x80000000 | 0 |

The 1 s press gets past CRC and sets SLEEP_EN before 2 s. At 0x40086b97 the
firmware reads 0x3ff48040 and loops at 0x40086b94..0x40086b9d while RAW bits[1:0]
remain zero. The 3 s press does not reboot or restore the screen: the chip still
executes that loop at 5 s. Core0 stays at 0x40084790 in its IPC wait. The display
is blank/backlight off. This is evidence of a missing sleep transition/wakeup
path, not evidence of successful sleep or a standalone EXT0-only defect. That
work is outside this CRC change. No fake wake interrupt or firmware workaround.

## Checks

All Cargo commands use Rust 1.99.0, nice priority 10, an isolated target directory.
`checks.json` records exit codes; full logs are retained outside the source tree.

- Native workspace/all-targets Clippy and release wasm32 jit-tests Clippy, both
  with `-D warnings`: pass.
- Empty-HOME CI-policy release workspace tests, only explicit ROM directory
  (plus Cargo/rustup homes): 987 passed, zero failed/ignored.
- Empty-HOME plain release workspace tests without a ROM-directory override:
  959 passed, zero failed, 50 ignored. Existing goldens unchanged.
- Production WASM build plus hello, c3-hello, c6-hello, c6-energy-scan,
  c6-contiki, c6-contiki-net, c6-rpl-net and panel: pass.
- Three-second public classic/S3/C3/C6 hello controls match per-core instruction
  totals, console hashes and exception/IRQ reports against main (`controls.json`).
  Reproduce with `nice -n 10 python3 docs/evidence/classic-rtc-mem-crc/controls.py "$BASE" "$FIX" "$OUT/controls"`.
- Evidence privacy check and manual review: pass. No production data.
- `cargo +1.99.0 fmt --all -- --check`: fails on both base and candidate due to
  repository-wide existing formatting. Base has 4175 diff hunks; candidate 4176
  (inserting the CRC block splits an existing bus-formatting hunk). New test file
  passes `rustfmt +1.99.0 --edition 2021 --check esp32/tests/rtc_crc.rs`.
  No repository-wide reformat is included.
- WASM JIT-specific differential suite not required: no JIT implementation change.

## Static hot-path comparison

```sh
nice -n 10 python3 docs/evidence/classic-rtc-mem-crc/codegen.py "$BASE" "$FIX" "$OUT/codegen"
```

EX230 address/constant-pool normalizer, preserving field offsets and opcodes;
not literal whole-binary identity. `codegen.json` pins both binary hashes and all
106 selected function results. 104 are identical, including selected machine
run/step/IRQ paths, core execution, classic read/fetch/tick/deadline paths and
other chips' selected paths. Two differ:

- ClassicRtc::write: 41 -> 58 instructions, the two hardware-owned registers.
- Classic SocBus::store: 2736 -> 2722 instructions. The MMIO arm adds address/START
  checks and a call; LLVM changes register allocation/spill slots and chooses an
  out-of-line UART route call instead of the previous inline body. Stack frame
  subtraction remains 0xc10 in both. The first 335 normalized instructions are
  identical, before the MMIO-path divergence. The CRC loop is out of line.

No added work in reads/ticks or the common RAM-store path is inferred from the
source and comparison; whole-store identity and CPU parity are NOT claimed.
MMIO writes pay the additional address check even when CRC is unused.

## CPU comparison

PENDING. No CPU timing table was measured in this change. A quiet-machine,
1 warmup + 7 alternating-pair run against main is separate work.
