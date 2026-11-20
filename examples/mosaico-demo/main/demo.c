// ESP-Mosaico demo for esp32sim: LVGL 9 on the round 480 x 480 CO5300 AMOLED through the
// esp-mosaico-bsp (display, CST9220 touch, buttons, motor, BMI270, BMM150), small enough to run
// in the browser. Two pages:
//   compass: the BMM150's field as a needle (north tip red), the heading of the board's top
//            edge, and a ball that rolls to the lowest point of its ring (the BMI270's
//            accelerometer: centred when the board lies flat, at the bottom held upright);
//   sensors: the raw readings and the input counters.
// Touch anywhere: a ring follows the finger and the touch counter goes up. The AI button
// (GPIO 7) changes the accent colour and pulses the vibration motor; the Boot key (GPIO 61)
// switches the page. Every input is also one console line that starts with "demo: ".
#include <inttypes.h>
#include <math.h>
#include <stdio.h>
#include "bsp/esp_mosaico.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "lvgl.h"

#define CX 240
#define CY 240
#define NEEDLE_R 100
#define LEVEL_R 64
#define BUBBLE_R 14
#define SENSOR_PERIOD_MS 100
#define MOTOR_PULSE_MS 120

static const char *TAG = "demo";

static const uint32_t ACCENTS[] = {0x18c8ff, 0xff4fa3, 0x7cf068, 0xffb020};
#define N_ACCENTS (sizeof(ACCENTS) / sizeof(ACCENTS[0]))

static struct {
    lv_obj_t *compass, *sensors;                 // the two pages
    lv_obj_t *scale, *title, *needle_n, *needle_s, *hub, *north, *heading, *level, *bubble;
    lv_obj_t *readout, *counters, *uptime, *touch_ring, *hint;
    lv_point_precise_t n_pts[2], s_pts[2];
} ui;

static volatile uint32_t s_touches, s_ai, s_boot;
static volatile int s_accent, s_page;
static volatile bool s_dirty_accent, s_dirty_page;
static bool s_imu_ok, s_mag_ok[BSP_MAGNETOMETER_NUM];
static esp_timer_handle_t s_motor_off;

static lv_obj_t *plain(lv_obj_t *parent)
{
    lv_obj_t *o = lv_obj_create(parent);
    lv_obj_remove_style_all(o);
    lv_obj_remove_flag(o, LV_OBJ_FLAG_CLICKABLE | LV_OBJ_FLAG_SCROLLABLE);
    return o;
}

static lv_obj_t *label(lv_obj_t *parent, const lv_font_t *font, uint32_t colour, const char *text)
{
    lv_obj_t *l = lv_label_create(parent);
    lv_obj_set_style_text_font(l, font, 0);
    lv_obj_set_style_text_color(l, lv_color_hex(colour), 0);
    lv_obj_set_style_text_align(l, LV_TEXT_ALIGN_CENTER, 0);
    lv_label_set_text(l, text);
    return l;
}

static lv_obj_t *needle(lv_obj_t *parent, lv_point_precise_t *pts, uint32_t colour, int width)
{
    lv_obj_t *l = lv_line_create(parent);
    lv_obj_set_style_line_width(l, width, 0);
    lv_obj_set_style_line_color(l, lv_color_hex(colour), 0);
    lv_obj_set_style_line_rounded(l, true, 0);
    lv_line_set_points(l, pts, 2);
    return l;
}

static void on_touch(lv_event_t *e)
{
    lv_point_t p;
    lv_indev_get_point(lv_indev_active(), &p);
    if (lv_event_get_code(e) == LV_EVENT_PRESSED) {
        s_touches++;
        printf("demo: touch %" PRIu32 " at %d,%d\n", s_touches, (int)p.x, (int)p.y);
    }
    if (lv_event_get_code(e) == LV_EVENT_RELEASED) {
        lv_obj_add_flag(ui.touch_ring, LV_OBJ_FLAG_HIDDEN);
        return;
    }
    lv_obj_set_pos(ui.touch_ring, p.x - 28, p.y - 28);
    lv_obj_remove_flag(ui.touch_ring, LV_OBJ_FLAG_HIDDEN);
}

static void apply_accent(void)
{
    lv_color_t c = lv_color_hex(ACCENTS[s_accent]);
    lv_obj_set_style_line_color(ui.scale, c, LV_PART_INDICATOR);
    lv_obj_set_style_text_color(ui.title, c, 0);
    lv_obj_set_style_border_color(ui.level, c, 0);
    lv_obj_set_style_bg_color(ui.bubble, c, 0);
    lv_obj_set_style_border_color(ui.touch_ring, c, 0);
}

