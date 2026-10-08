//! I2C master controller (I2C0/I2C1) and the `I2cDevice` trait the board's bus devices implement.
//! The controller executes the command list (RSTART/WRITE/READ/STOP/END) written by the driver
//! after `trans_start`, clocking bytes between the FIFOs and addressed devices, and raises the
//! NACK / END_DETECT / TRANS_COMPLETE interrupts the IDF `i2c_master` driver waits for.
use crate::device::{Device, WriteEffect};
use crate::regram::RegRam;
use std::collections::VecDeque;
use emu_core::ClockDomain;

pub trait I2cDevice {
    /// Current 7-bit address, sampled after each data write. Defaults to the attached address.
    fn address(&self, configured: u8) -> u8 { configured }
    /// Match the address and start the transfer; override for aliases or general calls.
    fn start_address(&mut self, configured: u8, address: u8, read: bool) -> bool {
        self.address(configured) == address && self.start(read)
    }
    /// Optional physical (SDA, SCL) attachment. None keeps controller-only addressing.
    fn pins(&self) -> Option<(u8, u8)> { None }
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
pub const INT_NACK: u32 = 1 << 10;

pub struct I2c {
    pub regs: RegRam,
    tx: VecDeque<u8>,
    rx: VecDeque<u8>,
    pub int_raw: u32,
    pub int_ena: u32,
    cmd: [u32; 8],
    devices: Vec<(u8, Box<dyn I2cDevice>)>,
    cur: Vec<usize>,
    pins: Option<(u8, u8)>,
    expect_addr: bool,
    nack: bool,
    pub log: bool,
    pub transactions: u64,
    active: bool,
    command_index: usize,
    byte_index: usize,
    remaining: u64,
    /// C6 supplies its PCR divider in the S3/C3 CLK_CONF layout.
    pub external_clock_config: Option<u32>,
}

impl I2c {
    pub fn new() -> Self {
        I2c { regs: RegRam::new(), tx: VecDeque::new(), rx: VecDeque::new(), int_raw: 0, int_ena: 0, cmd: [0; 8], devices: Vec::new(), cur: Vec::new(), pins: None, expect_addr: false, nack: false,
              log: false, transactions: 0, active: false, command_index: 0, byte_index: 0, remaining: 0, external_clock_config: None }
    }
    /// A device attached at an occupied address and pin pair replaces the one there: a board swapped before
    /// boot (`esp32sim_set_measured_te`) must not leave the old board's devices answering.
    pub fn attach(&mut self, addr: u8, dev: Box<dyn I2cDevice>) {
        match self.devices.iter_mut().find(|(attached, old)| *attached == addr && old.pins() == dev.pins()) {
            Some(slot) => slot.1 = dev,
            None => self.devices.push((addr, dev)),
        }
    }
    pub fn has_pinned_devices(&self) -> bool { self.devices.iter().any(|(_, d)| d.pins().is_some()) }
    /// Current controller route, supplied by the SoC before starting a command list.
    pub fn set_pins(&mut self, pins: Option<(u8, u8)>) {
        if self.pins != pins { self.cur.clear(); }
        self.pins = pins;
    }
    /// Remove a device, returning it so the host can move it to another bus or address.
    /// Other selected recipients remain active. With none left, writes NACK and reads return 0xff.
    pub fn detach(&mut self, addr: u8) -> Option<Box<dyn I2cDevice>> {
        let index = self.devices.iter().position(|(attached, _)| *attached == addr)?;
        self.cur.retain_mut(|cur| { if *cur == index { return false; } *cur -= usize::from(*cur > index); true });
        Some(self.devices.remove(index).1)
    }
    /// Remove every device without resetting registers, FIFOs or interrupt status.
    pub fn clear_devices(&mut self) {
        self.cur.clear();
        self.devices.clear();
    }
    pub fn has_device(&self, addr: u8) -> bool { self.devices.iter().any(|(attached, _)| *attached == addr) }
    #[inline(always)]
    pub fn is_active(&self) -> bool { self.active }
    pub fn irq(&self) -> bool { self.int_raw & self.int_ena != 0 }

