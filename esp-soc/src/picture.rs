//! Tiny image loaders for camera sources: binary PPM (P6) and uncompressed 24/32-bit BMP.
//! Anything a page uploads comes through here, so a bad header is an error, never a panic.
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;
pub const MAX_RGBA_BYTES: u64 = 8 << 20;
pub struct Picture { pub w: u32, pub h: u32, pub rgb: Vec<u8> }

impl Picture {
    pub fn valid(&self) -> bool { valid_size(self.w, self.h) && self.rgb.len() as u64 == self.w as u64 * self.h as u64 * 3 }
    fn pixel(&self, x: u32, y: u32, w: u32, h: u32) -> (u8, u8, u8) {
        let o = ((y as u64 * self.h as u64 / h as u64 * self.w as u64 + x as u64 * self.w as u64 / w as u64) * 3) as usize;
        (self.rgb[o], self.rgb[o + 1], self.rgb[o + 2])
    }
    pub fn from_rgba(w: u32, h: u32, rgba: &[u8]) -> Option<Self> {
        if rgba_len(w, h) != Some(rgba.len()) { return None; }
        let rgb = rgba.as_chunks::<4>().0.iter().flat_map(|p| p[..3].iter().copied()).collect();
        Some(Self { w, h, rgb })
    }
}

pub fn valid_size(w: u32, h: u32) -> bool { w != 0 && h != 0 && w as u64 * h as u64 <= MAX_PIXELS }
pub fn rgba_len(w: u32, h: u32) -> Option<usize> {
    if !valid_size(w, h) { return None; }
    let n = w as u64 * h as u64 * 4;
    (n + 5 <= MAX_RGBA_BYTES).then_some(n as usize)
}
fn luma(r: f32, g: f32, b: f32) -> f32 { 0.299 * r + 0.587 * g + 0.114 * b }
#[derive(Clone, Copy)]
pub enum PixelFormat { Rgb565, Yuyv, Grayscale }

pub fn load(path: &str) -> Result<Picture, String> {
    let d = std::fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
    parse(&d).map_err(|e| format!("{}: {}", path, e))
}

/// Decode an image already in memory (the wasm build gets files from the page).
pub fn parse(d: &[u8]) -> Result<Picture, String> {
    if d.starts_with(b"P6") { return ppm(d); }
    if d.starts_with(b"BM") { return bmp(d); }
    Err("unsupported image (use PPM P6 or 24/32-bit BMP; `sips -s format bmp in.png --out out.bmp`)".into())
}

fn ppm(d: &[u8]) -> Result<Picture, String> {
    let mut i = 2; let mut nums = Vec::new();
    while nums.len() < 3 {
        while i < d.len() && (d[i] as char).is_whitespace() { i += 1; }
        if i < d.len() && d[i] == b'#' { while i < d.len() && d[i] != b'\n' { i += 1; } continue; }
        let s = i; while i < d.len() && d[i].is_ascii_digit() { i += 1; }
        if s == i { return Err("ppm: bad header".into()); }
        nums.push(std::str::from_utf8(&d[s..i]).unwrap().parse::<u32>().map_err(|e| e.to_string())?);
    }
    if !d.get(i).is_some_and(u8::is_ascii_whitespace) { return Err("ppm: missing pixel separator".into()); }
    i += 1;
    let (w, h) = (nums[0], nums[1]);
    if w == 0 || h == 0 || w as u64 * h as u64 > MAX_PIXELS { return Err(format!("ppm: unreasonable size {}x{}", w, h)); }
    let n = (w as u64 * h as u64 * 3) as usize;
    let rgb = d.get(i..).and_then(|tail| tail.get(..n)).ok_or("ppm: truncated")?.to_vec();
    Ok(Picture { w, h, rgb })
}

fn bmp(d: &[u8]) -> Result<Picture, String> {
    if d.len() < 54 { return Err("bmp: truncated header".into()); }
    let u32le = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let off = u32le(10) as usize; let w = u32le(18) as i32; let h = u32le(22) as i32; let bpp = u16::from_le_bytes([d[28], d[29]]) as usize;
    if bpp != 24 && bpp != 32 { return Err(format!("bmp: {} bpp not supported", bpp)); }
    let (wu, hu, flip) = (w.unsigned_abs(), h.unsigned_abs(), h > 0);
    if wu == 0 || hu == 0 || wu as u64 * hu as u64 > MAX_PIXELS { return Err(format!("bmp: unreasonable size {}x{}", wu, hu)); }
    let stride = ((wu as usize * bpp / 8) + 3) & !3;
    // Row padding separates rows; no decoder read needs padding after the final row.
    let len = stride.checked_mul(hu as usize - 1).and_then(|n| n.checked_add(wu as usize * bpp / 8)).ok_or("bmp: pixel extent overflows")?;
    let pixels = d.get(off..).and_then(|tail| tail.get(..len)).ok_or("bmp: truncated")?;
    let mut rgb = vec![0u8; (wu as u64 * hu as u64 * 3) as usize];
    for y in 0..hu as usize {
        let src_row = if flip { hu as usize - 1 - y } else { y };
        for x in 0..wu as usize {
            let p = src_row * stride + x * bpp / 8;
            let o = (y * wu as usize + x) * 3;
            rgb[o] = pixels[p + 2]; rgb[o + 1] = pixels[p + 1]; rgb[o + 2] = pixels[p];
        }
    }
    Ok(Picture { w: wu, h: hu, rgb })
}

