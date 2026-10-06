#include <Arduino.h>
#include <BLEDevice.h>
#include <BLEServer.h>
#include <esp_timer.h>
#include <esp_cpu.h>
#include <soc/syscon_reg.h>
#include <soc/system_reg.h>

// IDF 5.5.5 public clock/reset definitions above. LC/EM layout is inferred
// from C3 rev3 ROM; see INFERRED.md and esp32sim EX211 receipts.
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
    uint8_t et[16], cs[90], tx[14];
    dumpMem(stage,"et",0,0,16);
    if (readMem(0,et,16)) {
      uint32_t c = half(et,8)*2;
      dumpMem(stage,"cs",0,c,90);
      if (c && readMem(c,cs,90) && (half(cs,0)&31)==4) {
        uint32_t first = half(cs,28), next = first;
        for (unsigned i = 0; i < 9 && next; ++i) {
          dumpMem(stage,"tx",i,next,14);
          if (!readMem(next,tx,14)) break;
          unsigned n = half(tx,2)>>8;
          if (n >= 6 && n <= 37) dumpMem(stage,"payload",i,half(tx,4),n-6);
          next = half(tx,0)&0x7fff;
          if (next == first) break;
        }
      }
    }
    for (unsigned i = 0; i < 10; ++i) dumpMem(stage,"rx",i,0x1000+i*20,20);
  }
  Serial.printf("PROBE kind=snapshot stage=%s edge=end us=%lld\n",stage,esp_timer_get_time());
}
static void latch(const char *stage) {
  for (unsigned i = 0; i < 8; ++i) {
    delay(10);
    delayMicroseconds(23 + i * 137);
    // Serialize with the single-core controller's latch use; bounded to ~100 us.
    portENTER_CRITICAL(&latchMux);
    int64_t t0 = esp_timer_get_time();
    uint32_t c0 = esp_cpu_get_cycle_count(), polls = 0, coarse = rd(LC+0x1c);
    bool busy = coarse & 0x80000000;
    uint32_t cw = esp_cpu_get_cycle_count();
    if (!busy) {
      *reinterpret_cast<volatile uint32_t *>(LC+0x1c) = coarse | 0x80000000;
      do { coarse = rd(LC+0x1c); ++polls; }
      while ((coarse & 0x80000000) && uint32_t(esp_cpu_get_cycle_count()-cw) < 16000);
    }
    uint32_t cd = esp_cpu_get_cycle_count(), fine = rd(LC+0x20), c1 = esp_cpu_get_cycle_count();
    int64_t t1 = esp_timer_get_time();
    portEXIT_CRITICAL(&latchMux);
    Serial.printf("PROBE kind=latch stage=%s index=%u busy=%u polls=%lu coarse=0x%08lx fine=0x%08lx t0=%lld t1=%lld c0=%lu cw=%lu cd=%lu c1=%lu\n",stage,i,busy,(unsigned long)polls,(unsigned long)coarse,(unsigned long)fine,t0,t1,(unsigned long)c0,(unsigned long)cw,(unsigned long)cd,(unsigned long)c1);
  }
}
struct Sample { int64_t us; uint32_t cycle, coarse, fine; uint16_t state, entry; uint8_t et[16]; };
static Sample samples[512];
static void events() {
  uint8_t prev[16][16] = {};
  uint32_t alarm0 = ~0u, alarm1 = ~0u, count = 0, dropped = 0, polls = 0;
  int64_t begin = esp_timer_get_time(), last = begin;
  uint32_t maxGap = 0;
  while (esp_timer_get_time()-begin < 2000000) {
    int64_t now = esp_timer_get_time();
    if (now-last > maxGap) maxGap = now-last;
    last = now; ++polls;
    uint32_t a = rd(LC+0xec), b = rd(LC+0xf0);
    if (a != alarm0 || b != alarm1) {
      if (count < 512) samples[count++] = {now,esp_cpu_get_cycle_count(),a,b,0,16,{}};
      else ++dropped;
      alarm0 = a; alarm1 = b;
    }
    for (unsigned i = 0; i < 16; ++i) {
      uint8_t et[16];
      if (readMem(i*16,et,16) && memcmp(et,prev[i],16)) {
        if (count < 512) {
          Sample &s = samples[count++];
          s.us = esp_timer_get_time(); s.cycle = esp_cpu_get_cycle_count();
          s.coarse = half(et,2) | ((uint32_t(half(et,4))&0xfff)<<16);
          s.fine = half(et,6); s.state = (half(et,0)>>3)&7; s.entry = i;
          memcpy(s.et,et,16);
        } else ++dropped;
        memcpy(prev[i],et,16);
      }
    }
    delayMicroseconds(50);
  }
  int64_t end = esp_timer_get_time();
  for (unsigned i = 0; i < count; ++i) {
    const Sample &s = samples[i];
    Serial.printf("PROBE kind=event index=%u source=%s entry=%u us=%lld cycle=%lu coarse=0x%08lx fine=0x%08lx state=%u data=",i,s.entry==16?"alarm":"et",s.entry,s.us,(unsigned long)s.cycle,(unsigned long)s.coarse,(unsigned long)s.fine,s.state);
    for (uint8_t b : s.et) Serial.printf("%02x",b);
    Serial.println();
  }
  Serial.printf("PROBE kind=window begin=%lld end=%lld count=%lu dropped=%lu polls=%lu max_gap_us=%lu\n",begin,end,(unsigned long)count,(unsigned long)dropped,(unsigned long)polls,(unsigned long)maxGap);
}
void setup() {
  Serial.begin(115200);
  delay(1000);
  Serial.printf("PROBE kind=meta version=1 idf=%s cpu_mhz=%u\n",esp_get_idf_version(),getCpuFrequencyMhz());
  snapshot("pre");
  if (!BLEDevice::init("BLE Server Example")) { Serial.println("PROBE kind=error reason=ble_init"); return; }
  snapshot("init");
  latch("init");
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
  delay(500);
  snapshot("adv");
  latch("adv");
  events();
  Serial.println("PROBE kind=done version=1");
}
void loop() { delay(2000); }
