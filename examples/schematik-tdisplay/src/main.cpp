#include <Arduino.h>
#include <Arduino_GFX_Library.h>
#include <esp_sleep.h>


// Forward declarations
void measureCurrentText();
void drawStaticMessage();
void beginMessage();
void drawScrollFrame();
void nextMessage();
void powerOff();
void handleButton(bool reading, bool &previousReading, bool &stableState, uint32_t &lastChange, uint32_t &pressedAt, bool &longPressHandled);

constexpr int TFT_SCLK = 18;
constexpr int TFT_MOSI = 19;
constexpr int TFT_CS = 5;
constexpr int TFT_DC = 16;
constexpr int TFT_RST = 23;
constexpr int TFT_BL = 4;
constexpr int BUTTON_NEXT = 35;
constexpr int BUTTON_BOTTOM = 0;
constexpr uint16_t BLACK = 0x0000;
constexpr uint16_t WHITE = 0xFFFF;
constexpr uint32_t DEBOUNCE_MS = 35;
constexpr uint32_t SCROLL_FRAME_MS = 50;
constexpr uint32_t LONG_PRESS_MS = 1200;
// The screen is 240 px wide, so a message is shrunk to fit before it scrolls.
// Scrolling only kicks in once it would have to drop below this text size:
// 2 keeps anything up to 20 characters still (16 px tall), 3 scrolls anything
// over 13 characters instead, at a much larger size.
constexpr int16_t MIN_STATIC_TEXT_SIZE = 2;

Arduino_DataBus *bus = new Arduino_ESP32SPI(TFT_DC, TFT_CS, TFT_SCLK, TFT_MOSI, GFX_NOT_DEFINED);
Arduino_GFX *display = new Arduino_ST7789(bus, TFT_RST, 1, true, 135, 240, 52, 40, 53, 40);

const char *messages[] = {
  "Schematik Hairclip",
  "Schematik is great",
  "Like and Subscribe"
};
constexpr uint8_t MESSAGE_COUNT = sizeof(messages) / sizeof(messages[0]);

uint8_t messageIndex = 0;
bool scrolling = false;
int16_t textWidth = 0;
int16_t textHeight = 0;
int16_t textSize = 1;
int16_t scrollX = 0;
int16_t previousX = 0;
bool previousTopReading = HIGH;
bool stableTopState = HIGH;
uint32_t lastTopChange = 0;
uint32_t topPressedAt = 0;
bool topLongPressHandled = false;
bool previousBottomReading = HIGH;
bool stableBottomState = HIGH;
bool bottomButtonArmed = true;
uint32_t lastBottomChange = 0;
uint32_t bottomPressedAt = 0;
bool bottomLongPressHandled = false;
uint32_t lastScrollFrame = 0;

void measureCurrentText() {
  const int16_t screenWidth = display->width();
  const size_t characters = strlen(messages[messageIndex]);
  textSize = 8;
  while (textSize > 1 && (characters * 6 * textSize) > screenWidth) {
    textSize--;
  }
  textWidth = characters * 6 * textSize;
  textHeight = 8 * textSize;
  // Only scroll when fitting the message would make it too small to read.
  scrolling = textSize < MIN_STATIC_TEXT_SIZE;
  if (scrolling) {
    textSize = 6;
    textWidth = characters * 6 * textSize;
    textHeight = 8 * textSize;
  }
}

void drawStaticMessage() {
  display->fillScreen(BLACK);
  display->setTextSize(textSize);
  display->setTextColor(WHITE);
  display->setTextWrap(false);
  display->setCursor((display->width() - textWidth) / 2, (display->height() - textHeight) / 2);
  display->print(messages[messageIndex]);
}

void beginMessage() {
  measureCurrentText();
  if (scrolling) {
    scrollX = display->width();
    previousX = scrollX;
    display->fillScreen(BLACK);
  } else {
    drawStaticMessage();
  }
}

void drawScrollFrame() {
  const int16_t y = (display->height() - textHeight) / 2;
  // Clear only the band occupied by moving letters, then redraw it one step left.
  display->fillRect(0, y, display->width(), textHeight, BLACK);
  display->setTextSize(textSize);
  display->setTextColor(WHITE);
  display->setTextWrap(false);
  display->setCursor(scrollX, y);
  display->print(messages[messageIndex]);
  previousX = scrollX;
}

void nextMessage() {
  messageIndex = (messageIndex + 1) % MESSAGE_COUNT;
  beginMessage();
}

void powerOff() {
  // The press that switches the board off holds GPIO0 low. Wait for its release
  // before arming GPIO0, otherwise the ESP32 would wake immediately.
  display->fillScreen(BLACK);
  digitalWrite(TFT_BL, LOW);
  while (digitalRead(BUTTON_BOTTOM) == LOW) {
    delay(10);
  }
  delay(DEBOUNCE_MS);
  esp_sleep_enable_ext0_wakeup(static_cast<gpio_num_t>(BUTTON_BOTTOM), LOW);
  esp_deep_sleep_start();
}

void setup() {
  pinMode(TFT_BL, OUTPUT);
  digitalWrite(TFT_BL, HIGH);
  pinMode(BUTTON_NEXT, INPUT);
  pinMode(BUTTON_BOTTOM, INPUT_PULLUP);

  // A deep-sleep wake happens while the bottom button is still held down.
  // Do not treat that same press as a new request to switch off again.
  previousTopReading = stableTopState = digitalRead(BUTTON_NEXT);
  previousBottomReading = stableBottomState = digitalRead(BUTTON_BOTTOM);
  bottomButtonArmed = (stableBottomState == HIGH);

  display->begin();
  beginMessage();
}

void handleButton(bool reading, bool &previousReading, bool &stableState,
                  uint32_t &lastChange, uint32_t &pressedAt, bool &longPressHandled) {
  const uint32_t now = millis();
  if (reading != previousReading) {
    lastChange = now;
    previousReading = reading;
  }
  if ((now - lastChange) >= DEBOUNCE_MS && reading != stableState) {
    stableState = reading;
    if (stableState == LOW) {
      // A quick top-button tap changes the message exactly once.
      nextMessage();
    }
  }
}

void loop() {
  const uint32_t now = millis();
  // The top button advances through messages. The bottom button is power only.
  handleButton(digitalRead(BUTTON_NEXT), previousTopReading, stableTopState,
               lastTopChange, topPressedAt, topLongPressHandled);

  const bool bottomReading = digitalRead(BUTTON_BOTTOM);
  if (!bottomButtonArmed) {
    // The wake press must be released before a later press can switch off again.
    if (bottomReading == HIGH) {
      bottomButtonArmed = true;
      previousBottomReading = HIGH;
      stableBottomState = HIGH;
      lastBottomChange = now;
    }
  } else {
    if (bottomReading != previousBottomReading) {
      lastBottomChange = now;
      previousBottomReading = bottomReading;
    }
    if ((now - lastBottomChange) >= DEBOUNCE_MS && bottomReading != stableBottomState) {
      stableBottomState = bottomReading;
      if (stableBottomState == LOW) {
        // One bottom-button press turns the screen off and enters deep sleep.
        // That same button is configured as the wake source in powerOff().
        powerOff();
      }
    }
  }

  if (scrolling && (now - lastScrollFrame) >= SCROLL_FRAME_MS) {
    lastScrollFrame = now;
    scrollX -= 2;
    if (scrollX < -textWidth) {
      scrollX = display->width();
    }
    drawScrollFrame();
  }
}
