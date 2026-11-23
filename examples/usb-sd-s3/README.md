# USB-OTG and SDMMC on the ESP32-S3 (stock IDF examples)

Host scripts and build notes for three unchanged ESP-IDF v5.5.4 examples, built with
`idf.py set-target esp32s3 build`, that exercise the S3's full-speed USB-OTG core and its SDMMC
host. The images are not in this repository; the `external_s3_*` tests in `cli/tests/goldens.rs`
run them from a build directory you point them at.

| Example | Test | Environment | What it covers |
| --- | --- | --- | --- |
| `examples/storage/sd_card/sdmmc` | `external_s3_sdmmc` | `S3_SDMMC_DIR` | the SDMMC host on slot 1 through the GPIO matrix (CLK 36, CMD 35, D0-D3 37, 38, 33, 34), four lines, 20 MHz; FatFs on a FAT32 card image |
| `examples/peripherals/usb/device/tusb_serial_device` | `external_s3_usb_cdc` | `S3_TUSB_SERIAL_DIR` | esp_tinyusb 2.4.0 / TinyUSB 0.21.0~6 (the component manager's pick, in buffer DMA mode), the internal PHY moved to USB-OTG, enumeration at full speed, CDC-ACM echo (`cdc.txt`) |
| `examples/peripherals/usb/device/tusb_msc` with `CONFIG_EXAMPLE_STORAGE_MEDIA_SDMMC=y` in `sdkconfig.defaults` | `external_s3_usb_msc_sd` | `S3_TUSB_MSC_SD_DIR` | both at once: the card exposed over USB mass storage, a FAT32 file written and read back by the host (`msc-sd.txt`) |

Each variable names the example's project directory (the one holding `build/`); the tests need
the ESP32-S3 mask ROM ELF as the goldens do:

    S3_SDMMC_DIR=... S3_TUSB_SERIAL_DIR=... S3_TUSB_MSC_SD_DIR=... \
      cargo test --release -p esp32sim --test goldens external_s3_ -- --include-ignored

By hand, from a built project:

    esp32sim --boot rom --flash-mb 4 --bootloader build/bootloader/bootloader.bin \
      --ptable build/partition_table/partition-table.bin --app build/tusb_msc.bin \
      --sd-rw card.img --usb-host msc-sd.txt --max-seconds 3

The tusb_msc image's header says 4 MB flash; the others run with `--flash-mb 2`. A card image
must hold a FAT32 volume (`tests/sd_image.rs` lays one out); the examples do not format on a
failed mount. See ../../docs/peripherals.md.
