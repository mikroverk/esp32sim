//! Golden-output tests: the emulator must produce byte-identical console text, audio and
//! instruction counts for the committed demo firmware. This is the regression bar every
//! refactor and every speed change is held to (`docs/decisions.md`, "Performance").
//!
//! They need the ESP32-S3 mask ROM ELF, which ships with ESP-IDF and is not checked in, so they
//! are `#[ignore]`d: run `cargo test --release --workspace -- --include-ignored` (CI does; see
//! `tests/README.md`). Regenerate after an intentional change with `UPDATE_GOLDENS=1`.
#[path = "../../tests/common.rs"]
mod common;
use common::*;

const BIN: &str = env!("CARGO_BIN_EXE_esp32sim");
const BIN_C3: &str = env!("CARGO_BIN_EXE_esp32sim-c3");
const BIN_C6: &str = env!("CARGO_BIN_EXE_esp32sim-c6");
const FW: &str = "web/wasm/fw/public";

fn atech(extra: &[&str]) -> (Run, Vec<u8>) {
    let wav = tmp("atech.wav"); let rom = rom("esp32s3_rev0");
    let mut args: Vec<String> = ["--rom", rom.to_str().unwrap(), "--board", "atech14", "--boot", "rom", "--no-dump",
        "--bootloader", &format!("{FW}/atech-bootloader.bin"), "--ptable", &format!("{FW}/atech-ptable.bin"), "--app", &format!("{FW}/atech-firmware.bin"),
        "--script", &format!("{FW}/atech-script1.txt"), "--wav", wav.to_str().unwrap(), "--max-seconds", "5"].iter().map(|s| s.to_string()).collect();
    args.extend(extra.iter().map(|s| s.to_string()));
    let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let r = run(BIN, &args);
    let data = std::fs::read(&wav).expect("wav written");
    (r, data)
}

/// The Pocket Synth scenario: buttons, encoder, a serial command, the SID voice on I2S.
#[test] #[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn atech_script1() {
    let (r, wav) = atech(&[]);
    expect_text("atech-script1.console.txt", &r.stdout);
    expect_sha("atech-script1.wav.sha256", &wav);
    expect_u64("atech-script1.insns", r.insns);
}

/// `--no-jit` is the oracle: the block interpreter and the JIT must agree bit for bit.
#[test] #[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn atech_script1_no_jit() {
    let (r, wav) = atech(&["--no-jit"]);
    expect_text("atech-script1.console.txt", &r.stdout);
    expect_sha("atech-script1.wav.sha256", &wav);
    expect_u64("atech-script1.insns", r.insns);
}

/// The C64 SID jukebox (cRSID): a 6502 + SID emulated inside the emulated S3, 3 s of Commando.
#[test] #[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn atech_sid_jukebox() {
    let wav = tmp("sid.wav"); let rom = rom("esp32s3_rev0");
    let r = run(BIN, &["--rom", rom.to_str().unwrap(), "--board", "atech14", "--boot", "rom", "--no-dump",
        "--bootloader", &format!("{FW}/atech-bootloader.bin"), "--ptable", &format!("{FW}/atech-ptable.bin"), "--app", &format!("{FW}/atech-firmware.bin"),
        "--script", &format!("{FW}/atech-sid.txt"), "--wav", wav.to_str().unwrap(), "--max-seconds", "6"]);
    expect_text("atech-sid.console.txt", &r.stdout);
    expect_sha("atech-sid.wav.sha256", &std::fs::read(&wav).unwrap());
    expect_u64("atech-sid.insns", r.insns);
}

