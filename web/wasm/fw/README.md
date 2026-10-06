# Firmware for the WebAssembly build

Put images here for `run.html?wasm&fw=<name>`, described by `<name>.json`:

```json
{ "board": "waveshare-lcd4b", "flash_mb": 16, "psram_mb": 8, "wifi": "ssid=esp32sim,psk=esp32sim-pass", "stubs": ["esp_wifi_start=0"],
  "files": { "rom": "esp32s3_rev0_rom.elf", "bootloader": "bootloader.bin", "ptable": "partition-table.bin",
             "app": "energy_panel.bin", "elf": ["energy_panel.elf"], "script": "sid.txt" },
  "flash_at": { "0x610000": "energydata.json" } }
```

`kind` names: `rom`, `bootloader`, `ptable`, `app`, `elf` (one or a list), `flash` (whole image),
`script`, `picture`. `flash_at` writes files into flash at hex offsets (a data partition's contents). `terminal: true` opens the page on the xterm.js terminal tab (UART0) instead of the plain console log. `line_hint` is the example shown in the console's line box (default: the board's, JSON actions on the Atech board, otherwise a plain line); the tab title is the demo's name from `demos.json`. `display_rotate` (0, 90, 180 or 270, clockwise) turns the panel view for a firmware that draws rotated into its panel — pocket-tank renders landscape into the portrait AMOLED, so it sets 90; touch is mapped back, and `?rotate=` or the panel's selector overrides it. `stubs` are `NAME[=value]` function stubs — resolved through the ELF, or through a `symbols` map (`{"NAME": "0xaddr"}`) when no ELF is shipped; `wifi` an AP spec. `demos.json` lists the manifests the page offers as links. Everything else in this directory is git-ignored except `public/` (hello_world and the Atech firmware — our own code — plus pocket-tank's bootloader, partition table and app, MIT, with `pocket-tank-LICENSE.txt`; and the WiFi station of `examples/c6-wifi-station`, our own code, built for the S3, C3 and C6 as the WiFi goldens' input — the binaries also contain ESP-IDF, Espressif's WiFi, PHY and coexistence libraries and third-party code such as FreeRTOS, lwIP, Mbed TLS, wpa_supplicant and newlib, with their licences in `wifi-station-NOTICE.txt`) and the manifests: the mask ROM
is Espressif's and firmware is whoever built it — host them only where you may.

For TinyDraw manifests, set `"smoothDisplay": true` to opt in to 120 Hz host
publication without the quiet-pixel deferral on the AMOLED board. The default
is 50 Hz with quiet-pixel deferral, including pocket-tank's full-frame rescans.
This option changes host snapshots, not guest display timing. Unsupported boards
or older WASM modules reject the opt-in before boot. The publication evidence is
recorded in [EX117](../../../docs/experiments.md#ex117).
