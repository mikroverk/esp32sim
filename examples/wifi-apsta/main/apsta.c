// A WiFi access point and a station at once (WIFI_MODE_APSTA): the access point takes clients and
// leases them addresses while the station joins another network, takes a lease there and pings
// its gateway. Every event is one console line starting with "apsta: " and a fixed word (STARTED,
// JOINED, LEASED, LEFT, UPLINK, PING, STATUS), so a run on the board and a run in the emulator
// can be compared as text.
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
#include "ping/ping_sock.h"

static esp_netif_t *sta_netif;
static volatile int uplink;   // 0 not joined, 1 joined, 2 with a lease

static void on_ping_end(esp_ping_handle_t h, void *arg)
{
    uint32_t sent = 0, received = 0;
    esp_ping_get_profile(h, ESP_PING_PROF_REQUEST, &sent, sizeof(sent));
    esp_ping_get_profile(h, ESP_PING_PROF_REPLY, &received, sizeof(received));
    printf("apsta: PING done sent=%u received=%u\n", (unsigned)sent, (unsigned)received);
    esp_ping_delete_session(h);
}

static void ping_gateway(void)
{
    esp_netif_ip_info_t ip;
    if (CONFIG_APSTA_PING_COUNT == 0 || esp_netif_get_ip_info(sta_netif, &ip) != ESP_OK) return;
    esp_ping_config_t config = ESP_PING_DEFAULT_CONFIG();
    config.count = CONFIG_APSTA_PING_COUNT;
    config.interval_ms = 200;
    config.target_addr.type = IPADDR_TYPE_V4;
    config.target_addr.u_addr.ip4.addr = ip.gw.addr;
    esp_ping_callbacks_t callbacks = { .on_ping_end = on_ping_end };
    esp_ping_handle_t session;
    esp_err_t err = esp_ping_new_session(&config, &callbacks, &session);
    if (err == ESP_OK) err = esp_ping_start(session);
    if (err != ESP_OK) printf("apsta: PING could not start: %s\n", esp_err_to_name(err));
}

static void on_event(void *arg, esp_event_base_t base, int32_t id, void *data)
{
    if (base == WIFI_EVENT && id == WIFI_EVENT_AP_START) {
        uint8_t mac[6];
        esp_wifi_get_mac(WIFI_IF_AP, mac);
        printf("apsta: STARTED ssid=\"%s\" channel=%d auth=%s bssid=" MACSTR "\n", CONFIG_APSTA_AP_SSID,
               CONFIG_APSTA_CHANNEL, strlen(CONFIG_APSTA_AP_PASSWORD) ? "WPA2" : "OPEN", MAC2STR(mac));
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_AP_STACONNECTED) {
        wifi_event_ap_staconnected_t *e = data;
        printf("apsta: JOINED mac=" MACSTR " aid=%d\n", MAC2STR(e->mac), e->aid);
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_AP_STADISCONNECTED) {
        wifi_event_ap_stadisconnected_t *e = data;
        printf("apsta: LEFT mac=" MACSTR " aid=%d reason=%d\n", MAC2STR(e->mac), e->aid, e->reason);
    } else if (base == IP_EVENT && id == IP_EVENT_AP_STAIPASSIGNED) {
        ip_event_ap_staipassigned_t *e = data;
        printf("apsta: LEASED ip=" IPSTR " mac=" MACSTR "\n", IP2STR(&e->ip), MAC2STR(e->mac));
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_STA_START) {
        esp_wifi_connect();
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_STA_CONNECTED) {
        wifi_event_sta_connected_t *e = data;
        uplink = 1;
        printf("apsta: UPLINK CONNECTED ssid=\"%.*s\" channel=%d\n", e->ssid_len, (char *)e->ssid, e->channel);
    } else if (base == WIFI_EVENT && id == WIFI_EVENT_STA_DISCONNECTED) {
        wifi_event_sta_disconnected_t *e = data;
        uplink = 0;
        printf("apsta: UPLINK DISCONNECTED reason=%d\n", e->reason);
        esp_wifi_connect();
    } else if (base == IP_EVENT && id == IP_EVENT_STA_GOT_IP) {
        ip_event_got_ip_t *e = data;
        uplink = 2;
        printf("apsta: UPLINK GOT_IP ip=" IPSTR " gw=" IPSTR "\n", IP2STR(&e->ip_info.ip), IP2STR(&e->ip_info.gw));
        ping_gateway();
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
    sta_netif = esp_netif_create_default_wifi_sta();
    wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&init));
    ESP_ERROR_CHECK(esp_event_handler_register(WIFI_EVENT, ESP_EVENT_ANY_ID, on_event, NULL));
    ESP_ERROR_CHECK(esp_event_handler_register(IP_EVENT, IP_EVENT_AP_STAIPASSIGNED, on_event, NULL));
    ESP_ERROR_CHECK(esp_event_handler_register(IP_EVENT, IP_EVENT_STA_GOT_IP, on_event, NULL));

    wifi_config_t ap = {
        .ap = {
            .ssid = CONFIG_APSTA_AP_SSID,
            .ssid_len = strlen(CONFIG_APSTA_AP_SSID),
            .channel = CONFIG_APSTA_CHANNEL,
            .password = CONFIG_APSTA_AP_PASSWORD,
            .max_connection = 4,
            .authmode = strlen(CONFIG_APSTA_AP_PASSWORD) ? WIFI_AUTH_WPA2_PSK : WIFI_AUTH_OPEN,
            .pmf_cfg = { .required = false },
        },
    };
    wifi_config_t sta = {
        .sta = {
            .ssid = CONFIG_APSTA_UPLINK_SSID,
            .password = CONFIG_APSTA_UPLINK_PASSWORD,
            .threshold.authmode = WIFI_AUTH_WPA2_PSK,
        },
    };
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_APSTA));
    ESP_ERROR_CHECK(esp_wifi_set_config(WIFI_IF_AP, &ap));
    ESP_ERROR_CHECK(esp_wifi_set_config(WIFI_IF_STA, &sta));
    ESP_ERROR_CHECK(esp_wifi_start());

    // STATUS every five seconds, so the log is never silent
    for (;;) {
        vTaskDelay(pdMS_TO_TICKS(5000));
        wifi_sta_list_t list;
        printf("apsta: STATUS stations=%d uplink=%d\n", esp_wifi_ap_get_sta_list(&list) == ESP_OK ? list.num : -1, uplink);
    }
}
