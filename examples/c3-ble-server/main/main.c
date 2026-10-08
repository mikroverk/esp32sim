// SPDX-License-Identifier: MIT
#include <assert.h>
#include <stdio.h>
#include "nvs_flash.h"
#include "nimble/nimble_port.h"
#include "nimble/nimble_port_freertos.h"
#include "host/ble_hs.h"
#include "services/gap/ble_svc_gap.h"
#include "services/gatt/ble_svc_gatt.h"

static const ble_uuid128_t service = BLE_UUID128_INIT(
    0x4b,0x91,0x31,0xc3,0xc9,0xc5,0xcc,0x8f,0x9e,0x45,0xb5,0x1f,0x01,0xc2,0xaf,0x4f);
static const ble_uuid128_t characteristic = BLE_UUID128_INIT(
    0xa8,0x26,0x1b,0x36,0x07,0xea,0xf5,0xb7,0x88,0x46,0xe1,0x36,0x3e,0x48,0xb5,0xbe);
static int read_value(uint16_t conn, uint16_t attr, struct ble_gatt_access_ctxt *ctx, void *arg)
{
    (void)conn; (void)attr; (void)arg;
    const char value[] = "Hello World says Neil";
    return os_mbuf_append(ctx->om, value, sizeof(value)-1) == 0 ? 0 : BLE_ATT_ERR_INSUFFICIENT_RES;
}
static const struct ble_gatt_svc_def services[] = {
    {.type = BLE_GATT_SVC_TYPE_PRIMARY, .uuid = &service.u,
     .characteristics = (const struct ble_gatt_chr_def[]) {
         {.uuid = &characteristic.u, .access_cb = read_value, .flags = BLE_GATT_CHR_F_READ}, {0}}},
    {0}
};
static void advertise(void);
static int gap(struct ble_gap_event *event, void *arg)
{
    (void)arg;
    if (event->type == BLE_GAP_EVENT_DISCONNECT) {
        printf("server: disconnected reason=%d\n", event->disconnect.reason);
        advertise();
    } else if (event->type == BLE_GAP_EVENT_CONNECT) {
        printf("server: connect status=%d\n", event->connect.status);
        if (event->connect.status) advertise();
    }
    return 0;
}
static void advertise(void)
{
    uint8_t address_type;
    assert(ble_hs_id_infer_auto(0, &address_type) == 0);
    const struct ble_hs_adv_fields fields = {.flags = 6, .uuids128 = (ble_uuid128_t *)&service,
        .num_uuids128 = 1, .uuids128_is_complete = 1};
    const char name[] = "BLE Server Example";
    const struct ble_hs_adv_fields response = {.name = (uint8_t *)name,
        .name_len = sizeof(name)-1, .name_is_complete = 1};
    assert(ble_gap_adv_set_fields(&fields) == 0);
    assert(ble_gap_adv_rsp_set_fields(&response) == 0);
    const struct ble_gap_adv_params params = {.conn_mode = BLE_GAP_CONN_MODE_UND,
        .disc_mode = BLE_GAP_DISC_MODE_GEN, .itvl_min = 0xa0, .itvl_max = 0xa0, .channel_map = 7};
    assert(ble_gap_adv_start(address_type, NULL, BLE_HS_FOREVER, &params, gap, NULL) == 0);
    puts("server: advertising");
}
static void host(void *arg)
{
    (void)arg;
    nimble_port_run();
    nimble_port_freertos_deinit();
}
void app_main(void)
{
    ESP_ERROR_CHECK(nvs_flash_init());
    ESP_ERROR_CHECK(nimble_port_init());
    ble_svc_gap_init();
    ble_svc_gatt_init();
    assert(ble_gatts_count_cfg(services) == 0);
    assert(ble_gatts_add_svcs(services) == 0);
    ble_hs_cfg.sync_cb = advertise;
    nimble_port_freertos_init(host);
}
