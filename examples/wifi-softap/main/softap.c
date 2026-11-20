// A WiFi access point and nothing else: start it, then report who joins, what the DHCP server
// leases them and who leaves. Every event is one console line starting with a fixed word
// (STARTED, JOINED, LEASED, LEFT, STATUS), so a run on the board and a run in the emulator can be
// compared as text.
#include <stdio.h>
#include <string.h>
#include "sdkconfig.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "esp_event.h"
#include "esp_mac.h"
#include "esp_netif.h"
#include "esp_wifi.h"
#include "nvs_flash.h"

static void on_event(void *arg, esp_event_base_t base, int32_t id, void *data)
{
    if (base == WIFI_EVENT && id == WIFI_EVENT_AP_START) {
        uint8_t mac[6];
        esp_wifi_get_mac(WIFI_IF_AP, mac);
        printf("softap: STARTED ssid=\"%s\" channel=%d auth=%s bssid=" MACSTR "\n", CONFIG_SOFTAP_SSID,
               CONFIG_SOFTAP_CHANNEL, strlen(CONFIG_SOFTAP_PASSWORD) ? "WPA2" : "OPEN", MAC2STR(mac));
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_AP_STACONNECTED) {
        wifi_event_ap_staconnected_t *e = data;
        printf("softap: JOINED mac=" MACSTR " aid=%d\n", MAC2STR(e->mac), e->aid);
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_AP_STADISCONNECTED) {
        wifi_event_ap_stadisconnected_t *e = data;
        printf("softap: LEFT mac=" MACSTR " aid=%d reason=%d\n", MAC2STR(e->mac), e->aid, e->reason);
    } else if (base == IP_EVENT && id == IP_EVENT_AP_STAIPASSIGNED) {
        ip_event_ap_staipassigned_t *e = data;
        printf("softap: LEASED ip=" IPSTR " mac=" MACSTR "\n", IP2STR(&e->ip), MAC2STR(e->mac));
    }
}

void app_main(void)
{
    esp_err_t err = nvs_flash_init();
    if (err == ESP_ERR_NVS_NO_FREE_PAGES || err == ESP_ERR_NVS_NEW_VERSION_FOUND) {
        ESP_ERROR_CHECK(nvs_flash_erase());
        err = nvs_flash_init();
    }
    ESP_ERROR_CHECK(err);
    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    esp_netif_create_default_wifi_ap();
    wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&init));
    ESP_ERROR_CHECK(esp_event_handler_register(WIFI_EVENT, ESP_EVENT_ANY_ID, on_event, NULL));
    ESP_ERROR_CHECK(esp_event_handler_register(IP_EVENT, IP_EVENT_AP_STAIPASSIGNED, on_event, NULL));

    wifi_config_t config = {
        .ap = {
            .ssid = CONFIG_SOFTAP_SSID,
            .ssid_len = strlen(CONFIG_SOFTAP_SSID),
            .channel = CONFIG_SOFTAP_CHANNEL,
            .password = CONFIG_SOFTAP_PASSWORD,
            .max_connection = 4,
            .authmode = strlen(CONFIG_SOFTAP_PASSWORD) ? WIFI_AUTH_WPA2_PSK : WIFI_AUTH_OPEN,
            .pmf_cfg = { .required = false },
        },
    };
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_AP));
    ESP_ERROR_CHECK(esp_wifi_set_config(WIFI_IF_AP, &config));
    ESP_ERROR_CHECK(esp_wifi_start());

    // STATUS every five seconds, so the log is never silent
    for (;;) {
        vTaskDelay(pdMS_TO_TICKS(5000));
        wifi_sta_list_t list;
        printf("softap: STATUS stations=%d\n", esp_wifi_ap_get_sta_list(&list) == ESP_OK ? list.num : -1);
    }
}
