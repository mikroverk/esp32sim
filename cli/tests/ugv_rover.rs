use std::{path::PathBuf, process::Command};

/// Waveshare's ugv_base_ros firmware (waveshareteam/ugv_base_ros at 2e7df97, GPL-3.0, built
/// unmodified with PlatformIO: bootloader.bin, partitions.bin, firmware.bin) on the
/// `waveshare-general-driver` board. Not redistributed here.
#[test]
#[ignore = "set UGV_BASE_FW_DIR to a ugv_base_ros PlatformIO build and ESP32SIM_ROM_DIR to esp32_rev300_rom.elf"]
fn external_ugv_base_drives_and_reports_odometry() {
    let dir = PathBuf::from(std::env::var("UGV_BASE_FW_DIR")
        .expect("set UGV_BASE_FW_DIR to ugv_base_ros's .pio/build/<env> (bootloader.bin, partitions.bin, firmware.bin)"));
    let rom = esp_soc::rom::require("esp32_rev300_rom.elf");
    let script = std::env::temp_dir().join(format!("ugv-rover-{}.txt", std::process::id()));
    // The firmware's T:1 is a closed-loop speed command in m/s; it streams T:1001 feedback lines.
    std::fs::write(&script, "0 waituart0 30 \"T\":1001\n0 uart0 {\"T\":1,\"L\":0.3,\"R\":-0.2}\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_esp32sim"))
        .args(["--chip", "esp32", "--board", "waveshare-general-driver", "--boot", "rom", "--rom"]).arg(&rom)
        .arg("--bootloader").arg(dir.join("bootloader.bin"))
        .arg("--ptable").arg(dir.join("partitions.bin"))
        .arg("--app").arg(dir.join("firmware.bin"))
        .arg("--script").arg(&script)
        .args(["--net", "none", "--max-seconds", "9", "--no-dump"])
        .output().expect("run esp32sim");
    let _ = std::fs::remove_file(&script);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("DMP enabled!"), "the ICM-20948 passes the SparkFun DMP setup:\n{text}");
    let field = |line: &str, key: &str| line.split(&format!("\"{key}\":")).nth(1)
        .and_then(|v| v.split([',', '}']).next()).and_then(|v| v.parse::<f64>().ok());
    let last = text.lines().rfind(|l| l.contains("\"T\":1001")).expect("T:1001 feedback");
    let (odl, odr, v) = (field(last, "odl").unwrap(), field(last, "odr").unwrap(), field(last, "v").unwrap());
    // About 3 s of motion before the heartbeat (3 s) stops the wheels: left forward, right back.
    assert!(odl > 50.0 && odr < -35.0, "odometry in cm: {last}");
    assert!((odl / -odr - 1.5).abs() < 0.3, "the PID holds the commanded ratio: {last}");
    assert!((v - 1180.0).abs() < 50.0, "11.8 V from the INA219: {last}");
}
