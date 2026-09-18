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

fn python_module(module: &str, args: &[&str]) -> Output {
    Command::new(python())
        .args(["-m", module])
        .args(args)
        .env("ESPTOOL_STUB_VERSION", "2")
        .current_dir(root())
        .output()
        .unwrap_or_else(|error| panic!("start {module}: {error}"))
}

fn esptool(args: &[&str]) -> Output {
    python_module("esptool", args)
}
fn espefuse(args: &[&str]) -> Output {
    python_module("espefuse", args)
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

fn start_download(chip: &str, state_flag: &str, state: &std::path::Path) -> (Child, String) {
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
            state_flag,
        ])
        .arg(state)
        .args(["--uart-tcp", &listen, "--console", "none", "--no-dump"])
        .current_dir(root())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for_listener(&mut emulator, port);
    (emulator, socket)
}

fn stop_download(mut child: Child) -> Output {
    let _ = child.kill();
    child.wait_with_output().unwrap()
}

fn lifecycle(chip: &str, rom_name: &str, prefix: &str) {
    require_tested_esptool();
    let state = tmp(&format!("esptool-{chip}-flash.bin"));
    let _ = std::fs::remove_file(&state);
    let (mut emulator, socket) = start_download(chip, "--flash-state", &state);

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

fn efuse_lifecycle(chip: &str, expected_identity: &str) {
    require_tested_esptool();
    let state = tmp(&format!("espefuse-{chip}.bin"));
    let _ = std::fs::remove_file(&state);
    let (emulator, socket) = start_download(chip, "--efuse-state", &state);
    let base = ["--chip", chip, "--port", &socket, "--before", "no_reset"];

    let summary = espefuse(&[base.as_slice(), &["summary"]].concat());
    assert!(
        summary.status.success(),
        "espefuse summary failed:\n{}\n{}",
        tail(&summary.stdout),
        tail(&summary.stderr)
    );
    assert!(
        String::from_utf8_lossy(&summary.stdout).contains(expected_identity),
        "espefuse reported the wrong identity:\n{}",
        tail(&summary.stdout)
    );

    let burn = espefuse(
        &[
            base.as_slice(),
            &["--do-not-confirm", "burn_bit", "BLOCK3", "0"],
        ]
        .concat(),
    );
    if !burn.status.success() {
        let emulator_output = stop_download(emulator);
        panic!(
            "espefuse burn failed:\n{}\n{}\nemulator:\n{}",
            tail(&burn.stdout),
            tail(&burn.stderr),
            tail(&emulator_output.stderr)
        );
    }
    assert!(
        String::from_utf8_lossy(&burn.stdout).contains("BURN BLOCK3  - OK"),
        "burn was not verified:\n{}",
        tail(&burn.stdout)
    );
    stop_download(emulator);

    let (emulator, socket) = start_download(chip, "--efuse-state", &state);
    let dump = espefuse(&[
        "--chip", chip, "--port", &socket, "--before", "no_reset", "dump",
    ]);
    stop_download(emulator);
    assert!(
        dump.status.success(),
        "espefuse dump failed:\n{}\n{}",
        tail(&dump.stdout),
        tail(&dump.stderr)
    );
    let dump = String::from_utf8_lossy(&dump.stdout);
    let block3 = dump.lines().find(|line| line.starts_with("BLOCK_USR_DATA"));
    assert!(
        block3.is_some_and(|line| line.contains("dump: 00000001 ")),
        "BLOCK3 burn did not persist:\n{}",
        tail(dump.as_bytes())
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

#[test]
#[ignore = "needs espefuse 4.8.1 (ESPTOOL_PYTHON) and the ESP32-C3 mask ROM ELF"]
fn external_espefuse_c3_persistence() {
    efuse_lifecycle("esp32c3", "60:55:f9:00:11:22");
}

#[test]
#[ignore = "needs espefuse 4.8.1 (ESPTOOL_PYTHON) and the ESP32-C6 mask ROM ELF"]
fn external_espefuse_c6_persistence() {
    efuse_lifecycle("esp32c6", "dc:1e:d5:ff:fe:6e:8c:dc");
}