/// The Touch-LCD-4B energy panel in demo mode: PSRAM, LCD_CAM RGB frames, GT911 touch over I2C,
/// the ES8311 codec on I2S, swipes and a play tap from the script.
#[test] #[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn panel_sid() {
    let wav = tmp("panel.wav"); let rom = rom("esp32s3_rev0");
    let r = run(BIN, &["--rom", rom.to_str().unwrap(), "--board", "waveshare-lcd4b", "--boot", "rom", "--no-dump", "--flash-mb", "16", "--psram-mb", "8", "--console", "usb",
        "--bootloader", &format!("{FW}/panel-bootloader.bin"), "--ptable", &format!("{FW}/panel-ptable.bin"), "--app", &format!("{FW}/panel-demo.bin"),
        "--flash-at", &format!("0x610000={FW}/energydata.json"),
        "--script", &format!("{FW}/panel-sid.txt"), "--wav", wav.to_str().unwrap(), "--max-seconds", "7"]);
    expect_text("panel-sid.console.txt", &r.stdout);
    expect_sha("panel-sid.wav.sha256", &std::fs::read(&wav).unwrap());
    expect_u64("panel-sid.insns", r.insns);
}

/// Observers attached must not change the run (the block-path ones run at full speed), and
/// each must produce its report.
#[test] #[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn atech_script1_with_observers() {
    let (cov, vcd) = (tmp("cov.txt"), tmp("atech.vcd"));
    let (r, wav) = atech(&["--profile-blocks", "--coverage-file", cov.to_str().unwrap(), "--irq-latency", "--vcd", vcd.to_str().unwrap()]);
    expect_text("atech-script1.console.txt", &r.stdout);
    expect_sha("atech-script1.wav.sha256", &wav);
    expect_u64("atech-script1.insns", r.insns);
    for tag in ["[profile-blocks] top", "[coverage] ", "[irq-latency] per core", "[vcd] wrote"] { assert!(r.stderr.contains(tag), "missing {} in:\n{}", tag, r.stderr); }
    assert!(r.stderr.contains("core0 int9"), "the systimer line should show up in the latency table");
    let v = std::fs::read_to_string(&vcd).unwrap();
    assert!(v.starts_with("$timescale 1ps $end") && v.contains("gpio2 $end") && v.contains("core0_int9 $end"), "vcd header");
    let c = std::fs::read_to_string(&cov).unwrap();
    assert!(c.lines().count() > 5000, "coverage rows: {}", c.lines().count());
}

/// Stock ESP-IDF hello_world from the mask ROM through the bootloader into app_main, on UART0.
#[test] #[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn hello_world_s3() {
    let rom = rom("esp32s3_rev0");
    let r = run(BIN, &["--rom", rom.to_str().unwrap(), "--board", "none", "--boot", "rom", "--no-dump", "--console", "uart0",
        "--bootloader", &format!("{FW}/hello-bootloader.bin"), "--ptable", &format!("{FW}/hello-ptable.bin"), "--app", &format!("{FW}/hello_world.bin"), "--max-seconds", "3"]);
    assert!(r.stdout.contains("Hello world!"), "app_main never printed:\n{}", r.stdout);
    expect_text("hello-s3.console.txt", &r.stdout);
    expect_u64("hello-s3.insns", r.insns);
}

/// Through the RTC watchdog reset esp_restart() arms at the end of the countdown and back up
/// from the ROM: the chip-reset path (what survives, what the ROM reports).
#[test] #[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn hello_world_s3_reboot() {
    let rom = rom("esp32s3_rev0");
    let r = run(BIN, &["--rom", rom.to_str().unwrap(), "--board", "none", "--boot", "rom", "--no-dump", "--console", "uart0",
        "--bootloader", &format!("{FW}/hello-bootloader.bin"), "--ptable", &format!("{FW}/hello-ptable.bin"), "--app", &format!("{FW}/hello_world.bin"), "--max-seconds", "12"]);
    assert!(r.stderr.contains("[emu] chip reset at t="), "no reset seen:\n{}", r.stderr);
    assert!(r.stdout.matches("Hello world!").count() >= 2, "the app did not come back after the reset:\n{}", r.stdout);
    expect_text("hello-s3-reboot.console.txt", &r.stdout);
    expect_u64("hello-s3-reboot.insns", r.insns);
}

