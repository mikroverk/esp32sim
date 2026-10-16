// Octal SPI flash and octal PSRAM probe (ESP32-S3-WROOM-2), built with FlashMode=opi, PSRAM=opi
// (fqbn-options.txt). On UART0: the flash ID and sizes, the PSRAM size, a PSRAM fill-and-check
// over the whole heap block, a flash erase / program / read at the last sector of the chip and in
// the spiffs partition (read back through the cache by mmap), then `done`. The first boot restarts
// once (the flash is left in octal DTR mode across the chip reset) and runs the checks again.
#include <esp_flash.h>
#include <esp_partition.h>
#include <esp_psram.h>
#include <esp_system.h>
#include <spi_flash_mmap.h>

static uint32_t mix(uint32_t x) {
  x ^= x >> 16;
  x *= 0x7feb352d;
  x ^= x >> 15;
  x *= 0x846ca68b;
  return x ^ (x >> 16);
}

// Every 64th word of the block and its first and last 16 KB: a 16 MB array that wraps at 8 MB
// shows up as a mismatch.
static void psramCheck() {
  size_t n = heap_caps_get_largest_free_block(MALLOC_CAP_SPIRAM) & ~3u;
  uint32_t *p = (uint32_t *)heap_caps_malloc(n, MALLOC_CAP_SPIRAM);
  if (!p) {
    Serial.println("psram alloc=fail");
    return;
  }
  size_t words = n / 4, bad = 0, checked = 0;
  auto picked = [&](size_t i) { return i % 64 == 0 || i < 4096 || i >= words - 4096; };
  for (size_t i = 0; i < words; i++)
    if (picked(i)) p[i] = mix(i);
  for (size_t i = 0; i < words; i++)
    if (picked(i)) {
      checked++;
      if (p[i] != mix(i)) bad++;
    }
  Serial.printf("psram block=%u at=%p checked=%u bad=%u\n", n, p, checked, bad);
  heap_caps_free(p);
}

// Erase a 4 KB sector at `addr`, program 256 bytes, read them back.
static bool flashCheck(uint32_t addr, uint8_t seed) {
  uint8_t out[256], in[256];
  for (int i = 0; i < 256; i++) out[i] = (uint8_t)(mix(i + seed) >> 8);
  if (esp_flash_erase_region(NULL, addr, 4096) != ESP_OK) return false;
  if (esp_flash_read(NULL, in, addr + 256, 4) != ESP_OK || in[0] != 0xff) return false;
  if (esp_flash_write(NULL, out, addr, sizeof out) != ESP_OK) return false;
  if (esp_flash_read(NULL, in, addr, sizeof in) != ESP_OK) return false;
  return memcmp(in, out, sizeof in) == 0;
}

void setup() {
  Serial.begin(115200);
  bool again = esp_reset_reason() == ESP_RST_SW;
  uint32_t id = 0, physical = 0;
  esp_flash_read_id(NULL, &id);
  esp_flash_get_physical_size(NULL, &physical);
  Serial.printf("boot again=%d\n", again);
  Serial.printf("flash id=%06lx chip=%lu physical=%lu\n", id, ESP.getFlashChipSize(), physical);
  Serial.printf("psram size=%u heap=%lu\n", esp_psram_get_size(), ESP.getPsramSize());
  psramCheck();
  Serial.printf("flash end=%s\n", flashCheck(physical - 4096, again) ? "ok" : "fail");
  const esp_partition_t *part =
    esp_partition_find_first(ESP_PARTITION_TYPE_DATA, ESP_PARTITION_SUBTYPE_DATA_SPIFFS, NULL);
  bool ok = part && flashCheck(part->address, 7);
  if (ok) {
    const void *map;
    esp_partition_mmap_handle_t h;
    uint8_t first[256];
    esp_flash_read(NULL, first, part->address, sizeof first);
    ok = esp_partition_mmap(part, 0, 65536, ESP_PARTITION_MMAP_DATA, &map, &h) == ESP_OK &&
         memcmp(map, first, sizeof first) == 0;
    if (ok) esp_partition_munmap(h);
  }
  Serial.printf("flash spiffs=%s\n", ok ? "ok" : "fail");
  uint8_t magic = 0;
  const esp_partition_t *app = esp_partition_find_first(ESP_PARTITION_TYPE_APP, ESP_PARTITION_SUBTYPE_ANY, NULL);
  if (app) esp_partition_read(app, 0, &magic, 1);
  Serial.printf("app magic=%02x\n", magic);
  Serial.println("done");
  if (!again) {
    delay(50);
    esp_restart();
  }
}

void loop() {}
