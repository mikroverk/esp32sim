#define CAM_SDA_PIN 8
#define CAM_SCL_PIN 7
#define CAM_XCLK_PIN 38
#define CAM_PCLK_PIN 41
#define CAM_VSYNC_PIN 17
#define CAM_HREF_PIN 18
#define CAM_D0_PIN 45
#define CAM_D1_PIN 47
#define CAM_D2_PIN 48
#define CAM_D3_PIN 46
#define CAM_D4_PIN 42
#define CAM_D5_PIN 40
#define CAM_D6_PIN 39
#define CAM_D7_PIN 21
#include <Arduino.h>
#include <esp_camera.h>
#include <driver/i2c_master.h>

static void configureCamera(pixformat_t format) {
  camera_config_t config = {};
  config.pin_pwdn = -1;
  config.pin_reset = -1;
  config.pin_sccb_sda = -1;
  config.pin_sccb_scl = -1;
  config.sccb_i2c_port = 0;
  config.pin_xclk = CAM_XCLK_PIN;
  config.pin_pclk = CAM_PCLK_PIN;
  config.pin_vsync = CAM_VSYNC_PIN;
  config.pin_href = CAM_HREF_PIN;
  config.pin_d0 = CAM_D0_PIN; config.pin_d1 = CAM_D1_PIN;
  config.pin_d2 = CAM_D2_PIN; config.pin_d3 = CAM_D3_PIN;
  config.pin_d4 = CAM_D4_PIN; config.pin_d5 = CAM_D5_PIN;
  config.pin_d6 = CAM_D6_PIN; config.pin_d7 = CAM_D7_PIN;
  config.xclk_freq_hz = 20000000;
  config.ledc_timer = LEDC_TIMER_0;
  config.ledc_channel = LEDC_CHANNEL_0;
  config.pixel_format = format;
  config.frame_size = format == PIXFORMAT_JPEG ? FRAMESIZE_QQVGA : FRAMESIZE_96X96;
  config.jpeg_quality = 12;
  config.fb_count = 1;
  config.fb_location = CAMERA_FB_IN_DRAM;
  config.grab_mode = CAMERA_GRAB_WHEN_EMPTY;
  esp_err_t result = esp_camera_init(&config);
  sensor_t *sensor = esp_camera_sensor_get();
  Serial.printf("CAMERA:READY:%d:%x\n", result, sensor ? sensor->id.PID : 0);
}

void setup() {
  Serial.begin(115200);
  // Waveshare shares I2C0 with its codecs and IO expander.
  i2c_master_bus_config_t bus = {};
  bus.i2c_port = I2C_NUM_0;
  bus.sda_io_num = static_cast<gpio_num_t>(CAM_SDA_PIN);
  bus.scl_io_num = static_cast<gpio_num_t>(CAM_SCL_PIN);
  bus.clk_source = I2C_CLK_SRC_DEFAULT;
  bus.glitch_ignore_cnt = 7;
  bus.flags.enable_internal_pullup = 1;
  i2c_master_bus_handle_t handle;
  ESP_ERROR_CHECK(i2c_new_master_bus(&bus, &handle));
  configureCamera(PIXFORMAT_RGB565);
}

void loop() {
  if (!Serial.available()) { delay(1); return; }
  char command = Serial.read();
  if (command == 'C') {
    camera_fb_t *frame = esp_camera_fb_get();
    if (!frame) { Serial.println("CAMERA:EMPTY"); return; }
    uint32_t hash = 2166136261u;
    for (size_t i = 0; i < frame->len; i++) hash = (hash ^ frame->buf[i]) * 16777619u;
    Serial.printf("CAMERA:FRAME:%u:%u:%u:%u:%08lx:%02x:%02x\n", unsigned(frame->width), unsigned(frame->height), unsigned(frame->format), unsigned(frame->len), static_cast<unsigned long>(hash), frame->buf[0], frame->buf[frame->len - 1]);
    esp_camera_fb_return(frame);
  }
  if (command == 'Y') { esp_camera_deinit(); configureCamera(PIXFORMAT_YUV422); }
  if (command == 'G') { esp_camera_deinit(); configureCamera(PIXFORMAT_GRAYSCALE); }
  if (command == 'J') { esp_camera_deinit(); configureCamera(PIXFORMAT_JPEG); }
  if (command == 'B') ESP.restart();
}
