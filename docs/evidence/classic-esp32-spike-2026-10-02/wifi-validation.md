# Classic ESP32 Wi-Fi validation, October 2, 2026

This receipt extends [EX199](../../experiments.md#ex199) from revision `4ff7f45` with
classic Wi-Fi station bring-up. Implementation revision: `8722c4c5908013e870a52e08d8a07441bdb4de39`.
The unchanged Arduino-ESP32 3.3.8 sketches scan, join an open AP, obtain a DHCP lease,
resolve a DNS name and fetch a body through the shared user-mode NAT. WPA2 stops in
SHA-1 after receiving handshake message 1. No firmware patches or function stubs are used.
The S3 and C6 adapters and their plans were inspected first. This extension changes the
classic MAC, DMA and PHY correctness contract, not an execution-speed experiment.

## Firmware inputs
Three validation projects were built outside the repository using unchanged Arduino
APIs and the installed framework libraries. No firmware or Espressif blob patches were
applied. All builds succeeded on their first invocation with access to the existing
PlatformIO cache. Runtime results are recorded below.

Tool versions reported by the build were PlatformIO Core 6.1.19,
`platform-espressif32` `55.3.38+sha.fbdfc29`, Arduino-ESP32 3.3.8,
`framework-arduinoespressif32-libs` `5.5.4+sha.735507283d`,
`toolchain-xtensa-esp-elf` `14.2.0+20260121`, and esptool 5.2.0.
The projects target `esp32dev`, 240 MHz CPU and 4 MiB flash.

Each project's `platformio.ini` was:
```ini
[env:esp32dev]
platform = https://github.com/pioarduino/platform-espressif32.git#55.03.38-1
board = esp32dev
framework = arduino
monitor_speed = 115200
```
Scan, `/tmp/esp32sim-classic-wifi-validation/scan/src/main.cpp`:
```cpp
#include <Arduino.h>
#include <WiFi.h>

void setup() {
  Serial.begin(115200);
  Serial.printf("WIFI_SCAN_BEGIN millis=%lu\n", millis());
  WiFi.mode(WIFI_STA);
  int count = WiFi.scanNetworks();
  Serial.printf("WIFI_SCAN_COUNT %d millis=%lu\n", count, millis());
  for (int i = 0; i < count; ++i) {
    Serial.printf("WIFI_SCAN_AP ssid=%s channel=%d rssi=%d auth=%d\n",
                  WiFi.SSID(i).c_str(), WiFi.channel(i), WiFi.RSSI(i),
                  WiFi.encryptionType(i));
  }
  Serial.println("WIFI_SCAN_DONE");
}

void loop() { delay(1000); }
```
Open network, DNS and HTTP, `/tmp/esp32sim-classic-wifi-validation/open/src/main.cpp`:
```cpp
#include <Arduino.h>
#include <WiFi.h>
#include <HTTPClient.h>

void setup() {
  Serial.begin(115200);
  Serial.printf("WIFI_OPEN_BEGIN millis=%lu\n", millis());
  WiFi.begin("esp32sim");
  while (WiFi.status() != WL_CONNECTED && millis() < 30000) delay(100);
  Serial.printf("WIFI_OPEN_STATUS %d millis=%lu ip=%s\n", WiFi.status(),
                millis(), WiFi.localIP().toString().c_str());
  if (WiFi.status() != WL_CONNECTED) return;
  Serial.println("WIFI_TARGETS_READY");
  while (!Serial.available()) delay(10);
  String name = Serial.readStringUntil('\n');
  name.trim();
  IPAddress address;
  int resolved = WiFi.hostByName(name.c_str(), address);
  Serial.printf("WIFI_DNS result=%d ip=%s\n", resolved, address.toString().c_str());
  while (!Serial.available()) delay(10);
  String url = Serial.readStringUntil('\n');
  url.trim();
  HTTPClient http;
  if (!http.begin(url)) {
    Serial.println("WIFI_HTTP_BEGIN_FAIL");
    return;
  }
  int status = http.GET();
  Serial.printf("WIFI_HTTP_STATUS %d\n", status);
  Serial.printf("WIFI_HTTP_BODY %s\n", http.getString().c_str());
  http.end();
  Serial.println("WIFI_OPEN_DONE");
}

void loop() { delay(1000); }
```
WPA2-PSK, `/tmp/esp32sim-classic-wifi-validation/wpa2/src/main.cpp`:
```cpp
#include <Arduino.h>
#include <WiFi.h>

void setup() {
  Serial.begin(115200);
  Serial.printf("WIFI_WPA2_BEGIN millis=%lu\n", millis());
  WiFi.begin("esp32sim", "classic-wifi-pass");
  while (WiFi.status() != WL_CONNECTED && millis() < 30000) delay(100);
  Serial.printf("WIFI_WPA2_STATUS %d millis=%lu ip=%s\n", WiFi.status(),
                millis(), WiFi.localIP().toString().c_str());
  Serial.println("WIFI_WPA2_DONE");
}

void loop() { delay(1000); }
```
Build commands:

```sh
pio run -d /tmp/esp32sim-classic-wifi-validation/scan
pio run -d /tmp/esp32sim-classic-wifi-validation/open
pio run -d /tmp/esp32sim-classic-wifi-validation/wpa2
```

Build output reported success in 9.80 seconds for scan, 8.81 seconds for open, and
9.04 seconds for WPA2.
These are build durations, not emulator performance samples. The factory images are
compact merged binaries, with the normal bootloader at `0x1000`, partition table at
`0x8000`, boot-app selection at `0xe000`, and application at `0x10000`.

SHA-256 inputs and artifacts, relative to `/tmp/esp32sim-classic-wifi-validation/`:

| Item | SHA-256 |
| --- | --- |
| `scan/platformio.ini` | `b86c69259be417474b2dfef705f80db5720beff2f80b471ef5762f2a65651500` |
| `scan/src/main.cpp` | `09a68535879cda7db5a36fe4850e88b6fde0e7a0b1c8ff78a68ead9b02bac5ff` |
| `scan/.pio/build/esp32dev/firmware.elf` | `8c9233db3855dcd2c9d07e145404dff83a5580cf0360cb8108f6f5e61604a655` |
| `scan/.pio/build/esp32dev/firmware.factory.bin` | `26204a829cd9243522e5bac3112ce3a9ffa0c546c90babee514c40d2ac8d25dc` |
| `open/platformio.ini` | `b86c69259be417474b2dfef705f80db5720beff2f80b471ef5762f2a65651500` |
| `open/src/main.cpp` | `636cc77bed923832a31c8afbee2c25f6d507e12f1933267abcceb31a73668464` |
| `open/.pio/build/esp32dev/firmware.elf` | `ef8579ba1dce678e9ec324882790fd4cff83cbb8a22cece945b5062c90b6abe7` |
| `open/.pio/build/esp32dev/firmware.factory.bin` | `8e3d39a82026cb4cce8b0a9433acd4f89ef97e7bb3dd946d0ebf89e50206cd59` |
| `wpa2/platformio.ini` | `b86c69259be417474b2dfef705f80db5720beff2f80b471ef5762f2a65651500` |
| `wpa2/src/main.cpp` | `633542a9ea1954ca58f3c22c5b1a4d92b3f36e11a088806cf3b9a5c6a2ec1461` |
| `wpa2/.pio/build/esp32dev/firmware.elf` | `cef8cc4e9ac2b0a050c90089481431e95bf8b6b9b077980ba494fdfe79218b70` |
| `wpa2/.pio/build/esp32dev/firmware.factory.bin` | `c575e1dad68e12ee09a217fe4bb3c4f73ee9114038af50742fe28855280017e1` |

## MAC register contract

Offsets are relative to `MAC = 0x3ff73000` and `WDEV = 0x3ff75000`.
The ROM also uses the corresponding AHB addresses `0x60033000` and `0x60035000`.

| Register | Classic blob evidence | Meaning / implemented contract |
| --- | --- | --- |
| MAC `+0xd24` | `hal_init+0x15` polls bit 0 after setting bit 1 | Core-ready handshake. Differs from S3 `+0xd14`. |
| MAC `+0xc00` | `hal_now+0x30`; `lmacProcessRxSucData` | Running MAC microsecond counter. Firmware adds `g_wifi_mac_time_delta`. |
| MAC `+0xc48`, `+0xc4c` | `hal_mac_interrupt_get_event`, `hal_mac_interrupt_clr_event` | Read / write-one-to-clear events. Differs from S3 `+0xc3c` / `+0xc40`. |
| MAC `+0xc40`, `+0xc44` | `hal_init`, `hal_enable_mac`, `hal_disable_mac` | Interrupt enable masks. Init writes `0x01e839e0` to `+0xc40`; disable writes both zero. |
| MAC `+0x088`, `+0x08c`, `+0x090` | `hal_mac_rx_set_base`, `hal_mac_rx_read_rxdscrnext`, `hal_mac_rx_get_last_dscr` | Base / next / last RX descriptor. |
| MAC `+0x084` bit 0 | `hal_mac_rx_set_dscr_reload`, `hal_mac_rx_is_dscr_reload` | Self-clearing descriptor-reload request. |
| MAC `+0x084` bit 31 | `hal_mac_rx_enable`, `hal_mac_rx_disable` | RX enable. |
| MAC `+0xd20 - 8q` | `hal_mac_txq_enable`: `(0x07fee7a4 - q) << 3` | Queue descriptor low 20 bits. Bits 31:30 start/validate TX. Differs from S3 `+0xd08 - 8q`. |
| MAC `+0xcc0`, `+0xcbc` | `hal_mac_get_txq_state` cases 0/1, `hal_mac_clr_txq_state` | TX error/collision bitmaps read / clear. Two 11-bit fields. |
| MAC `+0xcc8`, `+0xcc4` | `hal_mac_get_txq_state` case 2, `hal_mac_clr_txq_state` | Completed-queue bitmap read / clear. Blob extracts bits 3:0. |
| MAC `+0x1270 - 60q` | `hal_mac_get_txq_pmd` | TX result word. Success is zero status. This is in block `0x74`, not the S3 WDEV result area. |
| WDEV `+0x010` bits 0/1/2 | `hal_get_tsf_time`, `hal_mac_tsf_get_time` | Latch base / station / secondary TSF counters. |
| WDEV `+0x014/+0x018`, `+0x054/+0x058`, `+0x090/+0x094` | same accessors | Latched low/high words for the three counters. |
| WDEV `+0x024/+0x028`, control bit 6 | `hal_mac_tsf_reset` mode 1 | Load base TSF counter. |
| WDEV `+0x060/+0x064`, control bit 7 | `hal_mac_tsf_set_time` mode 1; `hal_mac_tsf_reset` | Load station TSF counter. |
| WDEV `+0x09c/+0x0a0`, control bit 8 | `hal_mac_tsf_set_time` mode 2 | Load secondary TSF counter. |

`wDev_ProcessFiq+0x3c` tests event mask `0x01000024` for RX, so RX event bit 24
is sufficient. `wDev_ProcessFiq+0x173` tests bit 7 and calls `lmacPostTxComplete`.
Those event bits were not assumed from the S3.

The installed classic `soc/interrupts.h` names source 0 `ETS_WIFI_MAC_INTR_SOURCE`,
source 1 `ETS_WIFI_MAC_NMI_SOURCE`, and source 2 `ETS_WIFI_BB_INTR_SOURCE`.
There is no separately named Wi-Fi power interrupt. No classic `hal_pwr` implementation
was found in this `libpp.a`; its `hal_pwr.o` contains no symbols. A power-event register map
has not been inferred from the S3's `WDEV+0x118/+0x11c`.

## RX layout and pointer semantics

The classic receive control header is 28 bytes, not S3's 48. Both the installed public
`wifi_pkt_rx_ctrl_t` and the blob's consumer agree:

| RX buffer location | Blob access / meaning |
| --- | --- |
| byte 0 | Raw RSSI; `wDev_ProcessRxSucData+0x13` subtracts `wDevCtrl+46`, observed as 96. Raw 56 gives -40 dBm. |
| word 0 bits 28..30 | `wDev_ProcessRxSucData+0x15a` requires one accepted-frame flag. |
| byte 8 | Noise floor; overwritten by the blob. |
| byte 10 | Primary/secondary channel; overwritten by the blob. |
| bytes 12..15 | Microsecond timestamp from the public classic header. |
| word at `+24`, bits 11:0 | Frame length including FCS. |
| byte 27 | Receive state, read at `wDev_ProcessRxSucData+0x87`. Zero accepts a normal frame. |
| `+28` | Start of the 802.11 frame when there is no CSI dump; `wDev_ProcessRxSucData+0xda`. |

The descriptor is three 32-bit words. Control fields are size bits 11:0, length bits
23:12, has-data bit 30 and owner bit 31, followed by full buffer and next-descriptor
pointers. `wDev_AppendRxBlocks` restores owner, clears has-data and bit 29, and resets
length to size. `wdevProcessRxSucDataAll` checks has-data when walking completed RX.
The model leaves owner set when it marks has-data. That matches the shared-family model
and is accepted by these consumers; classic silicon ownership was not measured.

Unlike the S3, the classic RX address registers return full native addresses.
`wdev_is_data_in_rxlist` compares the last-register value directly with its full descriptor
pointer. `wDev_AppendRxBlocks+0x111` compares next with empty sentinel `0x3ff00000` and
then dereferences last `+8` without restoring any high bits. No S3-style bit-24 prefix is
valid here. Internally the model must distinguish the empty pointer from a real descriptor.

## Radio register contract

These are functional calibration completion contracts recovered from Arduino-ESP32 3.3.8's classic `libphy.a` and the ECO3 ROM. They do not model RF measurements or calibration arithmetic. The S3 implementation supplied a starting point, but its register offsets and broad analog done-bit mask do not match this blob.

Inputs:

| Input | SHA-256 |
| --- | --- |
| `framework-arduinoespressif32-libs/esp32/ld/libphy.a` | `c2521071695734afa8f6f1ca3d3b604a1d08dd63fe52060449a11cfe8bac5740` |
| `framework-arduinoespressif32-libs/esp32/lib/libpp.a` | `538ce39f092887ec8640d14d0d69ef4803d86b9cf83c685c4eb8c0326a99764c` |
| `framework-arduinoespressif32-libs/esp32/lib/libnet80211.a` | `39c06f8aaf66cc798a85cec3f8bbcc3db7e607f6f597c617a7307f641ea8578b` |
| `framework-arduinoespressif32-libs/esp32/lib/libcore.a` | `1de00e97e876d47180322ba0f802fc6aec40019b093d16fcdfead87859bb3d53` |
| `framework-arduinoespressif32-libs/esp32/ld/librtc.a` | `e3411aeeb48d1da8acd304808290196baa0d6a2397db1a0c7741abee85dade61` |
| `framework-arduinoespressif32-libs/esp32/lib/libcoexist.a` | `d86f9352f4b21f59bfb8bafccb962eae1c074451be1e64525b2a81729a6d61b4` |
| `tool-esp-rom-elfs/esp32_rev300_rom.elf` | `920b70635440517866aab2230964a570d2cf2b676658d93c52fbac108c1cca31` |

Disassembly commands, with caller-supplied input paths:

```sh
"$OBJDUMP" -dr "$LIBPHY" > classic-libphy-disasm.txt
"$OBJDUMP" -dr "$LIBPP" > classic-libpp-disasm.txt
"$OBJDUMP" -d "$ROM" > classic-rom-disasm.txt
```

| Register and contract | Blob evidence |
| --- | --- |
| `0x3ff4e000 + 4*host`, hosts 0 through 4. Slave bits 7:0, register 15:8, data 23:16, write 24, busy 25. Reads return stored analog bytes and clear busy. | `ram_chip_i2c_readReg` and `ram_chip_i2c_writeReg`; `i2c_master_reset` advances the register by 4 until its counter equals 5. ROM `rom_chip_i2c_readReg` at `0x40004110` and `rom_chip_i2c_writeReg` at `0x40004168` use the corresponding `0x6000e000 + 4*host` alias. |
| Analog host 1, slave `0x62`, register 7, bit 7 reads done. | `ram_wait_rfpll_cal_end` and ROM `rom_wait_rfpll_cal_end` at `0x400047a8` repeatedly read this bit, with 20 us delays and a 100-iteration timeout. |
| `0x3ff4e04c`, bit 24 done, comparator signs 31:30 zero. | `ram_txdc_cal_v70` tests bit 24 at object section offset `0xe5`; ROM `rom_txdc_cal_v70` tests the same bit at `0x40004f3e` through AHB alias `0x6000e04c`. |
| `0x3ff4e050`, bits 26:24 clear. | `ram_get_fm_sar_dout` extracts three bits at offset `0x52` and repeats at `0x55` while nonzero. ROM `rom_get_fm_sar_dout` at `0x40005204` has the same contract. OR-ing S3's generic `7 << 24` done mask here would deadlock classic calibration. |
| `0x3ff4607c`, bit 31 done after bits 0 and 1 start IQ estimation. | `ram_iq_est_enable` writes the start bits and loops on a nonnegative read at section offset `0xca`. ROM `rom_iq_est_enable` uses AHB alias `0x6000607c` and polls at `0x40005589`. |
| `0x3ff4e168`, bit 31 clear, channel update idle. | `set_channel_rfpll_freq` reads it at section offset `0xd4` and loops on a negative result at `0xdc`. |
| `0x3ff4e0c4`, bit 8 clears when a software channel request completes. | `set_chan_freq_sw_start` writes `value | 0x100` at section offsets `0x144..0x14a`; when bit 31 is clear, it loops while bit 8 remains set at `0x198`. `wr_rf_freq_mem` uses bit 9 as a software-controlled strobe and clears it itself. |
| `0x3ff73c00` and its `0x60033c00` alias return a running microsecond counter. | `i2c_master_reset` and `phy_force_wifi_chan` subtract consecutive reads for timeout checks. `register_chipv7_phy` reads the AHB alias at function entry. |

## Runtime commands

The full classic peripheral mirror, `0x60000000..0x6003ffff` to
`0x3ff40000..0x3ff7ffff`, is documented in the
[ESP32 TRM, section 3.3.5](https://documentation.espressif.com/esp32_technical_reference_manual_en.pdf#page=73).
The ROM and coexistence code use this mirror. The existing UART and I2C FIFO handling
remains ahead of normalization. The MAC uses three independent register banks.

Each invocation below runs from the repository root. `--break` stops at the unchanged
sketch's `loop()` entry. A breakpoint is not a pass: the serial result must also match.
The WPA2 invocation instead stops when the unsupported SHA-1 path aborts.

```sh
target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-wifi-validation/scan/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-wifi-validation/scan/.pio/build/esp32dev/firmware.elf \
  --wifi ssid=esp32sim --break 0x400d6384 --max-seconds 8 --no-reboot --no-dump \
  --regstat /tmp/classic-wifi-final-scan-regstat.txt
```

The HTTP fixture serves one static body. `WIFI_HTTP_HOST` is a caller-supplied address
of the host reachable through its network interface. Guest `127.0.0.1` is the guest's
own loopback, and `10.0.2.2` is only the virtual gateway. Neither denotes the host server.
The DNS name used in this run was `example.com`. Its returned public address can change.

```sh
mkdir -p /tmp/esp32sim-classic-wifi-validation/http
printf 'CLASSIC_WIFI_NAT_OK\n' > /tmp/esp32sim-classic-wifi-validation/http/body.txt
python3 -m http.server 18732 --bind "$WIFI_HTTP_HOST" \
  --directory /tmp/esp32sim-classic-wifi-validation/http
```

In a second shell, supply the same host address and the DNS name, then run:

```sh
curl --noproxy '*' --fail --silent --show-error \
  "http://$WIFI_HTTP_HOST:18732/body.txt"
printf '5.0 serial %s\n5.1 serial http://%s:18732/body.txt\n' \
  "$WIFI_DNS_NAME" "$WIFI_HTTP_HOST" > /tmp/classic-wifi-open.script

target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-wifi-validation/open/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-wifi-validation/open/.pio/build/esp32dev/firmware.elf \
  --wifi ssid=esp32sim --net nat --script /tmp/classic-wifi-open.script \
  --break 0x400db298 --max-seconds 12 --no-reboot --no-dump --debug net \
  --trace-fn _ZN16NetworkInterface10_onIpEvent --trace-fn _ZN5Print6printfEPKcz \
  --regstat /tmp/classic-wifi-final-open-regstat.txt

target/release/esp32sim --chip esp32 --boot rom \
  --rom "$HOME/.platformio/packages/tool-esp-rom-elfs/esp32_rev300_rom.elf" \
  --flash-image /tmp/esp32sim-classic-wifi-validation/wpa2/.pio/build/esp32dev/firmware.factory.bin \
  --elf /tmp/esp32sim-classic-wifi-validation/wpa2/.pio/build/esp32dev/firmware.elf \
  --wifi ssid=esp32sim,psk=classic-wifi-pass --net nat \
  --break 0x400d61e0 --max-seconds 4 --no-reboot --no-dump --debug wifi-frames \
  --trace-fn esp_aes --trace-fn aes_hal --trace-fn wpa_sm_rx_eapol \
  --trace-fn sha_hal_read_digest --regstat /tmp/classic-wifi-final-wpa2-regstat.txt
```

The server body SHA-256 is
`622a08950131b4f24eca42356ed2e9d994c8734360dc25ab806d378af63805d2`.
No execution stubs, register pokes, fake-read environment overrides, firmware patches,
or shared AP/network/NAT changes are used. The CLI default JIT setting is unchanged.

## Runtime results

Final run order was scan, open/DNS/HTTP, then WPA2, after the builds and tests finished.
Each run booted a fresh factory image through the ECO3 ROM. The fixed AP used channel 6.
The passphrase is a synthetic test value, not a real network credential.

```text
WIFI_SCAN_BEGIN millis=3
WIFI_SCAN_COUNT 1 millis=2415
WIFI_SCAN_AP ssid=esp32sim channel=6 rssi=-40 auth=0
WIFI_SCAN_DONE
WIFI_OPEN_BEGIN millis=3
WIFI_OPEN_STATUS 3 millis=1739 ip=10.0.2.15
WIFI_TARGETS_READY
WIFI_DNS result=1 ip=104.20.23.154
WIFI_HTTP_STATUS 200
WIFI_HTTP_BODY CLASSIC_WIFI_NAT_OK
WIFI_OPEN_DONE
WIFI_WPA2_BEGIN millis=3
abort() was called at PC 0x4014f13e on core 0
```

| Workload | Stop, modeled seconds | Whole process, wall seconds | Core-0 + core-1 instructions | Modeled cycles |
| --- | ---: | ---: | --- | ---: |
| scan | 2.458 | 1.278887 | 22346406 + 4061436 | 589901112 |
| open | 5.116 | 1.644418 | 34002926 + 10055161 | 1227726408 |
| wpa2 | 0.865 | 1.774495 | 56273515 + 3162436 | 207500678 |

The open sketch first observed `WL_CONNECTED` at **1.7882 modeled seconds** from reset
and printed status 3 at **0.975265 wall seconds** from process launch. Its own Arduino
clock read 1739 ms. The earlier IP-event callback ran at 1.6993 modeled seconds and was
observed at 0.958211 wall seconds. Scan returned exactly one AP, with synthetic RSSI -40.
HTTP returned the complete expected body, and the host fixture recorded a GET for
`/body.txt` with status 200. WPA2 never reached a connected status.

Wall measurements use Python `time.monotonic()` immediately before `subprocess.Popen`
and when a complete merged stdout/stderr line arrives. They include process startup,
ROM/ELF loading, tracing and host scheduling. The connection observation is the first
`WIFI_OPEN_STATUS 3` line, sampled by the sketch every 100 Arduino milliseconds.
The `Print::printf` trace immediately before that line supplies its modeled time.
The IP-event callback timestamp is recorded separately. Final CLI wall time covers the
entire run and has only 0.1-second display precision. These are individual functional
runs on the same macOS arm64 host, not performance comparisons or physical radio timing.
No machine identity or process inventory was collected for this claim.
The final runs were sequential, after compilation and test execution had finished.

## WPA2 boundary

The AP sent EAPOL-Key message 1 after authentication and association. `wpa_sm_rx_eapol`
received its 99-byte EAPOL payload at 0.8558 modeled seconds, observed at 1.699236 wall
seconds. The subsequent trace repeatedly entered `sha_hal_read_digest` with `a2=0`,
the SHA-1 mode. The final entry was at 0.8561 modeled seconds. The guest called `abort()`
at `0x4014f13e` because all five digest words were zero, then requested a software reset
at 0.865 modeled seconds. The complete process took 1.774495 wall seconds.

The symbolized call path is `wpa_sm_rx_eapol` → `wpa_supplicant_process_1_of_4` →
`wpa_supplicant_send_2_of_4` → `wpa_eapol_key_send` →
`wpa_eapol_key_mic` → `hmac_vector` → `mbedtls_sha1_finish` →
`esp_sha_read_digest_state` → `sha_hal_read_digest`. Disassembly at `0x4014f138`
through `0x4014f17f` checks whether any digest word is nonzero and aborts otherwise.
Register evidence shows SHA-1 start `0x3ff03080`, continue `0x3ff03084`, digest load
`0x3ff03088` and idle `0x3ff0308c`; the final digest window is `0x3ff03000..0x3ff03010`.
The heat report contains 39 digest-load writes and zero-valued digest reads through
`esp_dport_access_sequence_reg_read`. This is an accelerator correctness failure,
not an AES polling stall at handshake message 3.

The classic SHA model at the starting revision implements SHA-256 for boot verification.
It returns idle for SHA-1 but does not execute its start, continue or digest-load commands.
This lane leaves that model unchanged. No AES call was reached before the abort, and no
message 2 was transmitted. WPA2 cannot be described as connected or crypto-validated.

## Negative results and corrections

- The baseline aborted in `esp_phy_load_cal_and_init` because the factory MAC's eFuse CRC
  was zero. `esp_efuse_mac_get_default` calculated `0xcf` for the configured synthetic
  address. The local eFuse adapter now supplies the ROM's CRC-8 polynomial `0x8c` in bits
  23:16 of register `0x3ff5a008`; a register test checks the resulting `0x00cf246f`.
- Mapping only APB MAC registers caused `LoadProhibited` at `0x60033c00` during
  `register_chipv7_phy`, followed by reset at 0.064 modeled seconds. A diagnostic run
  launched before its rebuild finished repeated that same failure; it was not a new
  model result. Later accesses failed at `0x600310d0` in `coex_bt_high_prio` and at GPIO
  alias `0x600041c4`. The documented complete AHB mirror fixes the common cause.
- The first analog implementation reached `register_chipv7_phy_init_param` but polled
  `0x3ff4e0c4` 8,764,467 times with value `0x53002d18`. The 0.500-second run retired
  119,408,612 core-0 and 1,793,183 core-1 instructions in 5.3 displayed wall seconds.
  `set_chan_freq_sw_start` waits for bit 8 to clear. Returning `0x53002c18` after its
  request lets calibration continue; subsequent function traces reached repeated
  channel updates between 0.0606 and 0.0777 modeled seconds.
- Review caught a single `RegRam` aliasing all three MAC/WDEV banks because `RegRam`
  masks offsets to 4 KiB. Separate banks and a same-offset isolation test fixed it.
- The initial scan printed RSSI 120: the S3's already signed `0xd8` sample was wrong
  for classic. `wDev_ProcessRxSucData` subtracts `wDevCtrl+46`, observed as 96.
  The classic raw sample is now 56, giving the intended synthetic -40 dBm.
- The first HTTP target was guest loopback and returned `-1`. A later run returned
  `-5`; a diagnostic retry found the temporary host fixture unavailable and logged
  `Connection refused (os error 61)`. Restarting the fixture, checking it with curl,
  and using a caller-supplied host interface address produced the successful NAT run.
  The `-5` observation alone does not establish why that connection was lost.
- Development builds briefly failed with unresolved `Analog`/`FrontEnd`, then private
  `dma_read_word`/`dma_write_word` methods, and finally `Dport::read_reg` after the bank
  edit. Each received one local correction and its next build passed. No shared-chip
  workaround or toolchain change was needed.

## Checks and limits

All required commands passed on implementation revision `8722c4c`:

```text
cargo build --release                       PASS
cargo test -p esp32 -p esp32sim              PASS, 60 passed / 15 ignored
cargo test --workspace                      PASS, 494 passed / 22 ignored
tools/wasm-build.sh                         PASS
node tools/check-evidence-privacy.mjs       PASS, 1,473 files / 15 gzip files
git diff --check                           PASS
```

The ESP32 crate has 31 unit tests. The aggregate counts above include integration tests
and doctests; external-firmware tests retain their existing ignored status.
Only `rustfmt --edition 2021 esp32/src/wifi.rs` was run, and the before/after status
contained the same set of changed files. No workspace or crate formatting was applied.
The local commit initially failed with `Unable to create .../index.lock: Operation not
permitted`; one retry with access to this worktree's Git metadata succeeded.

Build configuration: Darwin 25.6.0 arm64, Rust and Cargo 1.96.0, release profile,
240 MHz modeled CPU. The native executable and WASM artifact hashes are recorded below.

Register tests cover MAC bank isolation, ready and completion events, interrupt masks,
TX queue completion, native RX pointers and reload, TSF load/latch and the MAC clock,
RX metadata and FCS, descriptor bounds, analog host isolation and PHY completion bits,
DPORT clock/reset/source-0 routing, the AHB mirror and factory-MAC CRC.

The model reuses `esp-soc`'s AP, WPA2 protocol implementation, DHCP, DNS, SNTP and NAT.
Those shared files and the S3, C3 and C6 models are unchanged. Only the native CLI's
classic setup gains `--wifi`, `--net` and `--regstat` support. The WASM build passes,
but a classic browser Wi-Fi session was not exercised.

This is one station on one virtual AP, with immediate successful TX completion,
one descriptor per transmitted frame and one outstanding RX frame. RX uses the existing
S3 pacing rule of at least 400 modeled microseconds and a 50 ms recycle limit. The model
does not synthesize RF waveforms, calibration arithmetic, channel filtering, interference,
collisions, chained TX packets, power-save/beacon deadline events or physical throughput.
RSSI is a fixed synthetic value. NMI and BB interrupt behavior are not exercised.
No real classic ESP32 radio capture was used as an oracle.

## Evidence curation

This receipt retains source and firmware revisions, input hashes, reproduction commands,
selected serial output, modeled and wall observations, the register contract and failures.
Raw disassembly, register heat files, build logs and complete frame traces remain under
`/tmp` and are not added to Git. Private host-interface and resolver addresses, local
process identifiers and home-directory labels are omitted. Commands accept caller-supplied
network targets. This prevents reconstructing the particular host network, but does not
remove measured guest values, artifact hashes or the HTTP body check.

| Built artifact | SHA-256 |
| --- | --- |
| `target/release/esp32sim` | `6410637fc6b987b2d0b73c91e7b186868df69978c2003d43baf6cfb83874257d` |
| `web/wasm/esp32sim.wasm` | `fd9fe1c8b8afb77f937745f4a5a5ebf58a5da1e4cc3f525287ce6c80311c6972` |

## Combined-branch WPA2 result

After rebasing onto the AES, SHA and RSA extension, the unchanged WPA2 sketch above, run with
`--wifi ssid=esp32sim,psk=classic-wifi-pass --net nat`, prints
`WIFI_WPA2_STATUS 3 millis=1839 ip=10.0.2.15`. The SHA-1 blocker recorded above is resolved by the
crypto extension; the Wi-Fi adapter is unchanged.
