//! Opt-in end-to-end checks against an installed, unmodified esptool release.
//!
//! These are named `external_` and ignored because they need esptool plus Espressif's mask-ROM
//! ELFs. The firmware images are the tracked golden-test assets, not generated fixtures.
#[path = "../../tests/common.rs"]
mod common;

use common::{rom, root, tmp};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_esp32sim");
const TESTED_ESPTOOL_VERSION: &str = "4.8.1";
const FW: &str = "web/wasm/fw/public";

fn python() -> String {
    std::env::var("ESPTOOL_PYTHON").unwrap_or_else(|_| "python3".into())
}

fn tail(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(2000)..]).to_string()
}

fn esptool(args: &[&str]) -> Output {
    Command::new(python())
        .args(["-m", "esptool"])
        .args(args)
        .env("ESPTOOL_STUB_VERSION", "2")
        .current_dir(root())
        .output()
        .unwrap_or_else(|error| panic!("start esptool: {error}"))
}

fn require_tested_esptool() {
    let output = esptool(&["version"]);
    assert!(
        output.status.success(),
        "esptool version failed: {}",
        tail(&output.stderr)
    );
    let version = String::from_utf8_lossy(&output.stdout);
    assert!(version.contains(TESTED_ESPTOOL_VERSION),
        "external lifecycle tests are pinned to esptool {TESTED_ESPTOOL_VERSION}, got {version:?}; set ESPTOOL_PYTHON to its Python environment");
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_for_listener(child: &mut Child, port: u16) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("emulator exited before listening: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "emulator did not listen on port {port}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_exit(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "download-mode emulator did not exit after esptool:\n{}",
                tail(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn lifecycle(chip: &str, rom_name: &str, prefix: &str) {
    require_tested_esptool();
    let state = tmp(&format!("esptool-{chip}-flash.bin"));
    let _ = std::fs::remove_file(&state);
    let port = free_port();
    let socket = format!("socket://127.0.0.1:{port}");
    let listen = format!("127.0.0.1:{port}");

    let mut emulator = Command::new(BIN)
        .args([
            "--chip",
            chip,
            "--board",
            "none",
            "--boot",
            "download",
            "--flash-mb",
            "4",
            "--flash-state",
        ])
        .arg(&state)
        .args(["--uart-tcp", &listen, "--console", "none", "--no-dump"])
        .current_dir(root())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_listener(&mut emulator, port);

    let bootloader = root().join(format!("{FW}/{prefix}-hello-bootloader.bin"));
    let ptable = root().join(format!("{FW}/{prefix}-hello-ptable.bin"));
    let app = root().join(format!("{FW}/{prefix}-hello_world.bin"));
    let flash = esptool(&[
        "--chip",
        chip,
        "--port",
        &socket,
        "--before",
        "no_reset",
        "--after",
        "no_reset",
        "write_flash",
        "0x0",
        bootloader.to_str().unwrap(),
        "0x8000",
        ptable.to_str().unwrap(),
        "0x10000",
        app.to_str().unwrap(),
    ]);
    let verified = String::from_utf8_lossy(&flash.stdout)
        .matches("Hash of data verified.")
        .count();
    if !flash.status.success() || verified != 3 {
        let _ = emulator.kill();
        let emulator_output = emulator.wait_with_output().unwrap();
        panic!(
            "esptool flash failed or verified {verified}/3 images:\n{}\n{}\nemulator:\n{}",
            tail(&flash.stdout),
            tail(&flash.stderr),
            tail(&emulator_output.stderr)
        );
    }
    let download = wait_for_exit(emulator);
    assert!(
        download.status.success(),
        "download emulator failed:\n{}",
        tail(&download.stderr)
    );

    let rom = rom(rom_name);
    let boot = Command::new(BIN)
        .args(["--chip", chip, "--board", "none", "--boot", "rom", "--rom"])
        .arg(rom)
        .args(["--flash-mb", "4", "--flash-state"])
        .arg(&state)
        .args(["--console", "uart0", "--max-seconds", "2", "--no-dump"])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(
        boot.status.success(),
        "normal boot failed:\n{}",
        tail(&boot.stderr)
    );
    assert!(
        String::from_utf8_lossy(&boot.stdout).contains("Hello world!"),
        "flashed application did not boot:\n{}",
        tail(&boot.stdout)
    );
    std::fs::remove_file(state).unwrap();
}

#[test]
#[ignore = "needs esptool 4.8.1 (ESPTOOL_PYTHON) and the ESP32-C3 mask ROM ELF"]
fn external_esptool_c3_v2_lifecycle() {
    lifecycle("esp32c3", "esp32c3_rev3", "c3");
}

#[test]
#[ignore = "needs esptool 4.8.1 (ESPTOOL_PYTHON) and the ESP32-C6 mask ROM ELF"]
fn external_esptool_c6_v2_lifecycle() {
    lifecycle("esp32c6", "esp32c6_rev0", "c6");
}
