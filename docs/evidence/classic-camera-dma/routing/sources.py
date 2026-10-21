#!/usr/bin/env python3
"""Check the pinned public driver/matrix contract; no private sources."""
import hashlib, json, re, sys, urllib.request
from pathlib import Path
out = Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
idf = '38eeba213aa695aabfd6d89aa9f5078dbe5a94c3'
cam = 'b817e9b81041980608d1a3d3e0b08d86e3666382'
urls = {name: f'https://raw.githubusercontent.com/espressif/esp-idf/{idf}/components/soc/esp32/include/soc/{name}' for name in ['gpio_sig_map.h','gpio_reg.h','io_mux_reg.h','dport_reg.h']}
urls['ll_cam.c'] = f'https://raw.githubusercontent.com/espressif/esp32-camera/{cam}/target/esp32/ll_cam.c'
rows = {}; texts = {}
for name, url in urls.items():
 data = urllib.request.urlopen(url).read(); texts[name] = data.decode()
 (out/name).write_bytes(data)
 rows[name] = dict(url=url, bytes=len(data), sha256=hashlib.sha256(data).hexdigest())
sig = texts['gpio_sig_map.h']
for name, value in [('I2S0I_WS_IN_IDX',28),('I2S0I_DATA_IN0_IDX',140),('I2S0I_H_SYNC_IDX',190),('I2S0I_V_SYNC_IDX',191),('I2S0I_H_ENABLE_IDX',192)]:
 assert re.search(r'#define\s+' + name + r'\s+' + str(value) + r'\b', sig), name
c = texts['ll_cam.c']
for text in ['gpio_matrix_in(config->pin_pclk, I2S0I_WS_IN_IDX, false)', 'gpio_matrix_in(config->pin_vsync, I2S0I_V_SYNC_IDX, false)', 'gpio_matrix_in(config->pin_href, I2S0I_H_SYNC_IDX, false)', 'gpio_matrix_in(data_pins[i], I2S0I_DATA_IN0_IDX + i, false)', 'gpio_matrix_in(0x38, I2S0I_H_ENABLE_IDX, false)', 'ets_delay_us(1)']:
 assert text in c, text
# Retain exact source lines rather than inferred line-number claims.
for name, row in rows.items():
 patterns = ['I2S0I_', 'GPIO_FUNC0_IN_SEL', 'GPIO_SIG0_IN_SEL', 'GPIO_FUNC0_IN_INV_SEL', 'GPIO_PRO_CPU_INT_ENA', 'GPIO_PIN0_INT_ENA', 'ets_delay_us(1)', 'gpio_matrix_in(', 'PERIP_CLK_EN_REG', 'I2S0_CLK_EN']
 row['selected_lines'] = [{'line': i, 'text': line} for i,line in enumerate(texts[name].splitlines(),1) if any(s in line for s in patterns)]
(out/'sources.json').write_text(json.dumps(rows, indent=2)+'\n')
print('PASS: five immutable public sources; five matrix IDs, five driver routes and delayed VSYNC ISR contract')
