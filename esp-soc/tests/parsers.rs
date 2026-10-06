//! Anything a user or a page can hand the loaders must come back as an error, never a panic:
//! random bytes, truncations, headers that point past the end.
use esp_soc::{elf, image, picture};

struct Rng(u64);
impl Rng { fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 } fn bytes(&mut self, n: usize) -> Vec<u8> { (0..n).map(|_| self.next() as u8).collect() } }

fn no_panic(name: &str, f: &dyn Fn(&[u8]) -> bool, inputs: &[Vec<u8>]) {
    for (i, input) in inputs.iter().enumerate() {
        let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(input))).is_ok();
        assert!(ok, "{} panicked on input #{} ({} bytes: {:02x?}...)", name, i, input.len(), &input[..input.len().min(16)]);
    }
}

fn inputs() -> Vec<Vec<u8>> {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let mut v: Vec<Vec<u8>> = vec![vec![], vec![0], vec![0xff; 3], vec![0; 64]];
    for _ in 0..300 { let n = (r.next() % 4096) as usize; v.push(r.bytes(n)); }
    // plausible headers with lengths and offsets pointing anywhere
    let mut elf = b"\x7fELF\x01\x01\x01\x00".to_vec(); elf.resize(52, 0);
    for i in 0..40 { let mut e = elf.clone(); let k = r.next() as usize % e.len(); e[k] = r.next() as u8; e[28] = (r.next() % 256) as u8; e[44] = (r.next() % 256) as u8; v.push(e); if i % 2 == 0 { v.push(elf[..r.next() as usize % 52].to_vec()); } }
    for _ in 0..100 {
        let mut e = elf_header();
        for off in [28, 32] { put_u32(&mut e, off, r.next() as u32); }
        for off in [42, 44, 46, 48, 50] { e[off..off + 2].copy_from_slice(&(r.next() as u16).to_le_bytes()); }
        v.push(e);
    }
    let mut img = vec![0xe9u8]; img.resize(24, 0); img[1] = 3;
    for _ in 0..40 { let mut e = img.clone(); let k = r.next() as usize % e.len(); e[k] = r.next() as u8; let n = r.next() as usize % 200; e.extend(r.bytes(n)); v.push(e); }
    let mut ppm = b"P6\n4 4\n255\n".to_vec(); ppm.extend(r.bytes(20));
    v.push(ppm); v.push(b"P6\n99999999 99999999\n255\n".to_vec()); v.push(b"BM".to_vec());
    let mut bmp = b"BM".to_vec(); bmp.resize(54, 0); bmp[18] = 200; bmp[22] = 200; v.push(bmp);
    v
}

#[test] fn elf_parse_never_panics() { no_panic("elf::parse", &|d| elf::parse(d).is_ok(), &inputs()); }
#[test] fn image_parse_never_panics() { no_panic("image::parse", &|d| image::parse(d).is_ok(), &inputs()); }
#[test] fn picture_parse_never_panics() { no_panic("picture::parse", &|d| picture::parse(d).is_ok(), &inputs()); }

fn put_u32(d: &mut [u8], off: usize, value: u32) { d[off..off + 4].copy_from_slice(&value.to_le_bytes()); }
fn elf_header() -> Vec<u8> {
    let mut d = b"\x7fELF\x01\x01\x01\x00".to_vec();
    d.resize(52, 0);
    d[42] = 32;
    d[46] = 40;
    d
}

#[test]
fn elf_rejects_invalid_header_tables() {
    let mut d = elf_header();
    put_u32(&mut d, 32, 1000);
    d[48] = 1;
    assert!(elf::parse(&d).is_err()); // The original 52-byte section-table panic.
    for (offset, size, count, minimum) in [(28, 42, 44, 32), (32, 46, 48, 40)] {
        let mut d = elf_header();
        d.resize(92, 0);
        d[count] = 1;
        put_u32(&mut d, offset, 52);
        d[size] = minimum - 1;
        assert!(elf::parse(&d).is_err());
        d[size] = minimum;
        put_u32(&mut d, offset, u32::MAX);
        assert!(elf::parse(&d).is_err());
    }
}

#[test]
fn elf_validates_load_payloads_but_allows_pure_bss_segments() {
    let mut d = elf_header();
    d.resize(88, 0);
    put_u32(&mut d, 28, 52);
    d[44] = 1;
    put_u32(&mut d, 52, 1); // PT_LOAD.
    put_u32(&mut d, 56, 84);
    put_u32(&mut d, 68, 4);
    put_u32(&mut d, 72, 4);
    assert_eq!(elf::parse(&d).unwrap().segments[0].data.len(), 4);
    assert!(elf::parse(&d[..87]).is_err());
    put_u32(&mut d, 56, u32::MAX);
    assert!(elf::parse(&d).is_err());
    put_u32(&mut d, 68, 0);
    let e = elf::parse(&d).unwrap();
    assert!(e.segments[0].data.is_empty());
    assert_eq!(e.segments[0].memsz, 4);
}

