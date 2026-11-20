// Measures, on silicon, the match flags in word 0 of the classic ESP32's receive header
// (rx_ctrl bits 28..31): a SoftAP (and, on request, a station joined to another network) with
// promiscuous reception on, counting the top nibble of word 0 per frame type and per kind of first
// and third address (A1, A3): the SoftAP's MAC, the station's MAC, a group address, the BSSID the
// station joined, someone else's. Every 3 s one line per combination seen. "join WORD PASS" on
// the console joins the station to the first network whose SSID contains WORD.
#include <stdio.h>
#include <string.h>
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "esp_event.h"
#include "esp_mac.h"
#include "esp_netif.h"
#include "esp_wifi.h"
#include "nvs_flash.h"
#include "driver/uart.h"
#include "lwip/sockets.h"
#include "esp_netif.h"

static uint8_t ap_mac[6], sta_mac[6];
static volatile int got_ip;
static uint8_t joined_bssid[6];

// After a lease, a datagram to the gateway every 200 ms, so that the network sends the station
// unicast frames (ARP replies, ICMP) whose flags can be counted.
static void traffic(void *arg)
{
    int sock = socket(AF_INET, SOCK_DGRAM, 0);
    for (;;) {
        vTaskDelay(pdMS_TO_TICKS(200));
        if (!got_ip) continue;
        esp_netif_ip_info_t info;
        if (esp_netif_get_ip_info(esp_netif_get_handle_from_ifkey("WIFI_STA_DEF"), &info) != ESP_OK) continue;
        struct sockaddr_in to = { .sin_family = AF_INET, .sin_port = htons(9), .sin_addr.s_addr = info.gw.addr };
        sendto(sock, "probe", 5, 0, (struct sockaddr *)&to, sizeof to);
    }
}
// [frame: 0 data, 1 beacon, 2 probe request, 3 other management][a1 kind][a3 kind][word0 >> 28]
static volatile uint32_t counts[4][5][5][16];
static const char *KIND[5] = {"softap", "station", "group", "other", "joined-bssid"};
static const char *FRAME[4] = {"data", "beacon", "probe-req", "mgmt"};
static int kind_of(const uint8_t *a)
{
    return memcmp(a, ap_mac, 6) == 0 ? 0 : memcmp(a, sta_mac, 6) == 0 ? 1 : (a[0] & 1) ? 2 : memcmp(a, joined_bssid, 6) == 0 ? 4 : 3;
}

static void on_rx(void *buf, wifi_promiscuous_pkt_type_t type)
{
    if (type != WIFI_PKT_DATA && type != WIFI_PKT_MGMT) return;
    const wifi_promiscuous_pkt_t *p = buf;
    uint32_t w0 = ((const uint32_t *)&p->rx_ctrl)[0];
    int subtype = (p->payload[0] >> 4) & 15;
    int frame = type == WIFI_PKT_DATA ? 0 : subtype == 8 ? 1 : subtype == 4 ? 2 : 3;
    counts[frame][kind_of(p->payload + 4)][kind_of(p->payload + 16)][w0 >> 28]++;
}

static void on_event(void *arg, esp_event_base_t base, int32_t id, void *data)
{
    if (base == WIFI_EVENT && id == WIFI_EVENT_AP_STACONNECTED) printf("probe: AP JOINED " MACSTR "\n", MAC2STR(((wifi_event_ap_staconnected_t *)data)->mac));
    if (base == WIFI_EVENT && id == WIFI_EVENT_STA_CONNECTED) { memcpy(joined_bssid, ((wifi_event_sta_connected_t *)data)->bssid, 6); printf("probe: STA CONNECTED channel %d\n", ((wifi_event_sta_connected_t *)data)->channel); }
    if (base == WIFI_EVENT && id == WIFI_EVENT_STA_DISCONNECTED) printf("probe: STA DISCONNECTED reason=%d\n", ((wifi_event_sta_disconnected_t *)data)->reason);
    if (base == IP_EVENT && id == IP_EVENT_STA_GOT_IP) { got_ip = 1; printf("probe: STA GOT_IP\n"); }
}

