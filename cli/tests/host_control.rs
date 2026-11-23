//! The host-side run controls: `--exit-on`, `--timeout`, `--inject` / `--inject-on`.
#[path = "../../tests/common.rs"]
mod common;
use common::*;
use std::process::Command;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_esp32sim");
const FW: &str = "web/wasm/fw/public";

fn c3_hello(extra: &[&str]) -> Vec<String> {
    let rom = rom("esp32c3_rev3");
    let mut a: Vec<String> = ["--chip", "c3", "--rom", rom.to_str().unwrap(), "--boot", "rom", "--flash-mb", "4", "--strap", "0xd", "--no-dump",
        "--bootloader", &format!("{FW}/c3-hello-bootloader.bin"), "--ptable", &format!("{FW}/c3-hello-ptable.bin"), "--app", &format!("{FW}/c3-hello_world.bin")]
        .iter().map(|s| s.to_string()).collect();
    a.extend(extra.iter().map(|s| s.to_string()));
    a
}

fn emulated_seconds(stderr: &str) -> f64 {
    let line = stderr.lines().find(|l| l.starts_with("[emu] stop:")).unwrap_or_else(|| panic!("no stop line:\n{stderr}"));
    let i = line.find("emulated ").unwrap() + "emulated ".len();
    line[i..].split('s').next().unwrap().parse().unwrap()
}

/// `--exit-on` ends the run with status 0 as soon as the text is printed, well before `--max-seconds`; a text that
/// never comes ends the run at `--max-seconds` with status 1.
#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn exit_on_stops_the_run_when_the_console_prints_the_text() {
    let out = Command::new(BIN).args(c3_hello(&["--exit-on", "Restarting in 8 seconds", "--max-seconds", "30"])).current_dir(root()).output().unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stdout.contains("Restarting in 8 seconds") && !stdout.contains("Restarting in 7 seconds"), "{stdout}");
    // hello_world prints on USB-Serial/JTAG and UART0; whichever stream shows the text first ends the run
    assert!(stderr.contains("[emu] --exit-on \"Restarting in 8 seconds\" matched on "), "{stderr}");
    assert!(emulated_seconds(&stderr) < 3.0, "{stderr}");
    let out = Command::new(BIN).args(c3_hello(&["--exit-on", "never printed", "--max-seconds", "1"])).current_dir(root()).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
}

/// `--timeout` counts wall-clock time and ends the run gracefully (the report is printed) with status 124.
#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn timeout_ends_the_run_on_wall_clock_time_with_its_own_status() {
    let t0 = Instant::now();
    let out = Command::new(BIN).args(c3_hello(&["--timeout", "300ms", "--realtime"])).current_dir(root()).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(124), "{stderr}");
    assert!(stderr.contains("[emu] --timeout: the wall-clock time ran out") && stderr.contains("[emu] stop: Halted"), "{stderr}");
    assert!(t0.elapsed() < Duration::from_secs(20));
    // paced to wall time, 300 ms of it is at most a little more of emulated time
    assert!(emulated_seconds(&stderr) < 1.0, "{stderr}");
}

/// The payload of `--inject` reaches UART0's receive FIFO when its `--inject-on` trigger has been printed: the ROM's
/// `uart_rx_one_char` path is not used by hello_world, so the check is the script log of the chain.
#[test] #[ignore = "needs the ESP32-C3 mask ROM ELF"]
fn inject_on_waits_for_its_trigger_then_writes_the_payload() {
    let out = Command::new(BIN).args(c3_hello(&["--inject-on", "Restarting in 9 seconds", "--inject", "ab\\r\\n", "--inject", "first",
        "--exit-on", "Restarting in 7 seconds", "--max-seconds", "30"])).current_dir(root()).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let lines: Vec<&str> = stderr.lines().filter(|l| l.starts_with("[script]")).collect();
    assert_eq!(lines.len(), 3, "{stderr}");
    assert!(lines[0].starts_with("[script] t=0.000s Uart(0, \"first\")"), "{lines:?}");
    assert!(lines[1].contains("wait uart0 matched \"Restarting in 9 seconds\""), "{lines:?}");
    assert!(lines[2].contains("Uart(0, \"ab\\r\\n\")"), "{lines:?}");
}

/// A console REPL reads what `--inject` writes: the P4 USB-MSC example's `esp32p4>` prompt is the trigger, `help`
/// the payload, and the command list it prints ends the run through `--exit-on`.
#[test] #[ignore = "needs the ESP32-P4 mask ROM ELF"]
fn inject_reaches_the_p4_console_repl_after_its_prompt() {
    for name in ["p4-usbmsc-bootloader.bin", "p4-usbmsc-ptable.bin", "p4-usbmsc.bin"] {
        assert!(root().join(FW).join(name).exists(), "{FW}/{name} is missing: tools/fetch-p4-firmware.sh fetches it");
    }
    let card = tmp("inject-card.img");
    std::fs::File::create(&card).unwrap().set_len(64 << 20).unwrap();   // a blank card: the example formats it
    let rom = rom("esp32p4_rev0");
    let out = Command::new(BIN).args(["--chip", "p4", "--rom", rom.to_str().unwrap(), "--flash-mb", "4", "--no-dump",
        "--bootloader", &format!("{FW}/p4-usbmsc-bootloader.bin"), "--ptable", &format!("{FW}/p4-usbmsc-ptable.bin"), "--app", &format!("{FW}/p4-usbmsc.bin"),
        "--sd", card.to_str().unwrap(), "--inject-on", "esp32p4>", "--inject", "help\\n", "--exit-on", "Expose Storage", "--max-seconds", "10"])
        .current_dir(root()).output().unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{stderr}\n{stdout}");
    assert!(stderr.contains("wait uart0 matched \"esp32p4>\"") && stderr.contains("Uart(0, \"help\\n\")"), "{stderr}");
    assert!(stdout.contains("show storage size and sector size"), "{stdout}");
    assert!(emulated_seconds(&stderr) < 5.0, "{stderr}");
}
