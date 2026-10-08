// SPDX-License-Identifier: MIT
#include <assert.h>
#include <stdio.h>
#include "nvs_flash.h"
#include "nimble/nimble_port.h"
#include "nimble/nimble_port_freertos.h"
#include "host/ble_hs.h"

static void advertise(void)
{
    uint8_t address_type;
    assert(ble_hs_id_infer_auto(0, &address_type) == 0);
    // Flags, complete 16-bit Battery Service UUID and complete local name.
    const uint8_t data[] = {2, 1, 6, 3, 3, 0x0f, 0x18,
                           9, 9, 'e', 's', 'p', '3', '2', 's', 'i', 'm'};
    assert(ble_gap_adv_set_data(data, sizeof(data)) == 0);
    const struct ble_gap_adv_params params = {
        .conn_mode = BLE_GAP_CONN_MODE_NON,
        .disc_mode = BLE_GAP_DISC_MODE_GEN,
        .itvl_min = 0xa0, .itvl_max = 0xa0,
        .channel_map = 7,
    };
    assert(ble_gap_adv_start(address_type, NULL, BLE_HS_FOREVER, &params, NULL, NULL) == 0);
    puts("advertiser: STARTED name=esp32sim service=180f interval_units=160 channels=37,38,39");
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
    ble_hs_cfg.sync_cb = advertise;
    nimble_port_freertos_init(host);
}
