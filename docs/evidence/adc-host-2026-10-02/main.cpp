#include <Arduino.h>
#if CONFIG_IDF_TARGET_ESP32S3
const int pins[] = {1, 11};
#else
const int pins[] = {0};
#endif
void setup() {
  Serial.begin(115200);
  analogReadResolution(12);
  analogSetAttenuation(ADC_11db);
  Serial.println("ADC READY");
}
void loop() {
  if (!Serial.available()) { delay(1); return; }
  char phase = Serial.read();
  if (phase == '\n') return;
  for (int pin : pins) {
    int raw = analogRead(pin);
    int mv = analogReadMilliVolts(pin);
    Serial.printf("ADC %c pin=%d raw=%d mv=%d\n", phase, pin, raw, mv);
  }
  Serial.println("ADC DONE");
}
