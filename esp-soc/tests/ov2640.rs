//! Public-driver SCCB traces run without firmware, ROMs or network access.
use esp_periph::i2c::I2cDevice;
use esp_soc::camera::{Ov2640, Ov2640State};
use std::sync::{Arc, Mutex};

fn sensor() -> (Ov2640, Arc<Mutex<Ov2640State>>) {
    let state = Arc::new(Mutex::new(Ov2640State::default()));
    (Ov2640::new(state.clone()), state)
}
fn write(sensor: &mut Ov2640, reg: u8, value: u8) {
    assert!(sensor.start(false));
    assert!(sensor.write(reg));
    assert!(sensor.write(value));
    sensor.stop();
}
fn read(sensor: &mut Ov2640, reg: u8) -> u8 {
    assert!(sensor.start(false));
    assert!(sensor.write(reg));
    sensor.stop(); // v2.1.4 driver/sccb.c:SCCB_Read, not a repeated START.
    assert!(sensor.start(true));
    let value = sensor.read();
    sensor.stop();
    value
}
fn replay(sensor: &mut Ov2640, trace: &str) {
    // SCCB probe is an address-only write transaction at the configured 0x30.
    assert!(!sensor.start_address(0x30, 0x31, false));
    assert!(sensor.start_address(0x30, 0x30, false));
    sensor.stop();
    for line in trace.lines() {
        let words: Vec<_> = line.split_whitespace().collect();
        let reg = u8::from_str_radix(words[1], 16).unwrap();
        let value = u8::from_str_radix(words[2], 16).unwrap();
        match words[0] {
            "W" => write(sensor, reg, value),
            "R" => assert_eq!(read(sensor, reg), value, "{line}"),
            _ => panic!("invalid trace: {line}"),
        }
    }
}
const RESET: &str = include_str!("fixtures/ov2640/reset.txt");
const QQ_RGB: &str = include_str!("fixtures/ov2640/qqvga-rgb565.txt");
const VGA_RGB: &str = include_str!("fixtures/ov2640/vga-rgb565.txt");
const QQ_YUV: &str = include_str!("fixtures/ov2640/qqvga-yuv422.txt");
const VGA_JPEG: &str = include_str!("fixtures/ov2640/vga-jpeg.txt");
const UXGA_RGB: &str = include_str!("fixtures/ov2640/uxga-rgb565.txt");

#[test]
fn ov2640_classic_controller_replays_public_sccb_transactions() {
    use esp_periph::i2c::{I2c, INT_NACK, INT_TRANS_COMPLETE};
    let (sensor, state) = sensor();
    let mut bus = I2c::new_classic();
    bus.attach(0x30, Box::new(sensor));
    // Classic command opcodes: existing adapter's START=0, WRITE=1,
    // READ=2, STOP=3. No new controller behavior is introduced here.
    let transfer = |bus: &mut I2c<16>, bytes: &[u8], reading: bool| {
        bus.write(0x18, 3 << 12);
        bus.write(0x24, u32::MAX);
        for &byte in bytes { bus.write(0x1c, u32::from(byte)); }
        bus.write(0x58, 0);
        bus.write(0x5c, (1 << 11) | (1 << 8) | bytes.len() as u32);
        if reading {
            bus.write(0x60, (2 << 11) | (1 << 10) | 1);
            bus.write(0x64, 3 << 11);
        } else {
            bus.write(0x60, 3 << 11);
        }
        bus.write(4, 1 << 5);
        assert_eq!(bus.int_raw & INT_NACK, 0);
        assert_ne!(bus.int_raw & INT_TRANS_COMPLETE, 0);
    };
    transfer(&mut bus, &[0x60], false); // address-only probe
    for trace in [RESET, QQ_RGB, VGA_RGB, QQ_YUV, VGA_JPEG, UXGA_RGB] {
        for line in trace.lines() {
            let words: Vec<_> = line.split_whitespace().collect();
            let reg = u8::from_str_radix(words[1], 16).unwrap();
            let value = u8::from_str_radix(words[2], 16).unwrap();
            if words[0] == "W" {
                transfer(&mut bus, &[0x60, reg, value], false);
            } else {
                transfer(&mut bus, &[0x60, reg], false); // STOP after pointer
                transfer(&mut bus, &[0x61], true);
                assert_eq!(bus.read(0x1c), u32::from(value));
            }
        }
    }
    assert_eq!(*state.lock().unwrap(), Ov2640State {
        width: 1600, height: 1200, image_mode: 8, streaming: true,
    });
}

#[test]
fn ov2640_public_probe_reset_and_stop_reads() {
    let (mut sensor, state) = sensor();
    assert_eq!(*state.lock().unwrap(), Ov2640State::default());
    replay(&mut sensor, RESET);
    assert_eq!(*state.lock().unwrap(), Ov2640State::default());
    write(&mut sensor, 0xff, 1);
    for (reg, value) in [(0x0a, 0x26), (0x0b, 0x42), (0x1c, 0x7f), (0x1d, 0xa2)] {
        assert_eq!(read(&mut sensor, reg), value);
        write(&mut sensor, reg, 0); // ID fields are not ordinary writable storage.
        assert_eq!(read(&mut sensor, reg), value);
    }
    // Repeated START is also valid, with no STOP between pointer and read.
    assert!(sensor.start(false));
    assert!(sensor.write(0x0b));
    assert!(sensor.start(true));
    assert_eq!(sensor.read(), 0x42);
}

