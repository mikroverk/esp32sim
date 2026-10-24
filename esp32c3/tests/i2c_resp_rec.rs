//! SR.RESP_REC after an I2C transaction, as esp-hal's blocking driver reads it.
use emu_core::Bus;
use esp_periph::i2c::I2cDevice;

const BASE: u32 = 0x6001_3000;
const SR: u32 = BASE + 0x08;
const INT_RAW: u32 = BASE + 0x20;
const NACK: u32 = 1 << 10;

struct Device;
impl I2cDevice for Device {
    fn write(&mut self, _: u8) -> bool { true }
    fn read(&mut self) -> u8 { 0x5a }
}

/// START, address+R with ack check, one byte read, STOP.
fn read_byte(bus: &mut esp32c3::bus::SocBus, addr: u8) {
    for (off, value) in [
        (0x24, u32::MAX),                       // INT_CLR
        (0x1c, ((addr << 1) | 1) as u32),       // TX FIFO: address, read
        (0x58, 6 << 11),                        // RSTART
        (0x5c, 1 << 11 | 1 << 8 | 1),           // WRITE 1 byte, ack check
        (0x60, 3 << 11 | 1),                    // READ 1 byte
        (0x64, 2 << 11),                        // STOP
        (0x04, 1 << 5),                         // TRANS_START
    ] {
        bus.write32(BASE + off, value).unwrap();
    }
    bus.tick(100_000);
}

/// esp-hal's `check_errors` reports a data NACK when TRANS_COMPLETE is set and RESP_REC is clear,
/// so a transaction the slave acknowledged must leave RESP_REC set and a NACKed one clear.
#[test]
fn resp_rec_is_set_after_an_acknowledged_transaction_and_clear_after_a_nack() {
    let mut m = esp32c3::machine([0; 6], 4 * 1024 * 1024);
    let bus = &mut m.bus;
    bus.periph.i2c.attach(0x42, Box::new(Device));

    read_byte(bus, 0x42);
    assert_eq!(bus.read32(INT_RAW).unwrap() & NACK, 0, "acknowledged");
    assert_eq!(bus.read32(SR).unwrap() & 1, 1, "RESP_REC set: the slave acknowledged");

    read_byte(bus, 0x43);
    assert_ne!(bus.read32(INT_RAW).unwrap() & NACK, 0, "no device at 0x43");
    assert_eq!(bus.read32(SR).unwrap() & 1, 0, "RESP_REC clear after a NACK");

    read_byte(bus, 0x42);
    assert_eq!(bus.read32(SR).unwrap() & 1, 1, "and set again by the next acknowledged transaction");
}
