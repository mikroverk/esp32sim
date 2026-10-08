# ProbeB comparison

C3 rev v0.3, Arduino-ESP32 3.3.11 / ESP-IDF 5.5.5, unchanged ProbeB images.
`inputs.json` identifies the sketch, images and original serial captures.
Raw captures remain locally with their provider; no public download is claimed.
They contain device identifiers and are not committed. The JSON summaries omit
addresses, packet bytes and serial text. This prevents independent recovery of
omitted fields from the summaries; rerunning the read-only sketch reproduces them.

To rebuild with Arduino-ESP32 3.3.11, use an output directory inside the worktree:

```sh
mkdir -p "$WORK/ProbeB" "$WORK/scratch"
cp ProbeB.ino "$WORK/ProbeB/ProbeB.ino"
TMPDIR="$WORK/scratch" arduino-cli compile --fqbn esp32:esp32:esp32c3 \
  --build-path "$WORK/build" "$WORK/ProbeB"
```

Flash the bootloader at 0, partitions at 0x8000, the core's `boot_app0.bin` at
0xe000 and application at 0x10000, with 4 MB flash. The caller supplies the serial
port. Capture at 115200 baud. The board was reset before each scenario. A macOS central performed:

- Normal: connect/read, hold three seconds, LL_TERMINATE_IND disconnect, wait
  three seconds, reconnect/read, hold three seconds, normal disconnect.
- Timeout: connect/read, hold two seconds, central radio off for ten seconds,
  radio on, wait five seconds, reconnect/read, hold two seconds, normal disconnect.

Both hardware captures contain two host reads of `Hello World says Neil` and
five complete advertising/connected/advertising/connected/advertising snapshots.
The emulator runs the same ProbeB images. Its commands mirror those holds and
reconnect delays. Central-selected PHY, interval, hop and CSA differ; this is a
scenario comparison, not replay of the hardware central's RF packets.

```sh
python3 check_compare.py
python3 run.py "$C3_BINARY" "$ROM_ELF" "$PROBE_BUILD" "$OUTPUT_DIR" > scenarios.json
python3 compare.py "$HARDWARE_NORMAL" "$OUTPUT_DIR/emulator-normal.log" > normal.json
python3 compare.py "$HARDWARE_TIMEOUT" "$OUTPUT_DIR/emulator-timeout.log" > timeout.json
```

`compare.py` reconstructs records by PROBE markers, removing only HOST annotation
lines and serial line breaks. It rejects missing fields, malformed hex, wrong
payload lengths, incomplete snapshots and broken RX links. `check_compare.py`
includes HOST insertions inside a key and a hex byte, absent `data`, truncated
hex, arbitrary read fragmentation, and a static CS mutation. These retain the
original completeness checks and strengthen them; malformed records are not skipped.

The original JSON files retain the historical probe-b comparison. No new hardware
run was performed. `current-normal.json`, `current-timeout.json` and
`current-scenarios.json` compare this port's emulator with those same hardware
captures. All available image/capture hashes matched `inputs.json` before use.
Both current scenarios read twice, resume advertising twice and have zero guest
errors. Disconnect reasons are [19, 19] for normal and [8, 19] for timeout.
The commands above reproduce the current run with caller-supplied inputs;
raw emulator output stays in an ignored scratch directory.

## Results and limits

- Matched: format3/activity, fixed control bits, zero/reserved fields selected
  by the declared comparison masks, all-37-channel map, duplicated hop fields,
  ten linked 20-byte RX slots, and return to advertising after both disconnects.
  RX ownership bit15 is observed both clear and set, sometimes while invalid
  status bit15 remains set. The snapshot does not establish transition order.
- Fixed: serial framing in the comparator; added `disconnect` to the virtual
  central so normal LL termination can be compared separately from silence.
  No static register/descriptor correction was justified by these captures.
- Configuration differences: hardware CS+4 is 0x1105, emulator 0x1100.
  `r_lld_con_start` 0x4001b894..b922 packs the two PHY selectors into bits0..3.
  Hardware uses rate1/2M and CSA#2; the virtual peer requests rate0/1M and CSA#1.
  Hardware hop values are 15/9 in the normal run and 15/7 in the timeout run;
  emulator hop is 5. The comparator checks these explicitly, rather than claiming
  equality. `r_lld_con_start` 0x4001ba30..bb8c writes the channel fields.
- Asynchronous differences: CS+0 bits9:10 come from `rwip_coex_cfg` in
  `r_lld_con_start` 0x4001b7f8..b872. CS+26 windows, +28 TX pointers, +80 event
  counts and RX ownership vary during serial sampling. CS+30/+32 are 43 versus
  44; `r_lld_con_evt_time_update` 0x40019456..c6 computes these event durations.
  Their timing/PHY-dependent values are not copied into the model.
- Unproven live state: CS+24 high byte is 0x50/0x60 on hardware and zero in the
  model; CS+86 is 0x0100/0x0201 versus zero. `r_lld_con_frm_isr`
  0x4001b2f2..b376 reads +86's high byte and +24 bit14 with `g_event_empty`.
  `r_lld_con_set_tx_power` 0x4001910a preserves +24's high byte while setting
  the low power index, which matches at 0x0b. These live status fields remain
  unmodeled; the captures do not identify their per-packet update rules.

Stale format3/format4 structures remain reachable through old event entries
between stages on both targets. No blanket descriptor clearing on disconnect
is warranted. The second read succeeds on both targets. Emulator acceptance
also checks identical ADV_IND/SCAN_RSP bytes across disconnects; CoreBluetooth
provided no raw advertising bytes, so hardware byte equality is unproven.
CRC-error bits, sync timestamp placement, exact IRQ/ownership timing, CSA#2
execution and the empty-first hardware scenario remain unvalidated. The model
still supports one unencrypted 1M CSA#1 connection.