/// The block profile attributes time to symbols (the ROM's here), and the per-instruction
/// `--profile` (slow path, idle cores stepping) still agrees on the hottest function.
#[test] #[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn hello_world_s3_profiles() {
    let rom = rom("esp32s3_rev0");
    let base = ["--rom", rom.to_str().unwrap(), "--board", "none", "--boot", "rom", "--no-dump", "--console", "none",
        "--bootloader", &format!("{FW}/hello-bootloader.bin"), "--ptable", &format!("{FW}/hello-ptable.bin"), "--app", &format!("{FW}/hello_world.bin"), "--max-seconds", "1"];
    let mut a = base.to_vec(); a.push("--profile-blocks");
    let r = run(BIN, &a);
    let line = r.stderr.lines().skip_while(|l| !l.starts_with("[profile-blocks]")).nth(1).unwrap_or("");
    assert!(line.contains("ets_delay_us"), "hottest function should be the ROM's delay loop, got: {:?}", line);
    let mut b = base.to_vec(); b.push("--profile");
    let r = run(BIN, &b);
    assert!(r.stderr.contains("[profile] top 12 pcs"), "{}", r.stderr);
}

/// hello_world from the C3 mask ROM, with the MAC / reset cause / straps of the real module the
/// boot log was compared against (`hw/c3-hello-world-real.txt`, `docs/esp32c3.md`).
#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn hello_world_c3() {
    let rom = rom("esp32c3_rev3");
    let r = run(BIN_C3, &["--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4",
        "--mac", "3c:84:27:b6:a7:1c", "--reset-cause", "0x15", "--strap", "0xd",
        "--bootloader", &format!("{FW}/c3-hello-bootloader.bin"), "--ptable", &format!("{FW}/c3-hello-ptable.bin"), "--app", &format!("{FW}/c3-hello_world.bin"), "--max-seconds", "3"]);
    assert!(r.stdout.contains("Hello world!"), "app_main never printed:\n{}", r.stdout);
    expect_text("hello-c3.console.txt", &r.stdout);
    expect_u64("hello-c3.insns", r.insns);
    // the same run through the one binary
    let r2 = run(BIN, &["--chip", "c3", "--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4",
        "--mac", "3c:84:27:b6:a7:1c", "--reset-cause", "0x15", "--strap", "0xd",
        "--bootloader", &format!("{FW}/c3-hello-bootloader.bin"), "--ptable", &format!("{FW}/c3-hello-ptable.bin"), "--app", &format!("{FW}/c3-hello_world.bin"), "--max-seconds", "3"]);
    assert_eq!(r.stdout, r2.stdout); assert_eq!(r.insns, r2.insns);
}

/// The C3's esp_restart() path: a software CPU reset, back through the ROM with the right cause.
/// In this unmodeled RV32 path the reported count includes idle cycles and survives reset.
/// A reset charges only its executed partial round, so the 12 s count is 12 * 160 MHz;
/// charging a full reset round previously added device time without matching core work.
#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn hello_world_c3_reboot() {
    let rom = rom("esp32c3_rev3");
    let r = run(BIN_C3, &["--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4",
        "--mac", "3c:84:27:b6:a7:1c", "--reset-cause", "0x15", "--strap", "0xd",
        "--bootloader", &format!("{FW}/c3-hello-bootloader.bin"), "--ptable", &format!("{FW}/c3-hello-ptable.bin"), "--app", &format!("{FW}/c3-hello_world.bin"), "--max-seconds", "12"]);
    assert!(r.stderr.contains("[emu] chip reset at t="), "no reset seen:\n{}", r.stderr);
    assert!(r.stdout.matches("Hello world!").count() >= 2, "the app did not come back after the reset:\n{}", r.stdout);
    expect_text("hello-c3-reboot.console.txt", &r.stdout);
    expect_u64("hello-c3-reboot.insns", r.insns);
}

