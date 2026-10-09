# Wi-Fi and TLS accelerator fixture

Arduino-ESP32 **3.3.11**, bundled ESP-IDF **5.5.5**, connects to the simulator's
WPA2 access point and obtains `10.0.2.15`. It then runs two real Mbed TLS peers
with TLS 1.2, ECDHE-PSK, P-256 and AES-128-CBC-SHA256. Each peer sends the byte
sequence 0..255 and checks all 256 received bytes. The peers communicate through
bounded memory queues, so the test needs no host server or Internet connection.
C3/C6 use hardware SHA and AES; C6 also uses hardware ECC. Removing those SHA
paths or C6 ECC prevents the TLS golden from passing.

The PSK and xorshift RNG are deliberately public and deterministic. They must
not be used for application security. This fixture tests cryptographic driver
progress and data integrity, not certificate validation, entropy, TLS over TCP,
RF behavior or accelerator timing. C6 retains three PLL calibration warnings.

## Rebuild

Install Arduino CLI **1.5.1** and `esp32:esp32@3.3.11` from Espressif's package
index. The package pins `esp-rv32/2601`, GCC **14.2.0**, and esptool **5.3.1**.
Set `ARDUINO_DATA` to the Arduino15 package directory. From the repository root:

```sh
for chip in c3 c6; do
  CHIP=$chip BUILD_DIR="$PWD/target/crypto-tls-$chip" examples/crypto-tls/build.sh
  cp "target/crypto-tls-$chip/fixture.bin" "web/wasm/fw/public/$chip-crypto-tls.bin"
  cp "target/crypto-tls-$chip/Probe.ino.bootloader.bin" "web/wasm/fw/public/$chip-crypto-tls-bootloader.bin"
  cp "target/crypto-tls-$chip/Probe.ino.partitions.bin" "web/wasm/fw/public/$chip-crypto-tls-ptable.bin"
done
```

The build fixes `SOURCE_DATE_EPOCH=0`, maps compiled paths, and strips ELF debug
information before esptool calculates the embedded ELF hash. Two builds with
different source and output directories produced identical app, bootloader and
partition-table bytes for both chips. The hashes and retained archive inventory
are in [inputs.json](../../docs/evidence/c3-c6-sha-dma-wifi-cal/inputs.json).
The fixture and library versions must be pinned together when updating inputs.
Check retained link-map sections before updating the [NOTICE](../../web/wasm/fw/public/crypto-tls-NOTICE.txt).

```sh
tools/fetch-demo-assets.sh --no-linux
ESP32SIM_ROM_DIR="$PWD/web/wasm/fw" cargo +1.99.0 test --release \
  -p esp32sim --test goldens crypto_tls -- --ignored
```

`crypto_tls_c3` and `crypto_tls_c6` use only the committed flash images and the
CI-fetched mask ROMs, without ELF hooks or stubs. They pin the complete console,
stop exception/interrupt totals, per-source interrupt counts and Wi-Fi/network
reports. Cycle-derived instruction totals are not a correctness oracle here.
Existing station goldens remain unchanged, including the older C6 fixture's
`bb_init` stub. The new C6 fixture proves startup without that stub.
