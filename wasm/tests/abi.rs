//! The C ABI the page drives, exercised natively: create, load, boot, run in slices, drain the
//! outbox — for the S3 with a board and for the C3 and C6 — and check the messages of the web protocol
//! (`docs/web-ui.md`) come out: `board`, `serial`, `stat`, a display frame, audio, the LED ring.
//! Needs the mask ROM ELFs like the goldens.
#[path = "../../tests/common.rs"]
mod common;
use esp32sim_wasm::*;

struct Emu(*mut esp32sim_wasm::Emu);
impl Emu {
    fn new(board: &str, flash_mb: u32, psram_mb: u32) -> Emu {
        // SAFETY: `board` is readable for the call and the returned pointer is checked for null.
        let e = unsafe { esp32sim_new(board.as_ptr(), board.len(), flash_mb, psram_mb) };
        assert!(!e.is_null(), "board {}", board); Emu(e)
    }
    fn load(&mut self, kind: u32, d: &[u8]) {
        // SAFETY: `&mut self` provides exclusive access and `d` is readable for the call.
        assert_eq!(unsafe { esp32sim_load(self.0, kind, d.as_ptr(), d.len()) }, 0, "load kind {}", kind);
    }
    fn load_file(&mut self, kind: u32, path: &str) { self.load(kind, &std::fs::read(common::root().join(path)).unwrap_or_else(|e| panic!("{}: {}", path, e))); }
    fn boot(&mut self) {
        // SAFETY: `&mut self` provides exclusive access to the live emulator.
        assert_eq!(unsafe { esp32sim_boot(self.0, 0) }, 0);
    }
    fn run(&mut self, cycles: u32) -> u32 {
        // SAFETY: `&mut self` provides exclusive access to the live emulator.
        unsafe { esp32sim_run(self.0, cycles, 0.0) }
    }
    fn text_in(&mut self, s: &str) {
        // SAFETY: `&mut self` provides exclusive access and `s` is readable for the call.
        unsafe { esp32sim_in_text(self.0, s.as_ptr(), s.len()) }
    }
    /// (kind, payload) since the last call: 1 = text, 2 = binary
    fn out(&mut self) -> Vec<(u32, Vec<u8>)> {
        // SAFETY: `&mut self` provides exclusive access to the live emulator.
        let n = unsafe { esp32sim_out_take(self.0) };
        (0..n).map(|i| {
            // SAFETY: `i` came from this drain, the emulator stays live, and no output mutation
            // occurs until the payload is copied.
            unsafe { let (k, p, l) = (esp32sim_out_kind(self.0, i), esp32sim_out_ptr(self.0, i), esp32sim_out_len(self.0, i)); (k, std::slice::from_raw_parts(p, l).to_vec()) }
        }).collect()
    }
}
impl Drop for Emu {
    fn drop(&mut self) {
        // SAFETY: This wrapper uniquely owns the live pointer and never uses it after Drop.
        unsafe { esp32sim_delete(self.0) }
    }
}

fn texts(msgs: &[(u32, Vec<u8>)]) -> Vec<String> { msgs.iter().filter(|m| m.0 == 1).map(|m| String::from_utf8_lossy(&m.1).to_string()).collect() }
fn has(ts: &[String], needle: &str) -> bool { ts.iter().any(|t| t.contains(needle)) }

#[test]
#[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn s3_atech_speaks_the_web_protocol() {
    let mut e = Emu::new("atech14", 8, 2);
    e.load(0, &std::fs::read(common::rom("esp32s3_rev0")).unwrap());
    e.load_file(1, "web/wasm/fw/public/atech-bootloader.bin"); e.load_file(2, "web/wasm/fw/public/atech-ptable.bin"); e.load_file(3, "web/wasm/fw/public/atech-firmware.bin");
    e.load_file(6, "web/wasm/fw/public/atech-script1.txt");
    // SAFETY: `e.0` is live and this test retains exclusive access to its wrapper.
    assert_eq!(unsafe { esp32sim_run(e.0, 1000, 0.0) }, 9, "running before boot is refused");
    e.boot();
    let mut all = e.out();
    assert!(has(&texts(&all), "\"t\":\"board\",\"name\":\"atech14\""), "{:?}", texts(&all));
    e.text_in("{\"t\":\"knob\",\"d\":\"1\"}");
    for _ in 0..30 { assert_eq!(e.run(24_000_000), 0); all.extend(e.out()); }   // 3 s: past the script's first button press and the knob
    let ts = texts(&all);
    assert!(has(&ts, "\"t\":\"serial\",\"src\":\"usb\"") && has(&ts, "ESP-ROM:esp32s3"), "the ROM banner arrived on the USB console");
    assert!(has(&ts, "\"t\":\"stat\"") && has(&ts, "\"insns\":"), "statistics");
    assert!(has(&ts, "\"t\":\"ring\",\"leds\":[["), "the WS2812 ring was pushed");
    let bins: Vec<u8> = all.iter().filter(|m| m.0 == 2).map(|m| m.1[0]).collect();
    assert!(bins.contains(&1) && bins.contains(&2), "a display frame (1) and audio (2) went out: kinds seen {:?}", bins);
    let frame = all.iter().find(|m| m.0 == 2 && m.1[0] == 1).unwrap();
    assert_eq!(&frame.1[1..5], &[160, 0, 80, 0], "the ST7735 frame is 160x80");
    // SAFETY: `e.0` stays live and neither query overlaps mutable access.
    assert!(unsafe { esp32sim_insns(e.0) } > 1e8 && unsafe { esp32sim_cpu_hz(e.0) } == 240e6);
}

