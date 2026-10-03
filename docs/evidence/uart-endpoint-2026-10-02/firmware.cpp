#include <Arduino.h>

bool exchange() {
  const char message[] = "UART-PING\n";
  Serial1.write((const uint8_t *)message, sizeof(message) - 1);
  char reply[sizeof(message)] = {};
  size_t received = Serial1.readBytes(reply, sizeof(message) - 1);
  bool ok = received == sizeof(message) - 1 && memcmp(message, reply, received) == 0;
  Serial.printf("UART echo=%s bytes=%u\n", ok ? "PASS" : "FAIL", (unsigned)received);
  return ok;
}
void setup() {
  Serial.begin(115200);
  Serial1.begin(9600, SERIAL_8N1, 4, 5);
  Serial1.setTimeout(1000);
  bool ok = exchange();
  Serial1.end();
  Serial1.begin(19200, SERIAL_8N1, 4, 5);
  Serial1.write('!');
  Serial1.flush();
  delay(20);
  Serial1.end();
  Serial1.begin(9600, SERIAL_8N1, 4, 5);
  ok = exchange() && ok;
  Serial.println(ok ? "UART PASS reset" : "UART FAIL reset");
  Serial.flush();
  delay(20);
  ESP.restart();
}
void loop() {}
