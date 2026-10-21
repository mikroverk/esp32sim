#!/usr/bin/env python3
"""Re-fetch immutable public sources and check the C4 register trace contract."""
from pathlib import Path
import hashlib,json,re,urllib.request
sources=json.loads(Path(__file__).with_name('sources.json').read_text())
texts={}
for name,item in sources.items():
 data=urllib.request.urlopen(item['url']).read()
 assert hashlib.sha256(data).hexdigest()==item['sha256'],name
 texts[name]=data.decode()
h=texts['i2s_reg.h']
for name,offset in [('CONF',8),('TIMING',0x1c),('FIFO_CONF',0x20),('RXEOF_NUM',0x24),('CONF_CHAN',0x2c),('IN_LINK',0x34),('LC_CONF',0x60),('CONF2',0xa8),('CLKM_CONF',0xac),('SAMPLE_RATE_CONF',0xb0)]:
 match=re.search(r'#define I2S_'+name+r'_REG\(i\).*?\+ (0x[0-9a-fA-F]+)',h)
 assert match and int(match[1],16)==offset,(name,match)
for name,bit in [('RX_SLAVE_MOD',7),('RX_START',5),('RX_RESET',1),('RX_FIFO_RESET',3),('RX_RIGHT_FIRST',9),('RX_MSB_RIGHT',17),('RX_MSB_SHIFT',11),('RX_MONO',15),('RX_SHORT_SYNC',13),('LCD_EN',5),('CAMERA_EN',0),('RX_FIFO_MOD_FORCE_EN',20),('DSCR_EN',12),('IN_RST',0),('AHBM_FIFO_RST',2),('AHBM_RST',3),('RX_DSYNC_SW',21)]:
 assert re.search(r'#define I2S_'+name+r'\s+\(BIT\('+str(bit)+r'\)\)',h),name
for name,shift in [('RX_FIFO_MOD',16),('RX_BITS_MOD',18),('RX_CHAN_MOD',3)]:
 assert re.search(r'#define I2S_'+name+r'_S\s+'+str(shift)+r'\b',h),name
c=texts['ll_cam.c']
for field,value in [('conf2.lcd_en',1),('conf2.camera_en',1),('conf.rx_slave_mod',1),('sample_rate_conf.rx_bits_mod',0),('timing.rx_dsync_sw',1)]:
 assert f'I2S0.{field} = {value};' in c,field
assert 'I2S1.' not in c
print('PASS: immutable hashes, ten offsets, nineteen bits, three shifts, camera driver configuration and I2S0-only selection')