/// hello_world from the C6 mask ROM, with the MAC / reset cause / straps of the Waveshare
/// ESP32-C6-LCD-1.47 the boot log was compared against (`hw/c6-hello-world-real.txt`,
/// `docs/esp32c6.md`).
#[test] #[ignore = "needs the ESP32-C6 mask ROM ELF"]
fn hello_world_c6() {
    let rom = rom("esp32c6_rev0");
    let r = run(BIN_C6, &["--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4",
        "--mac", "dc:1e:d5:6e:8c:dc", "--reset-cause", "0x15", "--strap", "0x6e",
        "--bootloader", &format!("{FW}/c6-hello-bootloader.bin"), "--ptable", &format!("{FW}/c6-hello-ptable.bin"), "--app", &format!("{FW}/c6-hello_world.bin"), "--max-seconds", "3"]);
    assert!(r.stdout.contains("Hello world!"), "app_main never printed:\n{}", r.stdout);
    expect_text("hello-c6.console.txt", &r.stdout);
    expect_u64("hello-c6.insns", r.insns);
    // the same run through the one binary
    let r2 = run(BIN, &["--chip", "c6", "--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4",
        "--mac", "dc:1e:d5:6e:8c:dc", "--reset-cause", "0x15", "--strap", "0x6e",
        "--bootloader", &format!("{FW}/c6-hello-bootloader.bin"), "--ptable", &format!("{FW}/c6-hello-ptable.bin"), "--app", &format!("{FW}/c6-hello_world.bin"), "--max-seconds", "3"]);
    assert_eq!(r.stdout, r2.stdout); assert_eq!(r.insns, r2.insns);
}

/// The C6's esp_restart() path: a software CPU reset through LP_AON, back through the ROM with
/// the right cause and the ROM's `Saved PC` line.
/// As on the C3, partial-reset accounting preserves the 12 * 160 MHz reported count.
#[test] #[ignore = "needs the ESP32-C6 mask ROM ELF"]
fn hello_world_c6_reboot() {
    let rom = rom("esp32c6_rev0");
    let r = run(BIN_C6, &["--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4",
        "--mac", "dc:1e:d5:6e:8c:dc", "--reset-cause", "0x15", "--strap", "0x6e",
        "--bootloader", &format!("{FW}/c6-hello-bootloader.bin"), "--ptable", &format!("{FW}/c6-hello-ptable.bin"), "--app", &format!("{FW}/c6-hello_world.bin"), "--max-seconds", "12"]);
    assert!(r.stderr.contains("[emu] chip reset at t="), "no reset seen:\n{}", r.stderr);
    assert!(r.stdout.matches("Hello world!").count() >= 2, "the app did not come back after the reset:\n{}", r.stdout);
    assert!(r.stdout.contains("rst:0xc (SW_CPU)") && r.stdout.contains("Saved PC:0x4001975a"), "the second boot banner differs from silicon:\n{}", r.stdout);
    expect_text("hello-c6-reboot.console.txt", &r.stdout);
    expect_u64("hello-c6-reboot.insns", r.insns);
}

/// The IEEE 802.15.4 energy scanner on the Waveshare ESP32-C6-LCD-1.47 (its owner's firmware,
/// not in this repository): `ENERGY_SCAN_DIR` points at the built project. Pins the console and
/// the counts of energy scans, display writes and LED updates the board model reports.
#[test] #[ignore = "set ENERGY_SCAN_DIR=/path/to/energy_scan (built for esp32c6); needs the ESP32-C6 mask ROM ELF"]
fn external_energy_scan_c6() {
    let dir = std::env::var("ENERGY_SCAN_DIR").expect("ENERGY_SCAN_DIR=/path/to/energy_scan is required for this test");
    let b = format!("{}/build", dir); let rom = rom("esp32c6_rev0");
    let r = run(BIN_C6, &["--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4", "--board", "waveshare-c6-lcd147", "--no-dump",
        "--bootloader", &format!("{b}/bootloader/bootloader.bin"), "--ptable", &format!("{b}/partition_table/partition-table.bin"),
        "--app", &format!("{b}/energy_scan.bin"), "--elf", &format!("{b}/energy_scan.elf"), "--stub", "bb_init=0", "--max-seconds", "4"]);
    assert!(r.stdout.contains("libbtbb version"), "the PHY did not come up:\n{}", r.stdout);
    assert!(!r.stdout.contains("Guru Meditation"), "the app panicked:\n{}", r.stdout);
    let scans: u64 = r.stderr.lines().find(|l| l.starts_with("[emu] 802.15.4:")).and_then(|l| l.split_whitespace().nth(2)).and_then(|n| n.parse().ok()).unwrap_or(0);
    assert!(scans >= 50, "too few energy scans: {}\n{}", scans, r.stderr);
    let lcd = r.stderr.lines().find(|l| l.starts_with("[emu] lcd147:")).unwrap_or("");
    assert!(lcd.contains("RAMWR") && lcd.contains("updates"), "no board report:\n{}", r.stderr);
    expect_text("energy-scan-c6.console.txt", &r.stdout);
    expect_u64("energy-scan-c6.insns", r.insns);
}

