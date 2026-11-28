//! A bare board's sensors move on guest time only if the S3 bus keeps advancing the board clock.
//! The bus calls `board.advance_to` when `board_edges` is set, and that flag is cached from
//! `uses_gpio_edges()` at attach — so an I2C-only board with no panel used to sit at cycle 0 for
//! the whole run, and `--imu-motion 1` never left its first frame.
//!
//! The test goes through the bus on purpose: calling `advance_to` by hand cannot see the flag.
use emu_core::Bus;
use esp_periph::i2c::I2cDevice;
use esp_soc::board::{ConfigurableBoard, DeviceSpec};
use esp_soc::BoardModel;

const CPU_HZ: u64 = esp32s3::periph::CPU_HZ;

/// Read the six accelerometer output bytes of an LSM6DS3 at 0x28.
fn read_accel(dev: &mut Box<dyn I2cDevice>) -> [u8; 6] {
    dev.start(false);
    dev.write(0x28);
    dev.start(true);
    let mut out = [0u8; 6];
    for b in out.iter_mut() { *b = dev.read(); }
    out
}

/// 4 s and 18 s of guest time must not read the same posture. If the clock never advances, both
/// reads return the first frame of the script and the assertion fires.
#[test]
fn a_bare_i2c_board_moves_its_sensor_on_guest_time() {
    let specs = vec![DeviceSpec::parse("0:0x6a:lsm6ds3").unwrap()];
    let mut board = ConfigurableBoard::new(specs, CPU_HZ);
    assert!(board.set_imu_motion(1), "a bare board accepts the motion selector");

    let mut m = esp32s3::machine([1, 2, 3, 4, 5, 6]);
    m.bus.board = Box::new(board);
    m.bus.attach_board_devices();

    for _ in 0..40 { m.bus.tick((CPU_HZ / 10) as u32); }        // 4 s
    let early = read_accel(&mut m.bus.board.i2c_devices().pop().expect("the LSM6DS3").2);
    for _ in 0..140 { m.bus.tick((CPU_HZ / 10) as u32); }       // up to 18 s
    let late = read_accel(&mut m.bus.board.i2c_devices().pop().expect("the LSM6DS3").2);

    assert_ne!(early, late, "guest time must reach the sensor: {early:?} vs {late:?}");
}
