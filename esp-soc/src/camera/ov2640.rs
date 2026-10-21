//! OV2640 SCCB register model. Register meanings follow esp32-camera v2.1.4
//! (b817e9b8), sensors/private_include/ov2640_regs.h and ov2640.c:set_window.
//! No ISP, JPEG encoder, board wiring, DVP timing or capture peripheral is modeled.
use esp_periph::i2c::I2cDevice;
use std::sync::{Arc, Mutex};

/// OV2640 output configuration; not OV5640's `SensorState` register encoding.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ov2640State {
    pub width: u32,
    pub height: u32,
    /// Raw DSP IMAGE_MODE (0xda): YUV422=0, RGB565=8, JPEG enable=0x10.
    /// Grayscale in the public driver is YUV422 on the wire, not Y8 output.
    pub image_mode: u8,
    /// A configured output with DVP out of reset and sensor out of standby.
    /// This says nothing about external power, XCLK or physical routing.
    pub streaming: bool,
}

/// Two 8-bit register banks at board-configured SCCB address (normally 0x30).
/// Unmodeled ISP registers retain their last write for driver read/modify/write;
/// indirect ISP tables and hardware reset defaults are not simulated.
/// Fresh/reset geometry is deliberately unconfigured (zero), not a silicon default.
pub struct Ov2640 {
    regs: [[u8; 256]; 2],
    bank: u8,
    addr: u8,
    pointer: bool,
    state: Arc<Mutex<Ov2640State>>,
}
impl Ov2640 {
    pub fn new(state: Arc<Mutex<Ov2640State>>) -> Self {
        let mut sensor = Self { regs: [[0; 256]; 2], bank: 1, addr: 0, pointer: true, state };
        sensor.reset();
        sensor
    }

    fn reset(&mut self) {
        self.regs = [[0; 256]; 2];
        // Identification values used by ov2640.c:esp32_camera_ov2640_detect.
        // VER 0x42 and manufacturer 0x7fa2 identify the modeled OV2640 revision.
        for (reg, value) in [(0x0a, 0x26), (0x0b, 0x42), (0x1c, 0x7f), (0x1d, 0xa2)] {
            self.regs[1][reg] = value;
        }
        // Preserve selected bank: the driver's cached set_bank remains SENSOR
        // immediately after writing COM7_SRST. Reset clears the self-clearing bit.
        self.sync_state();
    }

    fn sync_state(&self) {
        let dsp = &self.regs[0];
        // ov2640.c:set_window divides dimensions by four, with ZMHH holding
        // width bits 9:8 and height bit 8 (ov2640_regs.h:ZMOW/ZMOH/ZMHH).
        let width = (u32::from(dsp[0x5a]) | (u32::from(dsp[0x5c] & 3) << 8)) * 4;
        let height = (u32::from(dsp[0x5b]) | (u32::from(dsp[0x5c] & 4) << 6)) * 4;
        *self.state.lock().unwrap() = Ov2640State {
            width, height, image_mode: dsp[0xda],
            // ov2640_regs.h:COM2_STDBY, RESET_DVP. DSP bypass is not a
            // converted DSP output; conservatively withhold a usable stream.
            streaming: width != 0 && height != 0 && self.regs[1][0x09] & 0x10 == 0
                && dsp[0xe0] & 4 == 0 && dsp[0x05] & 1 == 0,
        };
    }
}
impl I2cDevice for Ov2640 {
    fn start(&mut self, read: bool) -> bool {
        // SCCB_Read sends pointer+STOP, then a separate read transaction.
        if !read { self.pointer = true; }
        true
    }
    fn write(&mut self, value: u8) -> bool {
        if self.pointer {
            self.addr = value;
            self.pointer = false;
        } else if self.addr == 0xff {
            self.bank = value & 1;
        } else if self.bank == 1 && self.addr == 0x12 && value & 0x80 != 0 {
            self.reset();
        } else {
            // Sensor identification is read-only; DSP registers at the same
            // addresses are ordinary storage, not aliases of the sensor IDs.
            if self.bank != 1 || ![0x0a, 0x0b, 0x1c, 0x1d].contains(&self.addr) {
                self.regs[self.bank as usize][self.addr as usize] = value;
                self.sync_state();
            }
        }
        true
    }
    fn read(&mut self) -> u8 {
        if self.addr == 0xff { self.bank } else { self.regs[self.bank as usize][self.addr as usize] }
    }
}