#[test]
fn ov2640_bank_isolated_ordinary_writes() {
    let (mut sensor, _) = sensor();
    for (bank, value) in [(0, 0x35), (1, 0xa9)] {
        write(&mut sensor, 0xff, bank);
        write(&mut sensor, 0x44, value); // DSP QS vs unrelated sensor storage.
        write(&mut sensor, 0x45, value ^ 0xff);
    }
    for (bank, value) in [(0, 0x35), (1, 0xa9)] {
        write(&mut sensor, 0xff, bank);
        assert_eq!(read(&mut sensor, 0xff), bank);
        assert_eq!(read(&mut sensor, 0x44), value);
        assert_eq!(read(&mut sensor, 0x45), value ^ 0xff);
    }
    write(&mut sensor, 0xff, 0);
    write(&mut sensor, 0x0a, 0x39);
    assert_eq!(read(&mut sensor, 0x0a), 0x39);
    write(&mut sensor, 0xff, 1);
    assert_eq!(read(&mut sensor, 0x0a), 0x26);
}

#[test]
fn ov2640_public_qqvga_vga_and_format_sequences() {
    let (mut sensor, state) = sensor();
    for (trace, width, height, image_mode) in [
        (QQ_RGB, 160, 120, 8), (VGA_RGB, 640, 480, 8),
        (QQ_YUV, 160, 120, 0), (VGA_JPEG, 640, 480, 0x12),
    ] {
        replay(&mut sensor, trace);
        assert_eq!(*state.lock().unwrap(), Ov2640State { width, height, image_mode, streaming: true });
    }
    // Also change size/format on the same live sensor, without software reset.
    for (trace, width, height, image_mode) in [
        (QQ_RGB, 160, 120, 8), (VGA_JPEG, 640, 480, 0x12),
        (QQ_YUV, 160, 120, 0), (VGA_RGB, 640, 480, 8),
    ] {
        replay(&mut sensor, trace.strip_prefix(RESET).unwrap());
        assert_eq!(*state.lock().unwrap(), Ov2640State { width, height, image_mode, streaming: true });
    }
    // Driver grayscale uses the exact YUV422 sequence, not a Y8 register mode.
    replay(&mut sensor, QQ_YUV);
    assert_eq!(state.lock().unwrap().image_mode, 0);
}

#[test]
fn ov2640_public_uxga_requires_high_geometry_bits() {
    let (mut sensor, state) = sensor();
    replay(&mut sensor, UXGA_RGB);
    assert_eq!(*state.lock().unwrap(), Ov2640State {
        width: 1600, height: 1200, image_mode: 8, streaming: true,
    });
    // Isolate each extension and ignore reserved ZMHH bits.
    write(&mut sensor, 0x5c, 0xf9);
    assert_eq!((state.lock().unwrap().width, read(&mut sensor, 0x5c)), (1600, 0xf9));
    assert_eq!(state.lock().unwrap().height, 176);
    write(&mut sensor, 0x5c, 0xfc);
    assert_eq!(state.lock().unwrap().width, 576);
    assert_eq!(state.lock().unwrap().height, 1200);
}

#[test]
fn ov2640_com7_reset_clears_both_banks_and_stream() {
    let (mut sensor, state) = sensor();
    replay(&mut sensor, VGA_JPEG);
    write(&mut sensor, 0x44, 0x2f);
    write(&mut sensor, 0xff, 1);
    write(&mut sensor, 0x44, 0x3d);
    write(&mut sensor, 0x12, 0x80);
    assert_eq!(*state.lock().unwrap(), Ov2640State::default());
    assert_eq!(read(&mut sensor, 0xff), 1, "driver bank cache survives software reset");
    assert_eq!(read(&mut sensor, 0x12), 0, "COM7_SRST self-clears");
    assert_eq!(read(&mut sensor, 0x44), 0);
    assert_eq!(read(&mut sensor, 0x0a), 0x26);
    write(&mut sensor, 0xff, 0);
    assert_eq!(read(&mut sensor, 0x44), 0);
    assert_eq!(read(&mut sensor, 0x5a), 0);
    assert_eq!(read(&mut sensor, 0xda), 0);
    replay(&mut sensor, QQ_RGB);
    assert!(state.lock().unwrap().streaming);
}

#[test]
fn ov2640_stream_gates_are_bank_specific_and_reversible() {
    let (mut sensor, state) = sensor();
    replay(&mut sensor, QQ_RGB);
    for reg in [0xe0, 0x05] {
        write(&mut sensor, reg, if reg == 0xe0 { 4 } else { 1 });
        assert!(!state.lock().unwrap().streaming);
        write(&mut sensor, reg, 0);
        assert!(state.lock().unwrap().streaming);
    }
    write(&mut sensor, 0xff, 1);
    write(&mut sensor, 0x09, 0x12);
    assert!(!state.lock().unwrap().streaming);
    write(&mut sensor, 0x09, 2);
    assert!(state.lock().unwrap().streaming);
    // Sensor-bank aliases must not change DSP geometry/format/reset/bypass.
    for (reg, value) in [(0x5a, 1), (0x5b, 2), (0x5c, 7), (0xda, 0x10), (0xe0, 4), (0x05, 1)] {
        write(&mut sensor, reg, value);
    }
    assert_eq!(*state.lock().unwrap(), Ov2640State { width: 160, height: 120, image_mode: 8, streaming: true });
    write(&mut sensor, 0xff, 0);
    write(&mut sensor, 0x09, 0x10);
    write(&mut sensor, 0x12, 0x80); // Not sensor COM7; must not reset.
    assert!(state.lock().unwrap().streaming);
    assert_eq!(state.lock().unwrap().width, 160);
}
