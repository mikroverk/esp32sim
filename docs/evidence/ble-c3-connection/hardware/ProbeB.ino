#include <Arduino.h>
#include <BLEDevice.h>
#include <BLEServer.h>
#include <esp_timer.h>
#include <esp_cpu.h>
#include <soc/syscon_reg.h>
#include <soc/system_reg.h>

// IDF 5.5.5 public clock/reset definitions above. LC/EM layout is inferred
// from C3 rev3 ROM; see esp32sim EX211 and EX213 receipts.
static constexpr uint32_t LC = 0x60031000;
static uint32_t maps[56];
static portMUX_TYPE latchMux = portMUX_INITIALIZER_UNLOCKED;
static uint32_t rd(uint32_t a) { return *reinterpret_cast<volatile uint32_t *>(a); }
static uint16_t half(const uint8_t *b, unsigned n) { return b[n] | (uint16_t(b[n+1]) << 8); }
static uint32_t mapAddr(uint32_t off, unsigned len) {
  uint32_t best = 0, start = 0, end = 0x10000;
  bool found = false;
  for (unsigned i = 0; i < 56; ++i) {
    uint32_t v = maps[i], s = (v >> 18) << 2;
    if (!v) continue;
    if (s > off && s < end) end = s;
    if (s <= off && (!found || s > start)) { best = v; start = s; found = true; }
  }
  if (!found || !(best & 0x3ffff) || off >= end || len > end-off) return 0;
  uint32_t a = (0x3fc00000 | ((best << 2) & 0xffffc)) + off-start;
  // IDF5.5.5 soc/esp32c3/include/soc/soc.h SOC_DRAM_LOW/HIGH.
  if (a < 0x3fc80000 || a >= 0x3fce0000 || len > 0x3fce0000-a) return 0;
  return a;
}
static bool readMem(uint32_t off, uint8_t *b, unsigned len) {
  uint32_t a = mapAddr(off, len);
  if (!a) return false;
  for (unsigned i = 0; i < len; ++i) b[i] = reinterpret_cast<volatile uint8_t *>(a)[i];
  return true;
}
static void dumpMem(const char *stage, const char *kind, unsigned index, uint32_t off, unsigned len) {
  uint8_t b[90];
  bool ok = len <= sizeof(b) && readMem(off, b, len);
  Serial.printf("PROBE kind=mem stage=%s name=%s index=%u logical=0x%04lx physical=0x%08lx valid=%u data=", stage, kind, index, (unsigned long)off, (unsigned long)mapAddr(off,len), ok);
  if (ok) for (unsigned i = 0; i < len; ++i) Serial.printf("%02x", b[i]);
  else Serial.print("none");
  Serial.println();
}
static void snapshot(const char *stage) {
  Serial.printf("PROBE kind=snapshot stage=%s edge=begin us=%lld\n", stage, esp_timer_get_time());
  // Read-only register snapshots, including stored configuration used by the model.
  for (unsigned off = 0; off <= 0x100; off += 4)
    Serial.printf("PROBE kind=reg stage=%s address=0x%08lx value=0x%08lx\n", stage, (unsigned long)(LC+off), (unsigned long)rd(LC+off));
  for (uint32_t a : {LC+0x2c4, LC+0x2c8, LC+0x2cc, LC+0x2d0, LC+0x2d4, LC+0x2d8, LC+0x2dc,
       uint32_t(SYSCON_WIFI_CLK_EN_REG), uint32_t(SYSCON_WIFI_RST_EN_REG),
       uint32_t(SYSTEM_BT_LPCK_DIV_INT_REG), uint32_t(SYSTEM_BT_LPCK_DIV_FRAC_REG)})
    Serial.printf("PROBE kind=reg stage=%s address=0x%08lx value=0x%08lx\n", stage, (unsigned long)a, (unsigned long)rd(a));
  for (unsigned i = 0; i < 56; ++i) maps[i] = rd(LC+0x204+(i < 48 ? i : i+7)*4);
  for (unsigned i = 0; i < 56; ++i)
    Serial.printf("PROBE kind=map stage=%s index=%u value=0x%08lx\n", stage, i, (unsigned long)maps[i]);
  if (strcmp(stage,"pre") != 0) {
    uint8_t et[16], cs[90];
    for (unsigned slot = 0; slot < 16; ++slot) {
      if (!readMem(slot*16,et,16)) continue;
      uint32_t c = half(et,8)*2;
      if (!c || !readMem(c,cs,90)) continue;
      unsigned format = half(cs,0)&31;
      if (format != 3 && format != 4) continue;
      dumpMem(stage,"et",slot,slot*16,16);
      dumpMem(stage,"cs",slot,c,90);
      unsigned next = half(cs,28);
      if (next) dumpMem(stage,"tx",slot,next,14);
    }
    for (unsigned i = 0; i < 10; ++i) dumpMem(stage,"rx",i,0x1000+i*20,20);
  }
  Serial.printf("PROBE kind=snapshot stage=%s edge=end us=%lld\n",stage,esp_timer_get_time());
}
void setup() {
  Serial.begin(115200);
  delay(1000);
  Serial.printf("PROBE kind=meta version=1 idf=%s cpu_mhz=%u\n",esp_get_idf_version(),getCpuFrequencyMhz());

  if (!BLEDevice::init("BLE Server Example")) { Serial.println("PROBE kind=error reason=ble_init"); return; }

  BLEServer *server = BLEDevice::createServer();
  BLEService *service = server->createService("4fafc201-1fb5-459e-8fcc-c5c9c331914b");
  server->advertiseOnDisconnect(true);
  BLECharacteristic *characteristic = service->createCharacteristic("beb5483e-36e1-4688-b7f5-ea07361b26a8",BLECharacteristic::PROPERTY_READ | BLECharacteristic::PROPERTY_WRITE);
  characteristic->setValue("Hello World says Neil");
  service->start();
  BLEAdvertising *adv = BLEDevice::getAdvertising();
  adv->addServiceUUID("4fafc201-1fb5-459e-8fcc-c5c9c331914b");
  adv->setScanResponse(true); adv->setMinPreferred(0x06); adv->setMaxPreferred(0x12);
  BLEDevice::startAdvertising();
  Serial.println("PROBE kind=ready version=2");
}
void loop() {
  static unsigned previous = 99;
  unsigned count = BLEDevice::getServer()->getConnectedCount();
  if (count != previous) {
    snapshot(count ? "connected" : "advertising");
    previous = count;
  }
  delay(100);
}
