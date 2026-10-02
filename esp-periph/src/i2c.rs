//! I2C master controller (I2C0/I2C1) and the `I2cDevice` trait the board's bus devices implement.
//! The controller executes the command list (RSTART/WRITE/READ/STOP/END) written by the driver
//! at `trans_start`, moving bytes between the FIFOs and the addressed device, and raises the
//! NACK / END_DETECT / TRANS_COMPLETE interrupts the IDF `i2c_master` driver waits for.
use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;
use std::collections::VecDeque;

pub trait I2cDevice {
    /// Address phase: the master addressed this device for a read (`read`) or a write. Return ACK.
    fn start(&mut self, _read: bool) -> bool { true }
    /// One data byte from the master. Return ACK.
    fn write(&mut self, b: u8) -> bool;
    /// One data byte to the master.
    fn read(&mut self) -> u8;
    fn stop(&mut self) {}
}

pub const INT_END_DETECT: u32 = 1 << 3;
pub const INT_TRANS_COMPLETE: u32 = 1 << 7;
pub const INT_TIMEOUT: u32 = 1 << 8;
pub const INT_NACK: u32 = 1 << 10;

pub struct I2c {
    pub regs: RegRam,
    tx: VecDeque<u8>,
    rx: VecDeque<u8>,
    pub int_raw: u32,
    pub int_ena: u32,
    cmd: [u32; 16],
    classic: bool,
    devices: Vec<(u8, Box<dyn I2cDevice>)>,
    cur: Option<usize>,
    expect_addr: bool,
    nack: bool,
    pub log: bool,
    pub transactions: u64,
}