static void build_ui(void)
{
    lv_obj_t *scr = lv_screen_active();
    lv_obj_set_style_bg_color(scr, lv_color_black(), 0);
    lv_obj_set_style_bg_opa(scr, LV_OPA_COVER, 0);
    lv_obj_remove_flag(scr, LV_OBJ_FLAG_SCROLLABLE);
    lv_obj_add_event_cb(scr, on_touch, LV_EVENT_PRESSED, NULL);
    lv_obj_add_event_cb(scr, on_touch, LV_EVENT_PRESSING, NULL);
    lv_obj_add_event_cb(scr, on_touch, LV_EVENT_RELEASED, NULL);

    // The tick ring is on both pages: drawn once, recoloured by the AI button.
    ui.scale = lv_scale_create(scr);
    lv_obj_remove_flag(ui.scale, LV_OBJ_FLAG_CLICKABLE);
    lv_obj_set_size(ui.scale, 456, 456);
    lv_obj_center(ui.scale);
    lv_scale_set_mode(ui.scale, LV_SCALE_MODE_ROUND_INNER);
    lv_scale_set_range(ui.scale, 0, 360);
    lv_scale_set_angle_range(ui.scale, 360);
    lv_scale_set_rotation(ui.scale, 270);
    lv_scale_set_total_tick_count(ui.scale, 73);
    lv_scale_set_major_tick_every(ui.scale, 6);
    lv_scale_set_label_show(ui.scale, false);
    lv_obj_set_style_arc_width(ui.scale, 0, LV_PART_MAIN);
    lv_obj_set_style_length(ui.scale, 18, LV_PART_INDICATOR);
    lv_obj_set_style_line_width(ui.scale, 4, LV_PART_INDICATOR);
    lv_obj_set_style_length(ui.scale, 8, LV_PART_ITEMS);
    lv_obj_set_style_line_width(ui.scale, 2, LV_PART_ITEMS);
    lv_obj_set_style_line_color(ui.scale, lv_color_hex(0x4a5568), LV_PART_ITEMS);

    ui.title = label(scr, &lv_font_montserrat_20, ACCENTS[0], "ESP-Mosaico");
    lv_obj_align(ui.title, LV_ALIGN_CENTER, 0, 146);

    // Page 1: compass and level.
    ui.compass = plain(scr);
    lv_obj_set_size(ui.compass, 480, 480);
    ui.level = plain(ui.compass);
    lv_obj_set_size(ui.level, 2 * LEVEL_R, 2 * LEVEL_R);
    lv_obj_set_style_radius(ui.level, LV_RADIUS_CIRCLE, 0);
    lv_obj_set_style_border_width(ui.level, 2, 0);
    lv_obj_set_style_border_opa(ui.level, LV_OPA_COVER, 0);
    lv_obj_align(ui.level, LV_ALIGN_CENTER, 0, 0);
    ui.bubble = plain(ui.compass);
    lv_obj_set_size(ui.bubble, 2 * BUBBLE_R, 2 * BUBBLE_R);
    lv_obj_set_style_radius(ui.bubble, LV_RADIUS_CIRCLE, 0);
    lv_obj_set_style_bg_opa(ui.bubble, LV_OPA_COVER, 0);
    lv_obj_set_pos(ui.bubble, CX - BUBBLE_R, CY - BUBBLE_R);
    ui.n_pts[0] = ui.n_pts[1] = ui.s_pts[0] = ui.s_pts[1] = (lv_point_precise_t){CX, CY};
    ui.needle_s = needle(ui.compass, ui.s_pts, 0x9aa5b1, 8);
    ui.needle_n = needle(ui.compass, ui.n_pts, 0xff3b30, 8);
    ui.hub = plain(ui.compass);
    lv_obj_set_size(ui.hub, 18, 18);
    lv_obj_set_style_radius(ui.hub, LV_RADIUS_CIRCLE, 0);
    lv_obj_set_style_bg_opa(ui.hub, LV_OPA_COVER, 0);
    lv_obj_set_style_bg_color(ui.hub, lv_color_hex(0xe5e7eb), 0);
    lv_obj_center(ui.hub);
    ui.north = label(ui.compass, &lv_font_montserrat_20, 0xff3b30, "N");
    ui.heading = label(ui.compass, &lv_font_montserrat_32, 0xffffff, "---\xc2\xb0");
    lv_obj_align(ui.heading, LV_ALIGN_CENTER, 0, -146);
    ui.hint = label(ui.compass, &lv_font_montserrat_14, 0x8b95a1, "touch me \xe2\x80\xa2 AI: colour \xe2\x80\xa2 BOOT: page");
    lv_obj_align(ui.hint, LV_ALIGN_BOTTOM_MID, 0, -60);

    // Page 2: the numbers behind it.
    ui.sensors = plain(scr);
    lv_obj_set_size(ui.sensors, 480, 480);
    lv_obj_add_flag(ui.sensors, LV_OBJ_FLAG_HIDDEN);
    ui.readout = label(ui.sensors, &lv_font_montserrat_20, 0xe5e7eb, "");
    lv_obj_align(ui.readout, LV_ALIGN_CENTER, 0, -24);
    ui.counters = label(ui.sensors, &lv_font_montserrat_14, 0x8b95a1, "");
    lv_obj_align(ui.counters, LV_ALIGN_CENTER, 0, 118);

    ui.uptime = label(scr, &lv_font_montserrat_14, 0x8b95a1, "");
    lv_obj_align(ui.uptime, LV_ALIGN_BOTTOM_MID, 0, -36);

    ui.touch_ring = plain(scr);
    lv_obj_set_size(ui.touch_ring, 56, 56);
    lv_obj_set_style_radius(ui.touch_ring, LV_RADIUS_CIRCLE, 0);
    lv_obj_set_style_border_width(ui.touch_ring, 4, 0);
    lv_obj_set_style_border_opa(ui.touch_ring, LV_OPA_COVER, 0);
    lv_obj_add_flag(ui.touch_ring, LV_OBJ_FLAG_HIDDEN);

    apply_accent();
}