/// The WiFi station (`examples/c6-wifi-station`, built with ESP-IDF 5.5.4 for each chip with the
/// emulator's default network and no display, `sdkconfig.ci.defaults`): the unmodified WiFi
/// library scans, finds the virtual access point, joins it through the WPA2 four-way handshake,
/// takes a DHCP lease and gets five of five gateway pings answered. The station's own lines are
/// the same on every chip and are what the board prints against a real network; the console and
/// the instruction count pin the run bit for bit, and the end-of-run counts pin the radio traffic
/// and the interrupts (on the C3 and C6 the instruction count is the cycle count, so a change in
/// the WiFi model's timing shows only in the interrupt count). This is the bar for changes to the
/// WiFi model.
fn wifi_station(chip: &str, bin: &str, rom_name: &str, extra: &[&str]) {
    let rom = rom(rom_name);
    let mut args = vec!["--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4", "--console", "usb", "--no-dump"];
    let (bl, pt, app) = (format!("{FW}/{chip}-wifi-bootloader.bin"), format!("{FW}/{chip}-wifi-ptable.bin"), format!("{FW}/{chip}-wifi_station.bin"));
    args.extend(["--bootloader", &bl, "--ptable", &pt, "--app", &app, "--wifi", "ssid=esp32sim,psk=esp32sim-pass", "--net", "none", "--max-seconds", "14"]);
    args.extend(extra);
    let r = run(bin, &args);
    assert!(!r.stdout.contains("Guru Meditation") && !r.stdout.contains("assert failed"), "the app panicked:\n{}", r.stdout);
    let station: String = r.stdout.lines().filter_map(|l| l.split_once("station: ").map(|(_, rest)| rest.trim_end_matches("\u{1b}[0m"))).map(|l| format!("{l}\n")).collect();
    assert!(station.contains("GOT_IP ip=10.0.2.15") && station.contains("PING done sent=5 received=5"), "the station did not get through:\n{}\n{}", station, r.stderr);
    expect_text(&format!("wifi-station-{chip}.station.txt"), &station);
    expect_text(&format!("wifi-station-{chip}.console.txt"), &r.stdout);
    let events = r.stderr.lines().find(|l| l.starts_with("[emu] stop:")).and_then(|l| l.rsplit_once("); ")).map_or("", |(_, e)| e);
    let report: String = std::iter::once(events).chain(r.stderr.lines().filter(|l| l.starts_with("[emu] wifi:") || l.starts_with("[emu] net:"))).map(|l| format!("{l}\n")).collect();
    expect_text(&format!("wifi-station-{chip}.report.txt"), &report);
    expect_u64(&format!("wifi-station-{chip}.insns"), r.insns);
}

#[test] #[ignore = "needs the ESP32-S3 mask ROM ELF"]
fn wifi_station_s3() { wifi_station("s3", BIN, "esp32s3_rev0", &["--board", "none"]); }

#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn wifi_station_c3() { wifi_station("c3", BIN_C3, "esp32c3_rev3", &[]); }

/// `bb_init` is the PHY's baseband calibration, which wants analog the emulator does not have;
/// without the ELF it is stubbed by address (`riscv32-esp-elf-nm`, see the example's README).
#[test] #[ignore = "needs the ESP32-C6 mask ROM ELF"]
fn wifi_station_c6() { wifi_station("c6", BIN_C6, "esp32c6_rev0", &["--stub", "0x4207df40=0"]); }

