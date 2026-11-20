// ESP-Mosaico v1.2 board inputs and outputs, the way the esp-mosaico-bsp drives them
// (components/esp-mosaico-bsp, onboard/esp_mosaico.c and onboard/sensors.c): the AI button
// (GPIO 7) and the Boot key (GPIO 61), the vibration motor on GPIO 8 through LEDC PWM (the BSP
// default, CONFIG_BSP_MOTOR_ENABLE_PWM) and as a plain GPIO, and the two BMM150 magnetometers
// (0x11, 0x12) through the espressif/bmm150 driver the BSP uses. Every result is one console
// line that starts with "board: ".
#include <inttypes.h>
#include <math.h>
#include <stdio.h>
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "driver/gpio.h"
#include "driver/i2c_master.h"
#include "driver/ledc.h"
#include "bmm150.h"
#include "bmm150_common.h"

#define PIN_AI      GPIO_NUM_7
#define PIN_BOOT    GPIO_NUM_61
#define PIN_MOTOR   GPIO_NUM_8
#define PIN_SDA     GPIO_NUM_56
#define PIN_SCL     GPIO_NUM_3

// bsp_motor_init with CONFIG_BSP_MOTOR_ENABLE_PWM, then bsp_motor_set_strength.
static void motor_pwm(void)
{
    const ledc_timer_config_t timer = {
        .speed_mode = LEDC_LOW_SPEED_MODE, .duty_resolution = LEDC_TIMER_10_BIT,
        .timer_num = LEDC_TIMER_0, .freq_hz = 5000, .clk_cfg = LEDC_AUTO_CLK,
    };
    esp_err_t err = ledc_timer_config(&timer);
    const ledc_channel_config_t channel = {
        .gpio_num = PIN_MOTOR, .speed_mode = LEDC_LOW_SPEED_MODE, .channel = LEDC_CHANNEL_0,
        .intr_type = LEDC_INTR_DISABLE, .timer_sel = LEDC_TIMER_0, .duty = 0, .hpoint = 0,
    };
    if (err == ESP_OK) err = ledc_channel_config(&channel);
    printf("board: MOTOR pwm init %s\n", esp_err_to_name(err));
    if (err != ESP_OK) return;
    const uint8_t steps[] = {60, 100, 0};
    for (int i = 0; i < 3; i++) {
        uint32_t duty = ((1U << 10) - 1U) * steps[i] / 100U;
        ledc_set_duty(LEDC_LOW_SPEED_MODE, LEDC_CHANNEL_0, duty);
        ledc_update_duty(LEDC_LOW_SPEED_MODE, LEDC_CHANNEL_0);
        printf("board: MOTOR pwm %u%%\n", steps[i]);
        vTaskDelay(pdMS_TO_TICKS(200));
    }
}

// bsp_motor_init without PWM: a push-pull output, low = off.
static void motor_gpio(void)
{
    gpio_reset_pin(PIN_MOTOR);
    gpio_set_level(PIN_MOTOR, 0);
    gpio_set_direction(PIN_MOTOR, GPIO_MODE_OUTPUT);
    for (int on = 1; on >= 0; on--) {
        gpio_set_level(PIN_MOTOR, on);
        printf("board: MOTOR gpio %s\n", on ? "on" : "off");
        vTaskDelay(pdMS_TO_TICKS(200));
    }
}

static struct bmm150_dev mag[2];
static bool mag_ok[2];

// bsp_magnetometer_init: bmm150_init, normal mode, regular preset, wait for the first sample.
static void mag_init(i2c_master_bus_handle_t bus, int id, uint8_t addr)
{
    esp_err_t err = bmm150_interface_init_from_master_bus(&mag[id], bus, addr, 400000);
    int8_t r = err == ESP_OK ? bmm150_init(&mag[id]) : -1;
    struct bmm150_settings s = { .pwr_mode = BMM150_POWERMODE_NORMAL, .preset_mode = BMM150_PRESETMODE_REGULAR };
    if (r == BMM150_OK) r = bmm150_set_op_mode(&s, &mag[id]);
    if (r == BMM150_OK) r = bmm150_set_presetmode(&s, &mag[id]);
    uint8_t status = 0;
    for (int i = 0; r == BMM150_OK && i < 20 && !(status & BMM150_DRDY_STATUS_MSK); i++) {
        bmm150_get_regs(BMM150_REG_DATA_READY_STATUS, &status, 1, &mag[id]);
        if (!(status & BMM150_DRDY_STATUS_MSK)) vTaskDelay(pdMS_TO_TICKS(5));
    }
    mag_ok[id] = r == BMM150_OK;
    printf("board: MAG%d addr=0x%02x chip_id=0x%02x init=%d drdy=%d\n", id, addr, mag[id].chip_id, r, status & 1);
}

static void mag_read(const char *when)
{
    for (int id = 0; id < 2; id++) {
        if (!mag_ok[id]) continue;
        struct bmm150_mag_data d;
        int8_t r = bmm150_read_mag_data(&d, &mag[id]);
        float norm = sqrtf(d.x * d.x + d.y * d.y + d.z * d.z);
        printf("board: MAG%d %s r=%d x=%.0f y=%.0f z=%.0f |B|=%.0f uT\n", id, when, r, d.x, d.y, d.z, norm);
    }
}

void app_main(void)
{
    printf("board: START\n");
    const gpio_config_t keys = { .pin_bit_mask = BIT64(PIN_AI) | BIT64(PIN_BOOT), .mode = GPIO_MODE_INPUT, .pull_up_en = GPIO_PULLUP_ENABLE };
    gpio_config(&keys);
    printf("board: KEYS ai=%d boot=%d\n", gpio_get_level(PIN_AI), gpio_get_level(PIN_BOOT));

    motor_pwm();
    motor_gpio();

    i2c_master_bus_config_t cfg = {
        .i2c_port = I2C_NUM_0, .sda_io_num = PIN_SDA, .scl_io_num = PIN_SCL,
        .clk_source = I2C_CLK_SRC_DEFAULT, .glitch_ignore_cnt = 7, .flags.enable_internal_pullup = true,
    };
    i2c_master_bus_handle_t bus = NULL;
    esp_err_t err = i2c_new_master_bus(&cfg, &bus);
    printf("board: I2C %s\n", esp_err_to_name(err));
    if (err == ESP_OK) { mag_init(bus, 0, 0x11); mag_init(bus, 1, 0x12); }
    mag_read("rest");

    // Key edges for 8 s (a script presses them), then the field again: --imu-motion 1 has
    // turned the board by then.
    int ai = gpio_get_level(PIN_AI), boot = gpio_get_level(PIN_BOOT);
    for (int t = 0; t < 800; t++) {
        int a = gpio_get_level(PIN_AI), b = gpio_get_level(PIN_BOOT);
        if (a != ai) printf("board: KEY ai %s\n", a ? "released" : "pressed");
        if (b != boot) printf("board: KEY boot %s\n", b ? "released" : "pressed");
        ai = a; boot = b;
        vTaskDelay(pdMS_TO_TICKS(10));
    }
    mag_read("later");
    printf("board: DONE\n");
}