static void motor_off(void *arg)
{
    (void)arg;
    bsp_motor_set(false);
}

static void on_ai(void *button, void *data)
{
    (void)button; (void)data;
    s_ai++;
    s_accent = (s_accent + 1) % N_ACCENTS;
    s_dirty_accent = true;
    printf("demo: AI button %" PRIu32 ": accent %d, motor pulse\n", s_ai, s_accent);
    if (bsp_motor_set(true) == ESP_OK) {
        esp_timer_stop(s_motor_off);
        esp_timer_start_once(s_motor_off, MOTOR_PULSE_MS * 1000);
    }
}

static void on_boot(void *button, void *data)
{
    (void)button; (void)data;
    s_boot++;
    s_page ^= 1;
    s_dirty_page = true;
    printf("demo: BOOT key %" PRIu32 ": %s page\n", s_boot, s_page ? "sensors" : "compass");
}

static void inputs_init(void)
{
    button_handle_t ai[BSP_BUTTON_NUM];
    int n = 0;
    if (bsp_iot_button_create(ai, &n, BSP_BUTTON_NUM) == ESP_OK && n > 0) {
        iot_button_register_cb(ai[BSP_BUTTON_AI], BUTTON_SINGLE_CLICK, NULL, on_ai, NULL);
    }
    // The BSP creates only the AI button; the Boot key the same way, on BSP_BUTTON_BOOT_GPIO.
    const button_config_t common = {0};
    const button_gpio_config_t boot = {.gpio_num = BSP_BUTTON_BOOT_GPIO, .active_level = BSP_BUTTON_ACTIVE_LEVEL,
                                       .enable_power_save = true};
    button_handle_t b = NULL;
    if (iot_button_new_gpio_device(&common, &boot, &b) == ESP_OK) {
        iot_button_register_cb(b, BUTTON_SINGLE_CLICK, NULL, on_boot, NULL);
    }
    const esp_timer_create_args_t off = {.callback = motor_off, .name = "motor"};
    esp_timer_create(&off, &s_motor_off);
    if (bsp_motor_init() != ESP_OK) ESP_LOGW(TAG, "no motor");
}

static void sensors_init(void)
{
    const bsp_imu_config_t imu = BSP_IMU_CONFIG_DEFAULT();
    s_imu_ok = bsp_imu_init() == ESP_OK && bsp_imu_start(&imu) == ESP_OK;
    for (int i = 0; i < BSP_MAGNETOMETER_NUM; i++) s_mag_ok[i] = bsp_magnetometer_init(i) == ESP_OK;
    printf("demo: sensors imu=%d mag0=%d mag1=%d\n", s_imu_ok, s_mag_ok[0], s_mag_ok[1]);
}

static void set_needle(float rad)
{
    const float c = cosf(rad), s = sinf(rad);
    ui.n_pts[1] = (lv_point_precise_t){CX + c * NEEDLE_R, CY + s * NEEDLE_R};
    ui.s_pts[1] = (lv_point_precise_t){CX - c * (NEEDLE_R - 40), CY - s * (NEEDLE_R - 40)};
    lv_line_set_points(ui.needle_n, ui.n_pts, 2);
    lv_line_set_points(ui.needle_s, ui.s_pts, 2);
    lv_obj_align(ui.north, LV_ALIGN_CENTER, c * (NEEDLE_R + 22), s * (NEEDLE_R + 22));
}