    pub fn read(&mut self, off: u32) -> u32 {
        match off {
            // IDF v5.5.4 S3 i2c_reg.h:173-179: BUS_BUSY bit 4.
            0x08 => (self.nack as u32) | ((self.active as u32) << 4) | ((self.rx.len() as u32 & 0x3f) << 8) | ((self.tx.len() as u32 & 0x3f) << 18),   // SR: resp_rec, bus_busy, rxfifo_cnt, txfifo_cnt
            0x14 => ((self.rx.len() as u32 & 0x1f) << 5) | ((self.tx.len() as u32 & 0x1f) << 15),                        // FIFO_ST: waddr = count, raddr = 0
            0x1c => self.rx.pop_front().unwrap_or(0) as u32,
            0x20 => self.int_raw,
            0x28 => self.int_ena,
            0x2c => self.int_raw & self.int_ena,
            0x58..=0x74 => self.cmd[((off - 0x58) / 4) as usize],
            _ => self.regs.read(off),
        }
    }

    /// FIFO reset bits are independent and may both be set by one write.
    #[allow(clippy::possible_missing_else)]
    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            // IDF v5.5.4 components/soc/esp32s3/register/soc/i2c_reg.h:
            // TRANS_START bit 5 (72-78), FSM_RST bit 10 (111-117).
            // Reset cancellation of a pending callback is inferred.
            0x04 => {
                let reset = v & (1 << 10) != 0;
                self.regs.write(off, v & !((1 << 5) | (1 << 10)));
                if reset { self.active = false; self.cur.clear(); self.expect_addr = false; self.nack = false; }
                if v & (1 << 5) != 0 { self.run(); }
            }               // CTR.TRANS_START
            0x18 => { if v & (1 << 13) != 0 { self.tx.clear(); } if v & (1 << 12) != 0 { self.rx.clear(); } self.regs.write(off, v & !(3 << 12)); }
            0x1c => { if self.tx.len() < 32 { self.tx.push_back(v as u8); } }
            0x24 => self.int_raw &= !v,
            0x28 => self.int_ena = v,
            0x58..=0x74 => self.cmd[((off - 0x58) / 4) as usize] = v & !(1 << 31),
            _ => self.regs.write(off, v),
        }
    }

    fn op(&self) -> u32 {
        (self.cmd[self.command_index] >> 11) & 7
    }

    // IDF v5.5.4 components/hal/esp32s3/include/hal/i2c_ll.h:205-235:
    // divider = NUM + 1 + B/A (the register-header A/B prose is reversed).
    // components/soc/esp32s3/register/soc/i2c_reg.h:1043-1074; C3 has the same fields.
    // C6 uses PCR instead. clk_tree_defs.h:39 (C6:47) gives nominal 17.5 MHz
    // RC_FAST; XTAL is modeled at 40 MHz and APB at 80 MHz. IDF 4.4 uses a
    // nominal 20 MHz RC clock; this model does not reproduce oscillator calibration.
    fn clock_ticks(&self, cycles: u64) -> u64 {
        let c = self.external_clock_config.unwrap_or_else(|| self.regs.read(0x54));
        let a = u64::from((c >> 8) & 63);
        let b = u64::from((c >> 14) & 63);
        let denominator = a.max(1);
        let divisor = (u64::from(c & 255) + 1) * denominator + if a > 0 { b } else { 0 };
        let hz = if c & (1 << 20) != 0 { 17_500_000 } else { 40_000_000 };
        (cycles * divisor * 80_000_000).div_ceil(hz * denominator).max(1)
    }

    // Inferred byte/ACK timing, not measured on hardware. IDF v5.5.4
    // components/hal/{esp32s3,esp32c3,esp32c6}/include/hal/i2c_ll.h
    // master_set_bus_timing (S3:175-201, C6:173-200): low/setup/hold are
    // programmed minus one; high and wait-high are literal. IDF v4.4.7
    // S3:160-171 and C3:164-175 use the same convention.
    // S3 i2c_reg.h:16-26, 932-1006 gives 9-bit periods and 7-bit wait-high.
    fn schedule(&mut self) {
        if self.command_index >= 8 { self.active = false; return; }
        let mask = 0x1ff;
        let cycles = match self.op() {
            1 | 3 if self.cmd[self.command_index] & 255 != 0 => {
                let high = self.regs.read(0x38);
                let extra = (high >> 9) & 127;
                ((self.regs.read(0) & mask) + 1 + (high & mask) + extra) * 9
            }
            op @ (6 | 2) => {
                let off = if op == 6 { 0x40 } else { 0x48 };
                (self.regs.read(off) & mask) + (self.regs.read(off + 4) & mask) + 2
            }
            _ => 0,
        };
        self.remaining = self.clock_ticks(cycles.into());
    }

    fn run(&mut self) {
        if self.active { return; }
        self.nack = false;
        self.transactions += 1;
        self.active = true;
        self.command_index = 0;
        self.byte_index = 0;
        self.schedule();
    }

    fn advance(&mut self, mut ticks: u64) {
        while self.active && ticks >= self.remaining {
            ticks -= self.remaining;
            self.step();
            if self.active { self.schedule(); }
        }
        if self.active { self.remaining -= ticks; }
    }

    fn step(&mut self) {
        let i = self.command_index;
        let c = self.cmd[i];
        let n = (c & 255) as usize;
        match self.op() {
            6 => self.expect_addr = true,
            1 if self.byte_index < n => {
                let b = self.tx.pop_front().unwrap_or(0);
                let ack = if self.expect_addr {
                    self.expect_addr = false;
                    let addr = b >> 1; let rd = b & 1 != 0;
                    self.cur.clear();
                    for (k, (configured, device)) in self.devices.iter_mut().enumerate() {
                        if (device.pins().is_none() || device.pins() == self.pins) && device.start_address(*configured, addr, rd) {
                            self.cur.push(k);
                        }
                    }
                    if self.log { eprintln!("[i2c] start addr {addr:#04x} read={rd}"); }
                    !self.cur.is_empty()
                } else {
                    if self.log { eprintln!("[i2c]   write {b:#04x}"); }
                    let mut ack = false;
                    for &k in &self.cur {
                        let (addr, device) = &mut self.devices[k];
                        ack |= device.write(b);
                        *addr = device.address(*addr);
                    }
                    ack
                };
                if !ack && c & (1 << 8) != 0 {
                    self.nack = true; self.int_raw |= INT_NACK; self.cmd[i] |= 1 << 31;
                    self.cur.clear(); self.active = false;
                    return;
                }
                self.byte_index += 1;
                if self.byte_index < n { return; }
            }
            3 if self.byte_index < n => {
                let mut b = 0xff;
                for &k in &self.cur { b &= self.devices[k].1.read(); }
                if self.log { eprintln!("[i2c]   read  {b:#04x}"); }
                if self.rx.len() < 32 { self.rx.push_back(b); }
                self.byte_index += 1;
                if self.byte_index < n { return; }
            }
            1 | 3 => {}
            2 => {
                for &k in &self.cur { self.devices[k].1.stop(); }
                self.cur.clear(); self.int_raw |= INT_TRANS_COMPLETE; self.active = false;
            }
            4 => { self.int_raw |= INT_END_DETECT; self.active = false; }
            _ => self.active = false,
        }
        self.cmd[i] |= 1 << 31;
        self.command_index += 1;
        self.byte_index = 0;
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

impl Device for I2c {
    fn read(&mut self, off: u32) -> u32 { I2c::read(self, off) }
    fn write(&mut self, off: u32, v: u32) -> WriteEffect { I2c::write(self, off, v); WriteEffect::NONE }
    fn irq_sources(&self) -> u64 { self.irq() as u64 }
    fn clock(&self) -> Option<ClockDomain> { self.active.then_some(ClockDomain::Apb) }
    fn tick(&mut self, ticks: u64) { self.advance(ticks); }
    fn has_deadline(&self) -> bool { self.active }
    fn next_deadline(&self) -> Option<u64> { self.active.then_some(self.remaining) }
    fn debug(&mut self, on: bool) { self.log = on; }
}
