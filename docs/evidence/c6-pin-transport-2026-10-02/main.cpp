#include <Arduino.h>
#include <Wire.h>
volatile unsigned edges = 0;
void ARDUINO_ISR_ATTR echoEdge() { edges++; }
void setup() {
  Serial.begin(115200);
  Wire.begin(6, 7);
  Wire.beginTransmission(0x42);
  Wire.write(0x19);
  Serial.printf("I2C right=%u\n", Wire.endTransmission());
  unsigned count = Wire.requestFrom(0x42, 1);
  Serial.printf("I2C count=%u value=%02x\n", count, Wire.read());
  Wire.end();
  Wire.begin(10, 11);
  Wire.beginTransmission(0x42);
  Serial.printf("I2C wrong=%u\n", Wire.endTransmission());
  pinMode(5, INPUT);
  attachInterrupt(5, echoEdge, CHANGE);
  pinMode(4, OUTPUT);
  digitalWrite(4, HIGH);
  delayMicroseconds(100);
  digitalWrite(4, LOW);
  unsigned long width = pulseIn(5, HIGH, 10000);
  delay(1);
  Serial.printf("ECHO width_us=%lu edges=%u\n", width, edges);
  Serial.println("C6 TRANSPORT DONE");
}
void loop() { delay(1000); }