/// The guest controller owns the advertising schedule and handles every END IRQ.
#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF fetched by CI"]
fn ble_advertiser_c3() {
    let rom = rom("esp32c3_rev3");
    let r = run(BIN_C3, &["--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4", "--no-dump",
        "--bootloader", &format!("{FW}/c3-ble-bootloader.bin"), "--ptable", &format!("{FW}/c3-ble-ptable.bin"),
        "--app", &format!("{FW}/c3-ble-advertiser.bin"), "--ble", "full", "--ble-observe", "--max-seconds", "2"]);
    assert!(r.stdout.contains("advertiser: STARTED"), "{}", r.stdout);
    assert!(!r.stdout.contains("assert") && !r.stderr.contains("[ble-error]"), "{}\n{}", r.stdout, r.stderr);
    assert!(r.stderr.contains("0 exceptions"), "{}", r.stderr);
    let packets: Vec<_> = r.stderr.lines().filter(|line| line.starts_with("[ble-air]")).collect();
    assert!(packets.len() >= 51 && packets.len().is_multiple_of(3), "must wrap the 16-entry event table");
    for event in packets.as_chunks::<3>().0 {
        for (line, channel) in event.iter().zip([37, 38, 39]) {
            assert!(line.contains(&format!("channel={channel} type=ADV_SCAN_IND")), "{line}");
            assert!(line.contains("name=\"esp32sim\"") && line.contains("service=180f"), "{line}");
        }
    }
    expect_text("ble-advertiser-c3.console.txt", &r.stdout);
    let events = r.stderr.lines().find(|l| l.starts_with("[emu] stop:")).and_then(|l| l.rsplit_once("); ")).map_or("", |(_, e)| e);
    let report: String = std::iter::once(events).chain(r.stderr.lines().filter(|l| l.starts_with("  core") || l.starts_with("[ble-config]")))
        .map(|l| format!("{l}\n")).collect();
    expect_text("ble-advertiser-c3.report.txt", &report);
    expect_text("ble-advertiser-c3.observer.txt", &(packets.join("\n") + "\n"));
    expect_u64("ble-advertiser-c3.insns", r.insns);
}


fn crypto_tls(chip: &str, bin: &str, rom_name: &str) {
    let rom = rom(rom_name);
    let r = run(bin, &["--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4", "--no-dump",
        "--bootloader", &format!("{FW}/{chip}-crypto-tls-bootloader.bin"),
        "--ptable", &format!("{FW}/{chip}-crypto-tls-ptable.bin"), "--app", &format!("{FW}/{chip}-crypto-tls.bin"),
        "--wifi", "ssid=esp32sim,psk=esp32sim-pass", "--net", "none", "--max-seconds", "10"]);
    assert!(r.stdout.contains("TLS GOT_IP 10.0.2.15"), "{}", r.stdout);
    assert!(r.stdout.contains("TLS PASS TLSv1.2 TLS-ECDHE-PSK-WITH-AES-128-CBC-SHA256 bidirectional=256"), "{}", r.stdout);
    assert!(!r.stdout.contains("FAIL") && !r.stdout.contains("assert") && r.stderr.contains("0 exceptions"), "{}\n{}", r.stdout, r.stderr);
    expect_text(&format!("crypto-tls-{chip}.console.txt"), &r.stdout);
    let events = r.stderr.lines().find(|l| l.starts_with("[emu] stop:")).and_then(|l| l.rsplit_once("); ")).map_or("", |(_, e)| e);
    let report: String = std::iter::once(events).chain(r.stderr.lines().filter(|l| l.starts_with("  core") || l.starts_with("[emu] wifi:") || l.starts_with("[emu] net:")))
        .map(|l| format!("{l}\n")).collect();
    expect_text(&format!("crypto-tls-{chip}.report.txt"), &report);
}

#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF fetched by CI"]
fn crypto_tls_c3() { crypto_tls("c3", BIN_C3, "esp32c3_rev3"); }

#[test] #[ignore = "needs the ESP32-C6 mask ROM ELF fetched by CI"]
fn crypto_tls_c6() { crypto_tls("c6", BIN_C6, "esp32c6_rev0"); }