static const char *cardinal(int deg)
{
    static const char *names[] = {"N", "NE", "E", "SE", "S", "SW", "W", "NW"};
    return names[((deg + 22) % 360) / 45];
}

void app_main(void)
{
    printf("demo: start\n");
    bsp_display_config_t cfg = BSP_DISPLAY_DEFAULT_CONFIG();
    lv_display_t *disp = bsp_display_start_with_config(&cfg);
    if (!disp) {
        printf("demo: display failed\n");
        return;
    }
    sensors_init();
    inputs_init();
    if (bsp_display_lock(-1)) {
        build_ui();
        bsp_display_unlock();
    }
    printf("demo: ui up\n");

    float last_heading = -1;
    int last_bx = 0, last_by = 0, tick = 0;
    for (;;) {
        float ax = 0, ay = 0, az = 1, gx = 0, gy = 0, gz = 0;
        struct bmm150_mag_data mag[BSP_MAGNETOMETER_NUM] = {0};
        bool imu = s_imu_ok && bsp_imu_get_accel(&ax, &ay, &az) == ESP_OK;
        if (imu && s_page) bsp_imu_get_gyro(&gx, &gy, &gz);
        bool m0 = s_mag_ok[0] && bsp_magnetometer_read(BSP_MAGNETOMETER_0, &mag[0]) == ESP_OK;
        if (s_mag_ok[1]) bsp_magnetometer_read(BSP_MAGNETOMETER_1, &mag[1]);

        // The needle points along the field's horizontal part, the sensor's axes taken as the
        // screen's (x right, y down); the heading is that of the screen's top edge.
        float field = atan2f((float)mag[0].y, (float)mag[0].x);
        int heading = ((int)lrintf(270.0f - field * 180.0f / (float)M_PI) % 360 + 360) % 360;
        // The accelerometer reads the reaction to gravity: the ball goes the other way.
        int bx = (int)lrintf(-ax * LEVEL_R), by = (int)lrintf(-ay * LEVEL_R);
        float r = sqrtf((float)(bx * bx + by * by));
        if (r > LEVEL_R - BUBBLE_R) { bx = bx * (LEVEL_R - BUBBLE_R) / r; by = by * (LEVEL_R - BUBBLE_R) / r; }

        if (bsp_display_lock(-1)) {
            if (s_dirty_accent) { s_dirty_accent = false; apply_accent(); }
            if (s_dirty_page) {
                s_dirty_page = false;
                if (s_page) { lv_obj_add_flag(ui.compass, LV_OBJ_FLAG_HIDDEN); lv_obj_remove_flag(ui.sensors, LV_OBJ_FLAG_HIDDEN); }
                else { lv_obj_add_flag(ui.sensors, LV_OBJ_FLAG_HIDDEN); lv_obj_remove_flag(ui.compass, LV_OBJ_FLAG_HIDDEN); }
            }
            if (!s_page) {
                if (m0 && fabsf(field - last_heading) > 0.01f) {
                    last_heading = field;
                    set_needle(field);
                    lv_label_set_text_fmt(ui.heading, "%d\xc2\xb0 %s", heading, cardinal(heading));
                }
                if (imu && (bx != last_bx || by != last_by)) {
                    last_bx = bx; last_by = by;
                    lv_obj_set_pos(ui.bubble, CX + bx - BUBBLE_R, CY + by - BUBBLE_R);
                }
            } else if (tick % 2 == 0) {
                lv_label_set_text_fmt(ui.readout,
                    "accel  %+.2f %+.2f %+.2f g\n"
                    "gyro  %+.0f %+.0f %+.0f \xc2\xb0/s\n"
                    "mag 1  %+d %+d %+d uT\n"
                    "mag 2  %+d %+d %+d uT",
                    ax, ay, az, gx, gy, gz, mag[0].x, mag[0].y, mag[0].z, mag[1].x, mag[1].y, mag[1].z);
                lv_label_set_text_fmt(ui.counters, "touches %" PRIu32 "   AI %" PRIu32 "   BOOT %" PRIu32,
                                      s_touches, s_ai, s_boot);
            }
            if (tick % 10 == 0) {
                lv_label_set_text_fmt(ui.uptime, "ESP32-S31 \xe2\x80\xa2 up %" PRIu32 " s", (uint32_t)(esp_timer_get_time() / 1000000));
            }
            bsp_display_unlock();
        }
        if (tick % 50 == 0) {
            printf("demo: t=%ds heading=%d accel=%+.2f,%+.2f,%+.2f mag=%d,%d,%d\n", tick / 10, heading, ax, ay, az,
                   mag[0].x, mag[0].y, mag[0].z);
        }
        tick++;
        vTaskDelay(pdMS_TO_TICKS(SENSOR_PERIOD_MS));
    }
}
