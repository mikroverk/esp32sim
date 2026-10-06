use std::{path::PathBuf, process::Command};

#[test]
#[ignore = "set ESP32SIM_BLE_FIRMWARE_DIR to the Arduino builds and ESP32SIM_ROM_DIR to the ROM directory"]
fn external_ble_arduino_examples() {
    let firmware = PathBuf::from(std::env::var_os("ESP32SIM_BLE_FIRMWARE_DIR")
        .expect("set ESP32SIM_BLE_FIRMWARE_DIR to the Arduino builds: CHIP/EXAMPLE/.pio/build/BOARD/firmware.{elf,factory.bin}"));
    let roms = PathBuf::from(std::env::var_os("ESP32SIM_ROM_DIR").expect("set ESP32SIM_ROM_DIR to the S3/C3/C6 ROM ELF directory"));
    for (chip, board, rom) in [("s3", "esp32-s3-devkitc-1", "esp32s3_rev0_rom.elf"),
                              ("c3", "esp32-c3-devkitm-1", "esp32c3_rev3_rom.elf"),
                              ("c6", "esp32-c6-devkitc-1", "esp32c6_rev0_rom.elf")] {
        assert!(roms.join(rom).is_file(), "missing ROM input {}", roms.join(rom).display());
        for example in ["Server", "Notify", "Write", "Scan"] {
            for name in ["firmware.elf", "firmware.factory.bin"] {
                let file = firmware.join(chip).join(example).join(".pio/build").join(board).join(name);
                assert!(file.is_file(), "missing Arduino BLE input {}", file.display());
            }
        }
    }
    let output = std::env::temp_dir().join(format!("esp32sim-ble-{}", std::process::id()));
    let result = Command::new("python3").arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/evidence/ble-s3-c3-c6-2026-10-02/run.py"))
        .args(["--emulator", env!("CARGO_BIN_EXE_esp32sim"), "--firmware"]).arg(firmware)
        .arg("--roms").arg(roms).arg("--output").arg(&output).output().expect("python3 BLE evidence runner");
    assert!(result.status.success(), "Arduino BLE checks failed; logs in {}\n{}\n{}", output.display(),
        String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
}

#[test]
fn ble_script_without_enable_exits_unsuccessfully() {
    let script = std::env::temp_dir().join(format!("esp32sim-ble-invalid-{}.txt", std::process::id()));
    for line in ["0 ble conect", "0 ble connect"] {
        std::fs::write(&script, line).unwrap();
        let result = Command::new(env!("CARGO_BIN_EXE_esp32sim")).args(["--script", script.to_str().unwrap(), "--max-insns", "1", "--no-dump"])
            .output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("line 1"));
    }
    std::fs::remove_file(script).unwrap();
}

#[test]
fn c6_ble_without_elf_names_the_missing_symbol() {
    let result = Command::new(env!("CARGO_BIN_EXE_esp32sim")).args(["--chip", "c6", "--boot", "app", "--ble", "--max-insns", "1", "--no-dump"])
        .output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("requires ELF symbol esp_bt_controller_init"));
}

#[test]
fn full_ble_mode_is_c3_only_and_excludes_hci() {
    for args in [vec!["--chip", "s3", "--ble", "full"], vec!["--chip", "c6", "--ble", "full"], vec!["--chip", "c3", "--ble", "--ble", "full"], vec!["--chip", "c3", "--ble", "full", "--ble"]] {
        let result = Command::new(env!("CARGO_BIN_EXE_esp32sim")).args(args).output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("mutually exclusive"));
    }
}

#[test]
fn passive_observer_requires_full_ble() {
    let result = Command::new(env!("CARGO_BIN_EXE_esp32sim-c3")).arg("--ble-observe").output().unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("--ble-observe requires --ble full"));
}
