#!/usr/bin/env python3
"""Independent expected bytes for the retained C8 host image; no emulator imports."""
import hashlib, json
out = bytearray()
for p in range(160 * 120):
    r, g, b = p % 256, (p // 160) % 256, (p * 7) % 256
    value = ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
    # Sensor RGB565 is MSB-first. Classic mode-1 FIFO is [LSB,0,MSB,0];
    # esp32-camera ll_cam_dma_filter_yuyv selects sample1 then sample2,
    # restoring the original MSB,LSB bytes in the public framebuffer.
    out.extend(value.to_bytes(2, 'big'))
print(json.dumps(dict(width=160, height=120, bytes=len(out), byte_order='RGB565 MSB-first', sha256=hashlib.sha256(out).hexdigest()), indent=2))
