#include <Arduino.h>
#include <WiFi.h>
#include <HTTPClient.h>
#ifndef NET_URL
#error Supply NET_URL for the host HTTP server
#endif
#ifndef NET_PSK
#define NET_PSK ""
#endif
void setup() {
  Serial.begin(115200);
  Serial.println("NET start");
  WiFi.begin("esp32sim", NET_PSK);
  unsigned long start = millis();
  while (WiFi.status() != WL_CONNECTED && millis() - start < 20000) delay(100);
  Serial.printf("NET open status=%d ip=%s\n", WiFi.status(), WiFi.localIP().toString().c_str());
  if (WiFi.status() == WL_CONNECTED) {
    HTTPClient http;
    http.begin(NET_URL);
    int code = http.GET();
    Serial.printf("NET http=%d body=%s\n", code, http.getString().c_str());
    http.end();
  }
  Serial.println("NET done");
}
void loop() { delay(1000); }
