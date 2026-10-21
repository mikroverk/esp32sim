#[path = "support/camera_fixture.rs"]
mod fixture;

#[test]
#[ignore = "ESP32SIM_ROM_DIR must contain esp32_rev300_rom.elf"]
fn public_driver_captures_host_frame() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let roms = std::env::var_os("ESP32SIM_ROM_DIR").expect("set ESP32SIM_ROM_DIR");
    fixture::capture(
        &root.join("esp32/tests/fixtures/classic-camera"),
        &std::path::PathBuf::from(roms).join("esp32_rev300_rom.elf"),
    );
}