#[test]
#[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn c3_speaks_the_web_protocol() {
    let mut e = Emu::new("esp32c3", 4, 0);
    e.load(0, &std::fs::read(common::rom("esp32c3_rev3")).unwrap());
    e.load_file(1, "web/wasm/fw/public/c3-hello-bootloader.bin"); e.load_file(2, "web/wasm/fw/public/c3-hello-ptable.bin"); e.load_file(3, "web/wasm/fw/public/c3-hello_world.bin");
    e.boot();
    let mut all = e.out();
    for _ in 0..20 { assert_eq!(e.run(16_000_000), 0); all.extend(e.out()); }   // 2 s
    let ts = texts(&all);
    assert!(has(&ts, "\"name\":\"none\""), "a bare module: {:?}", &ts[..ts.len().min(3)]);
    assert!(has(&ts, "\"src\":\"uart0\"") && has(&ts, "Hello world!"), "hello_world on UART0");
    assert!(has(&ts, "\"t\":\"stat\""));
    // SAFETY: `e.0` stays live and the query does not overlap mutable access.
    assert_eq!(unsafe { esp32sim_cpu_hz(e.0) }, 160e6);
    // SAFETY: `e.0` is live, names are readable, and null is valid with a zero argument length.
    assert_eq!(unsafe { esp32sim_observer(e.0, b"coverage".as_ptr(), 8, std::ptr::null(), 0) }, 0);
    // SAFETY: `e.0` is live, names are readable, and null is valid with a zero argument length.
    assert_eq!(unsafe { esp32sim_observer(e.0, b"nope".as_ptr(), 4, std::ptr::null(), 0) }, 1);
}

#[test]
#[ignore = "needs the ESP32-C6 mask ROM ELF"]
fn c6_speaks_the_web_protocol() {
    let mut e = Emu::new("esp32c6", 4, 0);
    e.load(0, &std::fs::read(common::rom("esp32c6_rev0")).unwrap());
    e.load_file(1, "web/wasm/fw/public/c6-hello-bootloader.bin"); e.load_file(2, "web/wasm/fw/public/c6-hello-ptable.bin"); e.load_file(3, "web/wasm/fw/public/c6-hello_world.bin");
    e.boot();
    let mut all = e.out();
    for _ in 0..20 { assert_eq!(e.run(16_000_000), 0); all.extend(e.out()); }   // 2 s
    let ts = texts(&all);
    assert!(has(&ts, "\"name\":\"none\""), "a bare module: {:?}", &ts[..ts.len().min(3)]);
    assert!(has(&ts, "\"src\":\"uart0\"") && has(&ts, "Hello world!"), "hello_world on UART0");
    assert!(has(&ts, "\"t\":\"stat\""));
    // SAFETY: `e.0` stays live and the query does not overlap mutable access.
    assert_eq!(unsafe { esp32sim_cpu_hz(e.0) }, 160e6);
}

#[test]
fn camera_binary_input_reports_boards_without_a_camera() {
    for board in ["none", "atech14", "waveshare-cam", "c3", "c6"] {
        let mut e = Emu::new(board, 4, 0);
        e.boot();
        let red = [3, 1, 0, 1, 0, 255, 0, 0, 255];
        // SAFETY: the emulator is live and red is readable for its length.
        unsafe { esp32sim_in_bin(e.0, red.as_ptr(), red.len()); }
        e.run(0);
        let messages = texts(&e.out());
        assert_eq!(has(&messages, "no camera on this board"), board != "waveshare-cam", "{board}");
    }
}
