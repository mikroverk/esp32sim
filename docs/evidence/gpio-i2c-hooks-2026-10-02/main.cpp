#include <Arduino.h>
#include <Wire.h>

void waitHost() {
  while (!Serial0.available()) delay(1);
  Serial0.read();
}
void setup() {
  Serial0.begin(115200);
  pinMode(4, INPUT_PULLUP);
  Serial0.println("HOOK pullup");
  waitHost();
  pinMode(4, INPUT_PULLDOWN);
  Serial0.println("HOOK pulldown");
  waitHost();
  pinMode(4, OUTPUT);
  digitalWrite(4, HIGH);
  Serial0.println("HOOK output");
  waitHost();
  Wire.begin(8, 9);
  for (int phase = 0; phase < 4; ++phase) {
    Wire.beginTransmission(0x42);
    Serial0.printf("HOOK i2c %d %d\n", phase, Wire.endTransmission());
    waitHost();
  }
  Serial0.println("HOOK done");
}
void loop() { delay(1000); }
