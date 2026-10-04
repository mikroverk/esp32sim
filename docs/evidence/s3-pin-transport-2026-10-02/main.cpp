#include <Arduino.h>
#include <Wire.h>
#include <SPI.h>
void setup() {
  Serial.begin(115200);
  delay(100);
  Wire.begin(8, 9);
  Wire.beginTransmission(0x42);
  Serial.printf("I2C right=%u\n", Wire.endTransmission());
  Wire.end();
  Wire.begin(6, 7);
  Wire.beginTransmission(0x42);
  Serial.printf("I2C wrong=%u\n", Wire.endTransmission());
  Wire.end();
  pinMode(10, OUTPUT);
  pinMode(14, OUTPUT);
  digitalWrite(10, HIGH);
  digitalWrite(14, HIGH);
  SPI.begin(12, 13, 11, 10);
  SPI.beginTransaction(SPISettings(1000000, MSBFIRST, SPI_MODE0));
  digitalWrite(10, LOW);
  uint8_t right = SPI.transfer(0xa5);
  digitalWrite(10, HIGH);
  digitalWrite(14, LOW);
  uint8_t wrong = SPI.transfer(0x5a);
  digitalWrite(14, HIGH);
  SPI.endTransaction();
  Serial.printf("SPI right=%02x wrong=%02x\n", right, wrong);
  pinMode(4, OUTPUT);
  digitalWrite(4, HIGH);
  delayMicroseconds(100);
  digitalWrite(4, LOW);
  Serial.println("PULSE requested_us=100");
  Serial.println("TRANSPORT DONE");
}
void loop() { delay(1000); }