impl I2c {
    pub fn new() -> Self {
        I2c { regs: RegRam::new(), tx: VecDeque::new(), rx: VecDeque::new(), int_raw: 0, int_ena: 0, cmd: [0; 16], classic: false, devices: Vec::new(), cur: None, expect_addr: false, nack: false,
              log: false, transactions: 0 }
    }
    /// Classic ESP32 has the same controller registers but 16 command slots and older opcodes.
    pub fn new_classic() -> Self { Self { classic: true, ..Self::new() } }
    /// A device attached at an occupied address replaces the one there: a board swapped before
    /// boot (`esp32sim_set_measured_te`) must not leave the old board's devices answering.
    pub fn attach(&mut self, addr: u8, dev: Box<dyn I2cDevice>) {
        match self.devices.iter_mut().find(|(attached, _)| *attached == addr) {
            Some(slot) => slot.1 = dev,
            None => self.devices.push((addr, dev)),
        }
    }
    pub fn has_device(&self, addr: u8) -> bool { self.devices.iter().any(|(attached, _)| *attached == addr) }
    pub fn irq(&self) -> bool { self.int_raw & self.int_ena != 0 }

    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            0x08 => (self.nack as u32) | (((self.int_raw & INT_TIMEOUT != 0) as u32) << 2) | ((self.rx.len() as u32 & 0x3f) << 8) | ((self.tx.len() as u32 & 0x3f) << 18),   // SR: resp_rec, timeout, rxfifo_cnt, txfifo_cnt
            0x14 => ((self.rx.len() as u32 & 0x1f) << 5) | ((self.tx.len() as u32 & 0x1f) << 15),                        // FIFO_ST: waddr = count, raddr = 0
            0x1c => self.rx.pop_front().unwrap_or(0) as u32,
            0x20 => self.int_raw,
            0x28 => self.int_ena,
            0x2c => self.int_raw & self.int_ena,
            0x58..=0x94 if self.classic || off <= 0x74 => self.cmd[((off - 0x58) / 4) as usize],
            _ => self.regs.read(off),
        }
    }

    /// FIFO reset bits are independent and may both be set by one write.
    #[allow(clippy::possible_missing_else)]
    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            0x04 => { self.regs.write(off, v & !(1 << 5)); if v & (1 << 5) != 0 { self.run(); } }               // CTR.TRANS_START
            0x18 => { if v & (1 << 13) != 0 { self.tx.clear(); } if v & (1 << 12) != 0 { self.rx.clear(); } self.regs.write(off, v & !(3 << 12)); }
            0x1c => { if self.tx.len() < 32 { self.tx.push_back(v as u8); } }
            0x24 => self.int_raw &= !v,
            0x28 => self.int_ena = v,
            0x58..=0x94 if self.classic || off <= 0x74 => self.cmd[((off - 0x58) / 4) as usize] = v & !(1 << 31),
            _ => self.regs.write(off, v),
        }
    }

    fn run(&mut self) {
        self.nack = false;
        self.transactions += 1;
        for i in 0..if self.classic { 16 } else { 8 } {
            let c = self.cmd[i];
            let op = match ((c >> 11) & 7, self.classic) {
                (0, true) => 6,
                (2, true) => 3,
                (3, true) => 2,
                (op, _) => op,
            };
            let n = (c & 0xff) as usize;
            let ack_check = c & (1 << 8) != 0;
            match op {
                6 => self.expect_addr = true,                                            // RSTART
                1 => {                                                                   // WRITE n bytes
                    for _ in 0..n {
                        let b = self.tx.pop_front().unwrap_or(0);
                        let ack = if self.expect_addr {
                            self.expect_addr = false;
                            let addr = b >> 1; let rd = b & 1 != 0;
                            self.cur = self.devices.iter().position(|(a, _)| *a == addr);
                            if self.log { eprintln!("[i2c] start addr {:#04x} {}{}", addr, if rd { "R" } else { "W" }, if self.cur.is_none() { " (no device)" } else { "" }); }
                            match self.cur { Some(k) => self.devices[k].1.start(rd), None => false }
                        } else {
                            if self.log { eprintln!("[i2c]   write {:#04x}", b); }
                            match self.cur { Some(k) => self.devices[k].1.write(b), None => false }
                        };
                        if !ack && ack_check {
                            self.nack = true; self.int_raw |= INT_NACK; self.cmd[i] |= 1 << 31;
                            self.cur = None;
                            return;
                        }
                    }
                }
                3 => {                                                                   // READ n bytes
                    for _ in 0..n {
                        let b = match self.cur { Some(k) => self.devices[k].1.read(), None => 0xff };
                        if self.log { eprintln!("[i2c]   read  {:#04x}", b); }
                        if self.rx.len() < 32 { self.rx.push_back(b); }
                    }
                }
                2 => {                                                                   // STOP
                    if let Some(k) = self.cur { self.devices[k].1.stop(); }
                    self.cur = None; self.cmd[i] |= 1 << 31; self.int_raw |= INT_TRANS_COMPLETE;
                    return;
                }
                4 => { self.cmd[i] |= 1 << 31; self.int_raw |= INT_END_DETECT; return; } // END: driver continues later
                _ => { self.cmd[i] |= 1 << 31; return; }
            }
            self.cmd[i] |= 1 << 31;
        }
    }
}

impl Default for I2c { fn default() -> Self { Self::new() } }

// ------------------------------------------------------------------ devices

/// Generic 8-bit-register device (audio codecs etc.): first written byte selects the register,
/// following bytes / reads auto-increment.
pub struct Reg8Device { pub name: &'static str, pub regs: [u8; 256], ptr: u8, first: bool }
impl Reg8Device {
    pub fn new(name: &'static str, defaults: &[(u8, u8)]) -> Self { let mut d = Reg8Device { name, regs: [0; 256], ptr: 0, first: true }; for &(r, v) in defaults { d.regs[r as usize] = v; } d }
}
impl I2cDevice for Reg8Device {
    fn start(&mut self, read: bool) -> bool { if !read { self.first = true; } true }
    fn write(&mut self, b: u8) -> bool { if self.first { self.ptr = b; self.first = false; } else { self.regs[self.ptr as usize] = b; self.ptr = self.ptr.wrapping_add(1); } true }
    fn read(&mut self) -> u8 { let v = self.regs[self.ptr as usize]; self.ptr = self.ptr.wrapping_add(1); v }
}

/// Waveshare's CH32V003 IO expander: regs 0x02 direction, 0x03 output, 0x04 input, 0x05 PWM, 0x06 ADC, 0x07 RTC.
impl Device for I2c {
    fn read(&mut self, off: u32) -> u32 { I2c::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { I2c::write(self, off, v); WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { self.irq() as u64 }
    fn debug(&mut self, on: bool) { self.log = on; }
}
