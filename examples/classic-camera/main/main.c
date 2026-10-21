// Simulator-owned public-driver fixture; AI-Thinker wiring, internal DRAM only.
#include <stdio.h>
#include <assert.h>
#include "esp_camera.h"
#include "mbedtls/sha256.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"

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
    esp_err_t err = esp_camera_init(&config);
    printf("C8 esp_camera_init=%s (%d)\n", esp_err_to_name(err), err);
    assert(err == ESP_OK);
    for (unsigned n = 0; n < 2; ++n) {
        camera_fb_t *fb = esp_camera_fb_get();
        assert(fb != NULL);
        assert(fb->width == 160 && fb->height == 120 && fb->len == 38400);
        assert(fb->format == PIXFORMAT_RGB565);
        unsigned char hash[32];
        assert(mbedtls_sha256(fb->buf, fb->len, hash, 0) == 0);
        printf("C8 frame=%u width=%u height=%u len=%u RGB565 sha256=", n,
               (unsigned)fb->width, (unsigned)fb->height, (unsigned)fb->len);
        for (unsigned i = 0; i < sizeof(hash); ++i) printf("%02x", hash[i]);
        printf("\n");
        esp_camera_fb_return(fb);
    }
    printf("C8 PASS\n");
    fflush(stdout);
    for (;;) vTaskDelay(pdMS_TO_TICKS(1000));
}