fn elf_with_symbols() -> Vec<u8> {
    let mut d = elf_header();
    d.resize(191, 0);
    put_u32(&mut d, 32, 52);
    d[48] = 3;
    put_u32(&mut d, 96, 2); // SHT_SYMTAB, one ELF32 symbol.
    put_u32(&mut d, 108, 172);
    put_u32(&mut d, 112, 16);
    put_u32(&mut d, 116, 2); // sh_link -> string table.
    put_u32(&mut d, 128, 16);
    put_u32(&mut d, 136, 3); // SHT_STRTAB.
    put_u32(&mut d, 148, 188);
    put_u32(&mut d, 152, 3);
    put_u32(&mut d, 172, 1); // st_name -> "f".
    put_u32(&mut d, 176, 0x4000_0000);
    d[184] = 2; // STT_FUNC.
    d[188..].copy_from_slice(b"\0f\0");
    d
}

#[test]
fn elf_bounds_symbol_records_and_strings_to_their_sections() {
    let d = elf_with_symbols();
    assert_eq!(elf::parse(&d).unwrap().by_name["f"], 0x4000_0000);
    for (off, value) in [(108, u32::MAX), (148, u32::MAX), (128, 1)] {
        let mut broken = d.clone();
        put_u32(&mut broken, off, value);
        assert!(elf::parse(&broken).is_err(), "accepted invalid field at {off}");
    }
    for (off, value) in [(152, 2), (172, 3)] {
        let mut broken_name = d.clone();
        put_u32(&mut broken_name, off, value);
        let e = elf::parse(&broken_name).unwrap();
        assert!(e.symbols.is_empty());
        assert!(e.by_name.is_empty());
    }
}

#[test]
fn elf_allows_unnamed_symbols_with_an_empty_string_table() {
    let mut d = elf_with_symbols();
    put_u32(&mut d, 148, u32::MAX);
    put_u32(&mut d, 152, 0);
    put_u32(&mut d, 172, 0);
    let e = elf::parse(&d).unwrap();
    assert!(e.by_name.is_empty());
    assert!(e.symbols.is_empty());
}

#[test]
fn elf_skips_section_names_outside_the_string_table() {
    let mut d = elf_with_symbols();
    d[50] = 2;
    put_u32(&mut d, 96, 1); // PROGBITS section with a load address.
    put_u32(&mut d, 104, 0x3fc0_0000);
    for name in [3, u32::MAX] {
        put_u32(&mut d, 92, name);
        assert!(elf::parse(&d).unwrap().sections.is_empty());
    }
}

#[test]
fn app_image_rejects_truncated_and_overflowing_payloads() {
    let mut d = vec![0; 36];
    d[0] = 0xe9;
    d[1] = 1;
    put_u32(&mut d, 28, 4);
    assert!(image::parse(&d).is_ok());
    assert!(image::parse(&d[..35]).is_err());
    put_u32(&mut d, 28, u32::MAX);
    assert!(image::parse(&d).is_err());
}

#[test]
fn bmp_validates_the_pixel_extent_before_allocating() {
    let mut d = vec![0; 58];
    d[..2].copy_from_slice(b"BM");
    put_u32(&mut d, 10, 54);
    put_u32(&mut d, 18, 1);
    put_u32(&mut d, 22, 1);
    d[28] = 24;
    d[54..57].copy_from_slice(&[3, 2, 1]);
    assert_eq!(picture::parse(&d).unwrap().rgb, [1, 2, 3]);
    assert_eq!(picture::parse(&d[..57]).unwrap().rgb, [1, 2, 3]);
    assert!(picture::parse(&d[..56]).is_err());
    put_u32(&mut d, 10, u32::MAX);
    assert!(picture::parse(&d).is_err());
    put_u32(&mut d, 10, 54);
    put_u32(&mut d, 18, 8192);
    put_u32(&mut d, 22, 8192);
    assert!(picture::parse(&d).is_err());
}

#[test]
fn ppm_requires_one_separator_without_eating_pixel_whitespace() {
    assert!(picture::parse(b"P6\n1 1\n255").is_err());
    assert!(picture::parse(b"P6\n1 1\n255#abc").is_err());
    assert_eq!(picture::parse(b"P6\n1 1\n255\n \n\t").unwrap().rgb, b" \n\t");
}

/// Real images still parse: a truncated one must fail, not panic, and the whole one must parse.
#[test]
fn committed_images_and_their_truncations() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
    let app = std::fs::read(root.join("web/wasm/fw/public/hello_world.bin")).unwrap();
    assert!(image::parse(&app).is_ok());
    let cuts: Vec<Vec<u8>> = (0..app.len().min(4096)).step_by(37).map(|n| app[..n].to_vec()).collect();
    no_panic("image::parse (truncated)", &|d| image::parse(d).is_ok(), &cuts);
}

#[test]
fn camera_rgba_and_crop_never_panic() {
    for (w, h) in [(0, 0), (1, 1), (2, 3), (4096, 4096), (u32::MAX, u32::MAX)] {
        no_panic("picture::from_rgba", &|d| picture::Picture::from_rgba(w, h, d).is_some(), &inputs());
    }
    let p = picture::Picture { w: 1, h: 1, rgb: vec![1, 2, 3] };
    for window in [[0; 4], [0, 0, u32::MAX, u32::MAX], [1, 1, 0, 0]] {
        for size in [0, 1, u32::MAX] { let _ = picture::crop(&p, window, size, size); }
    }
    assert_eq!(picture::rgba_len(4096, 4096), None);
}
