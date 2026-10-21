// Same pinned public driver and DRAM configuration as main.c; no sensor/driver stubs.
#include <assert.h>
#include <stdio.h>
#include "esp_camera.h"
#include "mbedtls/sha256.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

static void frame(const char *phase, unsigned n) {
    camera_fb_t *fb = esp_camera_fb_get();
    assert(fb != NULL);
    assert(fb->width == 160 && fb->height == 120 && fb->len == 38400);
    assert(fb->format == PIXFORMAT_RGB565);
    unsigned char hash[32];
    assert(mbedtls_sha256(fb->buf, fb->len, hash, 0) == 0);
    printf("C9 %s frame=%u width=%u height=%u len=%u RGB565 sha256=", phase, n,
           (unsigned)fb->width, (unsigned)fb->height, (unsigned)fb->len);
    for (unsigned i = 0; i < sizeof(hash); ++i) printf("%02x", hash[i]);
    printf("\n");
    esp_camera_fb_return(fb);
}

void app_main(void) {
    const camera_config_t config = {
        .pin_pwdn = 32, .pin_reset = -1, .pin_xclk = 0,
        .pin_sccb_sda = 26, .pin_sccb_scl = 27,
        .pin_d0 = 5, .pin_d1 = 18, .pin_d2 = 19, .pin_d3 = 21,
        .pin_d4 = 36, .pin_d5 = 39, .pin_d6 = 34, .pin_d7 = 35,
        .pin_vsync = 25, .pin_href = 23, .pin_pclk = 22,
        .xclk_freq_hz = 10000000, .ledc_timer = LEDC_TIMER_0,
        .ledc_channel = LEDC_CHANNEL_0, .pixel_format = PIXFORMAT_RGB565,
        .frame_size = FRAMESIZE_QQVGA, .jpeg_quality = 12, .fb_count = 1,
        .fb_location = CAMERA_FB_IN_DRAM, .grab_mode = CAMERA_GRAB_WHEN_EMPTY,
    };
    assert(esp_camera_init(&config) == ESP_OK);
    printf("C9 init=ESP_OK\n");
    frame("initial", 0);
    frame("initial", 1);
    printf("C9 REPLACE\n");
    fflush(stdout);
    // Host changes the image at the marker, without stopping capture. A queued
    // frame may still contain the old image; require the next two fresh frames.
    vTaskDelay(pdMS_TO_TICKS(1000));
    frame("queued", 0);
    frame("replacement", 0);
    frame("replacement", 1);
    // Deliberately leave non-default sensor registers behind. Reinitialization
    // must restore QQVGA/RGB565, not merely reuse the old host conversion cache.
    sensor_t *sensor = esp_camera_sensor_get();
    assert(sensor != NULL);
    assert(sensor->set_framesize(sensor, FRAMESIZE_QVGA) == 0);
    assert(sensor->set_pixformat(sensor, PIXFORMAT_YUV422) == 0);
    assert(sensor->status.framesize == FRAMESIZE_QVGA);
    assert(sensor->pixformat == PIXFORMAT_YUV422);
    printf("C9 sensor=QVGA/YUV422\n");
    assert(esp_camera_deinit() == ESP_OK);
    assert(esp_camera_sensor_get() == NULL);
    printf("C9 deinit=ESP_OK sensor=NULL\n");
    fflush(stdout);
    vTaskDelay(pdMS_TO_TICKS(100));
    assert(esp_camera_init(&config) == ESP_OK);
    printf("C9 reinit=ESP_OK\n");
    frame("reinit", 0);
    frame("reinit", 1);
    printf("C9 PASS\n");
    fflush(stdout);
    for (;;) vTaskDelay(pdMS_TO_TICKS(1000));
}