/// Nearest-neighbour resample to `w`x`h` and pack as YUYV (BT.601 full range).
pub fn to_yuyv(p: &Picture, w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (w * h * 2) as usize];
    let yuv = |r: u8, g: u8, b: u8| -> (u8, u8, u8) {
        let (r, g, b) = (r as f32, g as f32, b as f32);
        let y = luma(r, g, b);
        let u = -0.169 * r - 0.331 * g + 0.5 * b + 128.0;
        let v = 0.5 * r - 0.419 * g - 0.081 * b + 128.0;
        (y.clamp(0.0, 255.0) as u8, u.clamp(0.0, 255.0) as u8, v.clamp(0.0, 255.0) as u8)
    };
    for y in 0..h {
        for x2 in 0..w / 2 {
            let mut px = [(0u8, 0u8, 0u8); 2];
            for (k, pixel) in px.iter_mut().enumerate() {
                let x = x2 * 2 + k as u32;
                let (r, g, b) = p.pixel(x, y, w, h);
                *pixel = yuv(r, g, b);
            }
            let o = ((y * w + x2 * 2) * 2) as usize;
            out[o] = px[0].0; out[o + 1] = ((px[0].1 as u16 + px[1].1 as u16) / 2) as u8; out[o + 2] = px[1].0; out[o + 3] = ((px[0].2 as u16 + px[1].2 as u16) / 2) as u8;
        }
    }
    out
}

/// Crop an inclusive sensor-array window, mapping its coordinates onto the host image.
/// Nearest-neighbour scaling models geometry, not the sensor's optical black pixels or ISP.
pub fn crop(p: &Picture, [x0, y0, x1, y1]: [u32; 4], array_w: u32, array_h: u32) -> Option<Picture> {
    if !p.valid() || x0 > x1 || y0 > y1 || x1 >= array_w || y1 >= array_h { return None; }
    let left = (x0 as u64 * p.w as u64 / array_w as u64) as u32;
    let top = (y0 as u64 * p.h as u64 / array_h as u64) as u32;
    let right = (((x1 + 1) as u64 * p.w as u64).div_ceil(array_w as u64) as u32).min(p.w);
    let bottom = (((y1 + 1) as u64 * p.h as u64).div_ceil(array_h as u64) as u32).min(p.h);
    let mut rgb = Vec::with_capacity(((right - left) * (bottom - top) * 3) as usize);
    for y in top..bottom {
        rgb.extend_from_slice(&p.rgb[((y * p.w + left) * 3) as usize..((y * p.w + right) * 3) as usize]);
    }
    Some(Picture { w: right - left, h: bottom - top, rgb })
}

/// Nearest-neighbour sampling; RGB565 bytes are most-significant first.
pub fn sensor_frame(p: &Picture, w: u32, h: u32, format: PixelFormat) -> Option<Vec<u8>> {
    if !p.valid() || !valid_size(w, h) { return None; }
    if matches!(format, PixelFormat::Yuyv) { return (w.is_multiple_of(2)).then(|| to_yuyv(p, w, h)); }
    let mut out = Vec::with_capacity((w * h * if matches!(format, PixelFormat::Grayscale) { 1 } else { 2 }) as usize);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = p.pixel(x, y, w, h);
            if matches!(format, PixelFormat::Grayscale) {
                out.push(luma(r as f32, g as f32, b as f32).clamp(0.0, 255.0) as u8);
            } else {
                let rgb565 = ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3);
                out.extend_from_slice(&rgb565.to_be_bytes());
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formats_scaling_and_window() {
        let p = Picture { w: 2, h: 1, rgb: vec![255, 0, 0, 0, 255, 0] };
        assert_eq!(sensor_frame(&p, 2, 1, PixelFormat::Rgb565).unwrap(), [0xf8, 0, 0x07, 0xe0]);
        assert_eq!(sensor_frame(&p, 2, 1, PixelFormat::Grayscale).unwrap(), [76, 149]);
        assert_eq!(sensor_frame(&p, 2, 1, PixelFormat::Yuyv).unwrap(), [76, 63, 149, 138]);
        assert_eq!(sensor_frame(&p, 1, 2, PixelFormat::Rgb565).unwrap(), [0xf8, 0, 0xf8, 0]);
        let c = crop(&p, [1, 0, 1, 0], 2, 1).unwrap();
        assert_eq!(c.rgb, [0, 255, 0]);
        assert_eq!(sensor_frame(&c, 2, 1, PixelFormat::Rgb565).unwrap(), [7, 224, 7, 224]);
        assert!(crop(&p, [1, 0, 0, 0], 2, 1).is_none());
        assert!(sensor_frame(&p, 1, 1, PixelFormat::Yuyv).is_none());
        assert!(sensor_frame(&p, 0, 1, PixelFormat::Rgb565).is_none());
    }
}