static void console(void *arg)
{
    char line[160]; int n = 0;
    for (;;) {
        uint8_t c;
        if (uart_read_bytes(UART_NUM_0, &c, 1, portMAX_DELAY) != 1) continue;
        if (c != '\r' && c != '\n') { if (n < (int)sizeof line - 1) line[n++] = c; continue; }
        line[n] = 0; n = 0;
        char word[33] = "", pass[65] = "";
        // "join WORD PASS": scan, and join the first network whose SSID contains WORD
        if (sscanf(line, "join %32s %64s", word, pass) == 2) {
            wifi_scan_config_t scan = {0};
            esp_wifi_scan_start(&scan, true);
            uint16_t n = 20; wifi_ap_record_t recs[20];
            esp_wifi_scan_get_ap_records(&n, recs);
            int found = -1;
            for (int i = 0; i < n && found < 0; i++) if (strstr((char *)recs[i].ssid, word)) found = i;
            if (found < 0) {
                printf("probe: no network with '%s' among %d\n", word, n);
                for (int i = 0; i < n; i++) printf("probe: seen ch=%d rssi=%d ssid=%s\n", recs[i].primary, recs[i].rssi, (char *)recs[i].ssid);
                continue;
            }
            printf("probe: joining a network on channel %d\n", recs[found].primary);
            wifi_config_t sta = {0};
            memcpy(sta.sta.ssid, recs[found].ssid, 32); strncpy((char *)sta.sta.password, pass, 64);
            esp_wifi_set_config(WIFI_IF_STA, &sta);
            esp_wifi_disconnect(); esp_wifi_connect();
            printf("probe: STA connecting\n");
        }
    }
}

void app_main(void)
{
    ESP_ERROR_CHECK(nvs_flash_init());
    ESP_ERROR_CHECK(esp_netif_init());
    ESP_ERROR_CHECK(esp_event_loop_create_default());
    esp_netif_create_default_wifi_ap();
    esp_netif_create_default_wifi_sta();
    wifi_init_config_t init = WIFI_INIT_CONFIG_DEFAULT();
    ESP_ERROR_CHECK(esp_wifi_init(&init));
    ESP_ERROR_CHECK(esp_wifi_set_storage(WIFI_STORAGE_RAM));
    esp_event_handler_register(WIFI_EVENT, ESP_EVENT_ANY_ID, on_event, NULL);
    esp_event_handler_register(IP_EVENT, IP_EVENT_STA_GOT_IP, on_event, NULL);
    wifi_config_t ap = { .ap = { .ssid = "rxctrl-probe", .ssid_len = 12, .password = "rxctrl-pass", .channel = 6,
                                 .max_connection = 4, .authmode = WIFI_AUTH_WPA2_PSK, .pmf_cfg = { .required = false } } };
    ESP_ERROR_CHECK(esp_wifi_set_mode(WIFI_MODE_APSTA));
    ESP_ERROR_CHECK(esp_wifi_set_config(WIFI_IF_AP, &ap));
    ESP_ERROR_CHECK(esp_wifi_start());
    esp_wifi_get_mac(WIFI_IF_AP, ap_mac); esp_wifi_get_mac(WIFI_IF_STA, sta_mac);
    printf("probe: STARTED softap " MACSTR " station " MACSTR "\n", MAC2STR(ap_mac), MAC2STR(sta_mac));
    wifi_promiscuous_filter_t f = { .filter_mask = WIFI_PROMIS_FILTER_MASK_MGMT | WIFI_PROMIS_FILTER_MASK_DATA };
    esp_wifi_set_promiscuous_filter(&f);
    esp_wifi_set_promiscuous_rx_cb(on_rx);
    esp_wifi_set_promiscuous(true);
    uart_driver_install(UART_NUM_0, 512, 0, 0, NULL, 0);
    xTaskCreate(console, "console", 6144, NULL, 5, NULL);
    xTaskCreate(traffic, "traffic", 4096, NULL, 5, NULL);
    for (;;) {
        vTaskDelay(pdMS_TO_TICKS(3000));
        for (int t = 0; t < 4; t++) for (int k = 0; k < 5; k++) for (int a = 0; a < 5; a++) for (int b = 0; b < 16; b++) {
            uint32_t n = counts[t][k][a][b];
            if (n) { printf("probe: %s a1=%s a3=%s bits31..28=%d%d%d%d n=%lu\n", FRAME[t], KIND[k], KIND[a], b >> 3 & 1, b >> 2 & 1, b >> 1 & 1, b & 1, (unsigned long)n); counts[t][k][a][b] = 0; }
        }
    }
}
